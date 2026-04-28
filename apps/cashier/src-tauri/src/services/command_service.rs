use crate::acl::{policy, Action};
use crate::auth::token::TokenClaims;
use crate::auth::AuthService;
use crate::domain::apply::{apply, ApplyCtx};
use crate::domain::event::DomainEvent;
use crate::error::{AppError, AppResult};
use crate::services::event_service::{EventService, WriteCtx};
use crate::services::locking::KeyMutex;
use crate::services::validation;
use crate::store::aggregate_store::AggregateStore;
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::Clock;
use std::sync::{Arc, Mutex};

pub struct CommandService {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub event_service: EventService,
    pub clock: Arc<dyn Clock>,
    pub auth: Arc<AuthService>,
    pub idem_lock: Arc<KeyMutex<String>>,
    pub agg_lock: Arc<KeyMutex<String>>,
    pub store: Arc<AggregateStore>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<crate::http::broadcast::EventNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Inserted,
    Cached,
}

impl CommandService {
    /// Run the full command pipeline for a single domain event:
    /// ACL guard (with optional override PIN) -> per-key idempotency lock ->
    /// memory-cache check -> per-aggregate validate-write-apply lock ->
    /// validation against memory -> encrypt+append to events.db ->
    /// apply mutation to memory -> project from memory -> persist cache.
    /// Returns either the freshly produced or the cached projection along
    /// with a `WriteOutcome` indicating whether a write actually occurred.
    #[allow(clippy::too_many_arguments)]
    pub fn execute<T, F>(
        &self,
        actor: &TokenClaims,
        action: Action,
        ctx: policy::PolicyCtx,
        idempotency_key: &str,
        command_name: &str,
        aggregate_id: &str,
        event: DomainEvent,
        override_pin: Option<&str>,
        project: F,
    ) -> AppResult<(T, WriteOutcome)>
    where
        F: FnOnce(&Self) -> AppResult<T>,
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        // ACL — first pass against actor's role. The requester (actor.staff_id from
        // the token) is ALWAYS the recorded actor; if the role is insufficient and an
        // override PIN clears it, the authorizer is recorded separately so audit can
        // show "X did Y, authorized by Z" without losing either identity.
        let actor_name = self
            .master
            .lock()
            .unwrap()
            .get_staff(actor.staff_id)?
            .map(|s| s.name);
        let mut override_staff: Option<(i64, Option<String>)> = None;
        match policy::check(action, actor.role, ctx) {
            policy::Decision::Allow => {}
            policy::Decision::Deny => return Err(AppError::Unauthorized),
            policy::Decision::OverrideRequired(min) => {
                let pin = override_pin.ok_or(AppError::OverrideRequired(min))?;
                let staff = self.auth.verify_pin_for_role(pin, min)?;
                let name = self
                    .master
                    .lock()
                    .unwrap()
                    .get_staff(staff.id)?
                    .map(|s| s.name);
                override_staff = Some((staff.id, name));
            }
        }

        // TOCTOU window closed: serialize same-key callers process-wide.
        let _idem_guard = self.idem_lock.lock(idempotency_key.to_string());

        // Memory cache check (mirrored to disk for restart durability).
        if let Some(cached) = self.store.idem.get(idempotency_key) {
            let v: T = serde_json::from_str(cached.value())
                .map_err(|e| AppError::Internal(format!("idempotency cached parse: {e}")))?;
            return Ok((v, WriteOutcome::Cached));
        }

        // Per-aggregate validate-write-apply serialization.
        let _agg_guard = self.agg_lock.lock(aggregate_id.to_string());

        // Validate against in-memory state.
        validation::validate(&self.store, aggregate_id, &event)?;

        // Encrypt + append to events.db (durable log). actor_staff is ALWAYS the
        // requester; override_staff_* is populated only when the override path fired.
        let (override_id, override_name) = match &override_staff {
            Some((id, name)) => (Some(*id), name.as_deref()),
            None => (None, None),
        };
        self.event_service.write(
            WriteCtx {
                aggregate_id,
                actor_staff: Some(actor.staff_id),
                actor_name: actor_name.as_deref(),
                override_staff_id: override_id,
                override_staff_name: override_name,
                at: None,
            },
            &event,
        )?;

        // Mutate memory.
        apply(&self.store, &event, ApplyCtx { aggregate_id })?;

        // Notify WS subscribers (best-effort; SendError = no live receivers).
        let _ = self
            .broadcast_tx
            .send(crate::http::broadcast::EventNotice::appended(
                event.event_type().as_str(),
                aggregate_id,
                self.clock.now().timestamp_millis(),
            ));

        // Project from memory.
        let projection = project(self)?;

        // Persist cache row to memory + disk.
        let now = self.clock.now().timestamp_millis();
        let json = serde_json::to_string(&projection)
            .map_err(|e| AppError::Internal(format!("idempotency serialize: {e}")))?;
        self.store
            .idem
            .insert(idempotency_key.to_string(), json.clone());
        self.master
            .lock()
            .unwrap()
            .put_idempotency(idempotency_key, command_name, &json, now)?;

        Ok((projection, WriteOutcome::Inserted))
    }

    /// Hot read: clone session state out of the in-memory store.
    pub fn load_session(
        &self,
        session_id: &str,
    ) -> AppResult<Option<crate::domain::session::SessionState>> {
        Ok(self.store.sessions.get(session_id).map(|r| r.clone()))
    }

    /// Hot read: clone order state out of the in-memory store.
    pub fn load_order(
        &self,
        order_id: &str,
    ) -> AppResult<Option<crate::domain::order::OrderState>> {
        Ok(self.store.orders.get(order_id).map(|r| r.clone()))
    }

    /// All sessions whose status is currently Open. Filters DashMap directly;
    /// per-shard locks let the iteration race with concurrent writers safely
    /// (each entry is cloned out under its shard lock).
    pub fn list_active_sessions(&self) -> AppResult<Vec<crate::domain::session::SessionState>> {
        Ok(self
            .store
            .sessions
            .iter()
            .filter(|r| r.value().status == crate::domain::session::SessionStatus::Open)
            .map(|r| r.value().clone())
            .collect())
    }

    /// Sum live subtotal across every order placed under `session_id`,
    /// including orders inherited via `apply(SessionMerged)` from absorbed
    /// source sessions. Returns `NotFound` if the session is not in memory.
    pub fn compute_bill(&self, session_id: &str) -> AppResult<i64> {
        let s = self
            .store
            .sessions
            .get(session_id)
            .ok_or(AppError::NotFound)?;
        let order_ids = s.order_ids.clone();
        drop(s);
        let mut total = 0i64;
        for oid in order_ids {
            if let Some(o) = self.store.orders.get(&oid) {
                total += o.live_subtotal();
            }
        }
        Ok(total)
    }
}
