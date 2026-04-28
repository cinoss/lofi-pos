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
        DomainEvent::SessionTransferred { .. } => {
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
        // SessionOpened / OrderPlaced / catalog events have no pre-state invariants
        // to enforce here; their command handlers carry richer per-input checks.
        _ => {}
    }
    Ok(())
}
