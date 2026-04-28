//! Build the JSON dump of all events for a closed business day.
//!
//! v1 emits the decrypted event payloads grouped by class (orders / payments
//! / sessions). Inventory deltas and per-method payment breakdowns are out of
//! scope per Plan F.

use crate::app_state::AppState;
use crate::domain::event::DomainEvent;
use crate::error::AppResult;
use serde::Serialize;
use serde_json::Value;

/// EOD report payload. Stored as a JSON string in `daily_report.order_summary_json`
/// AND written verbatim to `<reports_dir>/YYYY-MM-DD.json`.
#[derive(Debug, Serialize)]
pub struct Report {
    pub business_day: String,
    pub generated_at: i64,
    pub orders: Vec<Value>,
    pub payments: Vec<Value>,
    pub sessions: Vec<Value>,
}

/// Decrypt every event for `business_day` and bucket them by class.
///
/// Reads the event log via [`crate::store::events::EventStore::list_for_day`]
/// (which already filters on the `business_day` column) and decrypts each row
/// through the canonical [`crate::services::event_service::EventService`]
/// living inside [`crate::services::command_service::CommandService`].
pub fn build_report(state: &AppState, business_day: &str) -> AppResult<Report> {
    let rows = state.events.list_for_day(business_day)?;
    let mut orders = Vec::new();
    let mut payments = Vec::new();
    let mut sessions = Vec::new();

    for row in &rows {
        let ev = state.commands.event_service.read_decrypted(row)?;
        let v = serde_json::to_value(&ev)
            .map_err(|e| crate::error::AppError::Internal(format!("event to_value: {e}")))?;
        match ev {
            DomainEvent::OrderPlaced { .. }
            | DomainEvent::OrderItemCancelled { .. }
            | DomainEvent::OrderItemReturned { .. } => orders.push(v),
            DomainEvent::PaymentTaken { .. } => payments.push(v),
            DomainEvent::SessionOpened { .. }
            | DomainEvent::SessionClosed { .. }
            | DomainEvent::SessionTransferred { .. }
            | DomainEvent::SessionMerged { .. }
            | DomainEvent::SessionSplit { .. } => sessions.push(v),
        }
    }

    Ok(Report {
        business_day: business_day.to_string(),
        generated_at: state.clock.now_ms(),
        orders,
        payments,
        sessions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eod::test_support::{place_test_order, seed_app_state_at, take_test_payment};

    #[test]
    fn builder_dumps_orders_for_day() {
        // Frozen clock at 2026-04-27 07:00 UTC = 14:00 +07 → business day 2026-04-27.
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        // place_test_order opens a session AND places one order: 1 OrderPlaced.
        place_test_order(&rig);
        // take_test_payment runs open+order+pay+close: another OrderPlaced + PaymentTaken + SessionClosed.
        take_test_payment(&rig);
        let report = build_report(&rig.state, "2026-04-27").unwrap();
        assert_eq!(report.business_day, "2026-04-27");
        assert_eq!(report.orders.len(), 2);
        assert_eq!(report.payments.len(), 1);
        // 2 SessionOpened (one per helper) + 1 SessionClosed (only take_test_payment closes).
        assert_eq!(report.sessions.len(), 3);
    }

    #[test]
    fn empty_day_yields_empty_report() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        let report = build_report(&rig.state, "2026-04-27").unwrap();
        assert_eq!(report.orders.len(), 0);
        assert_eq!(report.payments.len(), 0);
        assert_eq!(report.sessions.len(), 0);
    }
}
