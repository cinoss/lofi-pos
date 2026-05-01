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
        DomainEvent::SessionTransferred { from, to } => {
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
            // No-op transfer (same spot before and after) is meaningless.
            if from.id() == to.id() {
                return Err(AppError::Validation(format!(
                    "transfer target spot {} is the same as current spot",
                    to.id()
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
            if sources.is_empty() {
                return Err(AppError::Validation("merge requires at least one source".into()));
            }
            // Target must not appear in its own sources.
            if sources.iter().any(|s| s == aggregate_id) {
                return Err(AppError::Validation(format!(
                    "merge target {aggregate_id} cannot also be a source"
                )));
            }
            // Sources must be distinct.
            let mut seen = std::collections::HashSet::with_capacity(sources.len());
            for src in sources {
                if !seen.insert(src) {
                    return Err(AppError::Validation(format!(
                        "merge source {src} listed more than once"
                    )));
                }
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
            if it.cancelled {
                return Err(AppError::Conflict(
                    "cannot return a cancelled item".into(),
                ));
            }
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
        DomainEvent::OrderPlaced {
            session_id, items, ..
        } => {
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
            if items.is_empty() {
                return Err(AppError::Validation(
                    "order must have at least one item".into(),
                ));
            }
            for (i, it) in items.iter().enumerate() {
                if it.qty <= 0 {
                    return Err(AppError::Validation(format!(
                        "item {i} qty {} must be positive",
                        it.qty
                    )));
                }
                if it.unit_price < 0 {
                    return Err(AppError::Validation(format!(
                        "item {i} unit_price {} must be non-negative",
                        it.unit_price
                    )));
                }
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
