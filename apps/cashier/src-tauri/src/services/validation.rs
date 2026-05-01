use crate::domain::event::DomainEvent;
use crate::domain::session::SessionStatus;
use crate::error::{AppError, AppResult};
use crate::store::aggregate_store::AggregateStore;

/// Pre-write invariant guard for domain events.
///
/// Reads in-memory state from `AggregateStore`. Race-free per aggregate
/// because `CommandService::execute` holds the per-aggregate `agg_lock`
/// for the duration of validate -> write -> apply, and `apply` mutates
/// the store which is the source of truth for subsequent validations.
pub fn validate(store: &AggregateStore, aggregate_id: &str, ev: &DomainEvent) -> AppResult<()> {
    match ev {
        DomainEvent::SessionClosed { .. } => {
            let s = store
                .sessions
                .get(aggregate_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session {aggregate_id} status {:?}, cannot close",
                    s.status
                )));
            }
        }
        DomainEvent::SessionTransferred { to, .. } => {
            let s = store
                .sessions
                .get(aggregate_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session {aggregate_id} status {:?}, cannot transfer",
                    s.status
                )));
            }
            // Reject if any OTHER currently-Open session occupies the target spot.
            let target_spot_id = to.id();
            drop(s);
            let conflict = store.sessions.iter().find(|entry| {
                entry.key() != aggregate_id
                    && entry.value().status == SessionStatus::Open
                    && entry.value().spot.id() == target_spot_id
            });
            if let Some(entry) = conflict {
                return Err(AppError::Conflict(format!(
                    "spot {target_spot_id} already has an open session ({})",
                    entry.key()
                )));
            }
        }
        DomainEvent::SessionMerged { sources, .. } => {
            let target = store
                .sessions
                .get(aggregate_id)
                .ok_or_else(|| AppError::Validation("merge target not opened".into()))?;
            if target.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "merge target {aggregate_id} status {:?}",
                    target.status
                )));
            }
            drop(target);
            // Target must not appear in its own sources.
            if sources.iter().any(|s| s == aggregate_id) {
                return Err(AppError::Validation(format!(
                    "merge target {aggregate_id} cannot also be a source"
                )));
            }
            for src in sources {
                let src_state = store
                    .sessions
                    .get(src)
                    .ok_or_else(|| AppError::Validation(format!("source {src} not opened")))?;
                if src_state.status != SessionStatus::Open {
                    return Err(AppError::Conflict(format!(
                        "merge source {src} status {:?}",
                        src_state.status
                    )));
                }
            }
        }
        DomainEvent::SessionSplit { from_session, .. } => {
            let s = store
                .sessions
                .get(from_session)
                .ok_or_else(|| AppError::Validation("split source not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "split source {from_session} status {:?}",
                    s.status
                )));
            }
        }
        DomainEvent::OrderItemCancelled {
            order_id,
            item_index,
            ..
        } => {
            let o = store
                .orders
                .get(order_id)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() {
                return Err(AppError::Validation(format!(
                    "item_index {item_index} out of bounds (len {})",
                    o.items.len()
                )));
            }
            if o.items[*item_index].cancelled {
                return Err(AppError::Conflict("item already cancelled".into()));
            }
        }
        DomainEvent::OrderItemReturned {
            order_id,
            item_index,
            qty,
            ..
        } => {
            let o = store
                .orders
                .get(order_id)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() {
                return Err(AppError::Validation(format!(
                    "item_index {item_index} out of bounds (len {})",
                    o.items.len()
                )));
            }
            let it = &o.items[*item_index];
            let remaining = it.spec.qty - it.returned_qty;
            if *qty <= 0 || *qty > remaining {
                return Err(AppError::Validation(format!(
                    "return qty {qty} invalid (remaining {remaining})"
                )));
            }
        }
        DomainEvent::PaymentTaken { session_id, .. } => {
            // CONTRACT: payment_cmd MUST write PaymentTaken with aggregate_id == session_id.
            if store.payments.contains_key(session_id) {
                return Err(AppError::Conflict("session already paid".into()));
            }
            let s = store
                .sessions
                .get(session_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session status {:?}, cannot take payment",
                    s.status
                )));
            }
        }
        DomainEvent::OrderPlaced { session_id, .. } => {
            let s = store
                .sessions
                .get(session_id)
                .ok_or_else(|| AppError::Validation(format!("session {session_id} not opened")))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session {session_id} status {:?}, cannot place order",
                    s.status
                )));
            }
        }
        DomainEvent::SessionOpened { spot, .. } => {
            // Reject if any currently-Open session occupies the same spot.
            // Race-safety: CommandService takes a spot-keyed lock for SessionOpened
            // (in addition to the per-aggregate lock) so concurrent opens for the
            // same spot serialize through this check.
            let target_spot_id = spot.id();
            let conflict = store.sessions.iter().find(|entry| {
                entry.value().status == SessionStatus::Open
                    && entry.value().spot.id() == target_spot_id
            });
            if let Some(entry) = conflict {
                return Err(AppError::Conflict(format!(
                    "spot {target_spot_id} already has an open session ({})",
                    entry.key()
                )));
            }
        }
    }
    Ok(())
}
