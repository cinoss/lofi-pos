//! Runtime in-memory aggregate state. Disk events.db is the durable log
//! and crash-recovery source; this struct holds the live projection that
//! commands read and mutate.
//!
//! Concurrency: `DashMap`'s shard locks let distinct aggregates proceed
//! without contention. The `agg_lock` (`KeyMutex<String>`) in
//! `CommandService::execute` serializes validate-then-apply for one
//! aggregate id, preventing TOCTOU between reads and writes. Idempotency
//! cache is mirrored to `master.idempotency_key` for restart durability.

use crate::domain::order::OrderState;
use crate::domain::payment::PaymentState;
use crate::domain::session::SessionState;
use dashmap::DashMap;

pub struct AggregateStore {
    pub sessions: DashMap<String, SessionState>,
    pub orders: DashMap<String, OrderState>,
    pub payments: DashMap<String, PaymentState>,
    /// Idempotency cache (key -> result_json). Mirrored to master.db for restart.
    pub idem: DashMap<String, String>,
}

impl AggregateStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            orders: DashMap::new(),
            payments: DashMap::new(),
            idem: DashMap::new(),
        }
    }
}

impl Default for AggregateStore {
    fn default() -> Self {
        Self::new()
    }
}

use crate::business_day::business_day_of;
use crate::crypto::{Dek, Kek};
use crate::domain::apply::{apply, ApplyCtx};
use crate::domain::event::DomainEvent;
use crate::error::{AppError, AppResult};
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::Clock;
use chrono::FixedOffset;
use std::collections::HashMap;

impl AggregateStore {
    /// Restore in-memory state from durable storage.
    ///
    /// 1. Unwrap every active business day's DEK so we can decrypt events.
    /// 2. Replay live aggregates (those without a terminal event) into apply().
    /// 3. Repopulate idempotency cache for the current business day.
    ///
    /// Costs O(events for live aggregates x decrypt). At POS scale this is
    /// well under 100ms even for a busy night with cross-midnight sessions.
    pub fn warm_up(
        &self,
        master: &Master,
        events: &EventStore,
        kek: &Kek,
        clock: &dyn Clock,
        tz: FixedOffset,
        cutoff_hour: u32,
    ) -> AppResult<WarmUpStats> {
        // Step 1: derive DEKs for every retained UTC day. After the UTC key
        // rotation refactor, `event.key_id` stores `utc_day_of(ts)` (decoupled
        // from `business_day`), and DEKs live in the `dek` table keyed by UTC
        // day. Warm-up just needs every currently-held key.
        let active_days: Vec<String> = master
            .list_dek_days()?
            .into_iter()
            .map(|i| i.utc_day)
            .collect();
        let mut deks: HashMap<String, Dek> = HashMap::new();
        for day in &active_days {
            if let Some(wrapped) = master.get_dek(day)? {
                deks.insert(day.clone(), kek.unwrap(&wrapped)?);
            }
        }

        // Step 2: replay live aggregates. Events for one aggregate can mutate
        // a sibling aggregate (e.g., OrderPlaced pushes into the session's
        // order_ids index). To preserve causal ordering, collect all rows
        // across live aggregates and replay in global event-id (= insertion)
        // order.
        let live = events.list_live_aggregate_ids()?;
        let mut all_rows = Vec::new();
        for agg in &live {
            all_rows.extend(events.list_for_aggregate(agg)?);
        }
        all_rows.sort_by_key(|r| r.id);
        let mut events_replayed = 0usize;
        for row in &all_rows {
            let dek = deks.get(&row.key_id).ok_or_else(|| {
                AppError::Internal(format!("warm_up: missing DEK for key_id {}", row.key_id))
            })?;
            let aad = format!(
                "{}|{}|{}|{}",
                row.business_day, row.event_type, row.aggregate_id, row.key_id
            );
            let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
            let ev: DomainEvent = serde_json::from_slice(&pt)
                .map_err(|e| AppError::Internal(format!("warm_up deserialize: {e}")))?;
            apply(
                self,
                &ev,
                ApplyCtx {
                    aggregate_id: &row.aggregate_id,
                },
            )?;
            events_replayed += 1;
        }

        // Step 3: repopulate idempotency cache (current business day).
        let today = business_day_of(clock.now(), tz, cutoff_hour);
        let idem_rows = master.list_idempotency_for_day(&today)?;
        let idem_count = idem_rows.len();
        for (key, json) in idem_rows {
            self.idem.insert(key, json);
        }

        Ok(WarmUpStats {
            aggregates_replayed: live.len(),
            events_replayed,
            idem_rows_loaded: idem_count,
            active_days: active_days.len(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WarmUpStats {
    pub aggregates_replayed: usize,
    pub events_replayed: usize,
    pub idem_rows_loaded: usize,
    pub active_days: usize,
}
