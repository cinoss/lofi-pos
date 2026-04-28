use crate::domain::event::DomainEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentState {
    pub session_id: String,
    pub subtotal: i64,
    pub discount_pct: u32,
    pub vat_pct: u32,
    pub total: i64,
    pub method: String,
}

/// Fold returns the FIRST PaymentTaken for `session_id`. Subsequent payment
/// events are an invariant violation and the caller (event writer) must
/// reject them — this fold ignores them so projections stay deterministic.
pub fn fold(session_id: &str, events: &[DomainEvent]) -> Option<PaymentState> {
    for ev in events {
        if let DomainEvent::PaymentTaken {
            session_id: sid,
            subtotal,
            discount_pct,
            vat_pct,
            total,
            method,
        } = ev
        {
            if sid == session_id {
                return Some(PaymentState {
                    session_id: sid.clone(),
                    subtotal: *subtotal,
                    discount_pct: *discount_pct,
                    vat_pct: *vat_pct,
                    total: *total,
                    method: method.clone(),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_payment_yields_none() {
        assert!(fold("s", &[]).is_none());
    }

    #[test]
    fn first_payment_wins() {
        let evs = vec![
            DomainEvent::PaymentTaken {
                session_id: "s".into(),
                subtotal: 100,
                discount_pct: 0,
                vat_pct: 8,
                total: 108,
                method: "cash".into(),
            },
            DomainEvent::PaymentTaken {
                session_id: "s".into(),
                subtotal: 999,
                discount_pct: 50,
                vat_pct: 0,
                total: 500,
                method: "card".into(),
            },
        ];
        let p = fold("s", &evs).unwrap();
        assert_eq!(p.total, 108);
        assert_eq!(p.method, "cash");
    }

    #[test]
    fn other_session_ignored() {
        let evs = vec![DomainEvent::PaymentTaken {
            session_id: "other".into(),
            subtotal: 100,
            discount_pct: 0,
            vat_pct: 8,
            total: 108,
            method: "cash".into(),
        }];
        assert!(fold("s", &evs).is_none());
    }
}
