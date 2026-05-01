//! `apply(store, event, ctx)` — single-source-of-truth state transition.
//! Used by both `CommandService::execute` (write-time mutation) and
//! `AggregateStore::warm_up` (replay on startup). Eliminates fold-vs-apply drift.

use crate::domain::event::DomainEvent;
use crate::domain::order::{OrderItem, OrderState};
use crate::domain::payment::PaymentState;
use crate::domain::session::{SessionState, SessionStatus};
use crate::error::AppResult;
use crate::store::aggregate_store::AggregateStore;

pub struct ApplyCtx<'a> {
    pub aggregate_id: &'a str,
    /// Wall-clock timestamp (ms since epoch) of the event being applied.
    /// At write time this is the value the event row was stamped with; at
    /// warm_up time it is `EventRow.ts`. Currently consumed only to seed
    /// `SessionState.opened_at_ms` for time-billed UI.
    pub at_ms: i64,
}

pub fn apply(store: &AggregateStore, event: &DomainEvent, ctx: ApplyCtx<'_>) -> AppResult<()> {
    match event {
        DomainEvent::SessionOpened {
            spot,
            opened_by,
            customer_label,
            team,
        } => {
            store.sessions.insert(
                ctx.aggregate_id.to_string(),
                SessionState {
                    session_id: ctx.aggregate_id.to_string(),
                    status: SessionStatus::Open,
                    spot: spot.clone(),
                    opened_by: *opened_by,
                    opened_at_ms: ctx.at_ms,
                    customer_label: customer_label.clone(),
                    team: team.clone(),
                    order_ids: Vec::new(),
                    payment_taken: false,
                },
            );
        }
        DomainEvent::SessionClosed { .. } => {
            if let Some(mut s) = store.sessions.get_mut(ctx.aggregate_id) {
                s.status = SessionStatus::Closed;
            }
        }
        DomainEvent::SessionTransferred { from: _, to } => {
            if let Some(mut s) = store.sessions.get_mut(ctx.aggregate_id) {
                s.spot = to.clone();
            }
        }
        DomainEvent::SessionMerged {
            into_session,
            sources,
        } => {
            // Each source's state is removed from active sessions; its order_ids
            // are absorbed into the target. Source's "Merged" status only matters
            // for warm-up's correctness (we don't need it in memory at runtime,
            // because removal IS the merge).
            let mut absorbed: Vec<String> = Vec::new();
            for src in sources {
                if let Some((_, src_state)) = store.sessions.remove(src) {
                    absorbed.extend(src_state.order_ids);
                }
            }
            if let Some(mut target) = store.sessions.get_mut(into_session) {
                target.order_ids.extend(absorbed);
            }
        }
        DomainEvent::SessionSplit { from_session, .. } => {
            if let Some(mut s) = store.sessions.get_mut(from_session) {
                s.status = SessionStatus::Split;
            }
            // New sessions are created by separate SessionOpened events
            // emitted by the split command (caller's responsibility).
        }
        DomainEvent::OrderPlaced {
            session_id,
            order_id,
            items,
        } => {
            store.orders.insert(
                order_id.clone(),
                OrderState {
                    order_id: order_id.clone(),
                    session_id: session_id.clone(),
                    items: items
                        .iter()
                        .cloned()
                        .map(|spec| OrderItem {
                            spec,
                            cancelled: false,
                            returned_qty: 0,
                        })
                        .collect(),
                },
            );
            if let Some(mut s) = store.sessions.get_mut(session_id) {
                s.order_ids.push(order_id.clone());
            }
        }
        DomainEvent::OrderItemCancelled {
            order_id,
            item_index,
            ..
        } => {
            if let Some(mut o) = store.orders.get_mut(order_id) {
                if let Some(it) = o.items.get_mut(*item_index) {
                    it.cancelled = true;
                }
            }
        }
        DomainEvent::OrderItemReturned {
            order_id,
            item_index,
            qty,
            ..
        } => {
            if let Some(mut o) = store.orders.get_mut(order_id) {
                if let Some(it) = o.items.get_mut(*item_index) {
                    it.returned_qty += qty;
                }
            }
        }
        DomainEvent::PaymentTaken {
            session_id,
            subtotal,
            discount_pct,
            vat_pct,
            total,
            method,
        } => {
            store.payments.insert(
                session_id.clone(),
                PaymentState {
                    session_id: session_id.clone(),
                    subtotal: *subtotal,
                    discount_pct: *discount_pct,
                    vat_pct: *vat_pct,
                    total: *total,
                    method: method.clone(),
                },
            );
            if let Some(mut s) = store.sessions.get_mut(session_id) {
                s.payment_taken = true;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::event::{OrderItemSpec, Route};
    use crate::domain::spot::SpotRef;

    fn opened(opener: i64) -> DomainEvent {
        DomainEvent::SessionOpened {
            spot: SpotRef::Room {
                id: 1,
                name: "R1".into(),
                hourly_rate: 50_000,
            },
            opened_by: opener,
            customer_label: Some("VIP".into()),
            team: None,
        }
    }

    fn item(product_id: i64, qty: i64, unit_price: i64) -> OrderItemSpec {
        OrderItemSpec {
            product_id,
            product_name: format!("P{product_id}"),
            qty,
            unit_price,
            note: None,
            route: Route::Bar,
            recipe_snapshot: vec![],
        }
    }

    #[test]
    fn opened_inserts_session() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a", at_ms: 0 }).unwrap();
        let r = s.sessions.get("a").unwrap();
        assert_eq!(r.status, SessionStatus::Open);
        assert!(r.order_ids.is_empty());
    }

    #[test]
    fn opened_captures_at_ms() {
        // ApplyCtx.at_ms threads the event-row wall clock into SessionState so
        // the time-billed UI can compute elapsed minutes without a side query.
        let s = AggregateStore::new();
        apply(
            &s,
            &opened(1),
            ApplyCtx {
                aggregate_id: "a",
                at_ms: 1_700_000_000_000,
            },
        )
        .unwrap();
        assert_eq!(s.sessions.get("a").unwrap().opened_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn closed_marks_session() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a", at_ms: 0 }).unwrap();
        apply(
            &s,
            &DomainEvent::SessionClosed {
                closed_by: 1,
                reason: None,
            },
            ApplyCtx { aggregate_id: "a", at_ms: 0 },
        )
        .unwrap();
        assert_eq!(s.sessions.get("a").unwrap().status, SessionStatus::Closed);
    }

    #[test]
    fn order_placed_indexes_into_session() {
        let s = AggregateStore::new();
        apply(
            &s,
            &opened(1),
            ApplyCtx { aggregate_id: "sess", at_ms: 0 },
        )
        .unwrap();
        apply(
            &s,
            &DomainEvent::OrderPlaced {
                session_id: "sess".into(),
                order_id: "o1".into(),
                items: vec![item(1, 1, 100)],
            },
            ApplyCtx { aggregate_id: "o1", at_ms: 0 },
        )
        .unwrap();
        assert_eq!(s.sessions.get("sess").unwrap().order_ids, vec!["o1"]);
        assert!(s.orders.contains_key("o1"));
    }

    #[test]
    fn merge_absorbs_source_orders_and_removes_source() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "A", at_ms: 0 }).unwrap();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "B", at_ms: 0 }).unwrap();
        apply(
            &s,
            &DomainEvent::OrderPlaced {
                session_id: "A".into(),
                order_id: "oA".into(),
                items: vec![item(1, 1, 100)],
            },
            ApplyCtx { aggregate_id: "oA", at_ms: 0 },
        )
        .unwrap();
        apply(
            &s,
            &DomainEvent::OrderPlaced {
                session_id: "B".into(),
                order_id: "oB".into(),
                items: vec![item(2, 1, 200)],
            },
            ApplyCtx { aggregate_id: "oB", at_ms: 0 },
        )
        .unwrap();

        apply(
            &s,
            &DomainEvent::SessionMerged {
                into_session: "A".into(),
                sources: vec!["B".into()],
            },
            ApplyCtx { aggregate_id: "A", at_ms: 0 },
        )
        .unwrap();

        let a = s.sessions.get("A").unwrap();
        assert_eq!(a.order_ids, vec!["oA", "oB"]);
        assert!(
            s.sessions.get("B").is_none(),
            "source B removed from active sessions"
        );
    }

    #[test]
    fn cancel_marks_order_item() {
        let s = AggregateStore::new();
        apply(
            &s,
            &opened(1),
            ApplyCtx { aggregate_id: "sess", at_ms: 0 },
        )
        .unwrap();
        apply(
            &s,
            &DomainEvent::OrderPlaced {
                session_id: "sess".into(),
                order_id: "o".into(),
                items: vec![item(1, 1, 100), item(2, 1, 200)],
            },
            ApplyCtx { aggregate_id: "o", at_ms: 0 },
        )
        .unwrap();
        apply(
            &s,
            &DomainEvent::OrderItemCancelled {
                order_id: "o".into(),
                item_index: 1,
                reason: None,
            },
            ApplyCtx { aggregate_id: "o", at_ms: 0 },
        )
        .unwrap();
        let o = s.orders.get("o").unwrap();
        assert!(!o.items[0].cancelled);
        assert!(o.items[1].cancelled);
    }

    #[test]
    fn return_increments_returned_qty() {
        let s = AggregateStore::new();
        apply(
            &s,
            &opened(1),
            ApplyCtx { aggregate_id: "sess", at_ms: 0 },
        )
        .unwrap();
        apply(
            &s,
            &DomainEvent::OrderPlaced {
                session_id: "sess".into(),
                order_id: "o".into(),
                items: vec![item(1, 5, 100)],
            },
            ApplyCtx { aggregate_id: "o", at_ms: 0 },
        )
        .unwrap();
        apply(
            &s,
            &DomainEvent::OrderItemReturned {
                order_id: "o".into(),
                item_index: 0,
                qty: 2,
                reason: None,
            },
            ApplyCtx { aggregate_id: "o", at_ms: 0 },
        )
        .unwrap();
        assert_eq!(s.orders.get("o").unwrap().items[0].returned_qty, 2);
    }

    #[test]
    fn payment_taken_inserts_payment() {
        let s = AggregateStore::new();
        apply(
            &s,
            &DomainEvent::PaymentTaken {
                session_id: "sess".into(),
                subtotal: 100,
                discount_pct: 0,
                vat_pct: 8,
                total: 108,
                method: "cash".into(),
            },
            ApplyCtx { aggregate_id: "pay", at_ms: 0 },
        )
        .unwrap();
        let p = s.payments.get("sess").unwrap();
        assert_eq!(p.total, 108);
    }

    #[test]
    fn payment_taken_flips_session_payment_taken_flag() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "sess", at_ms: 0 }).unwrap();
        assert!(!s.sessions.get("sess").unwrap().payment_taken);
        apply(
            &s,
            &DomainEvent::PaymentTaken {
                session_id: "sess".into(),
                subtotal: 100,
                discount_pct: 0,
                vat_pct: 0,
                total: 100,
                method: "cash".into(),
            },
            ApplyCtx { aggregate_id: "pay", at_ms: 0 },
        )
        .unwrap();
        assert!(s.sessions.get("sess").unwrap().payment_taken);
    }

    #[test]
    fn transfer_updates_spot() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a", at_ms: 0 }).unwrap();
        apply(
            &s,
            &DomainEvent::SessionTransferred {
                from: SpotRef::Room {
                    id: 1,
                    name: "R1".into(),
                    hourly_rate: 50_000,
                },
                to: SpotRef::Table {
                    id: 7,
                    name: "T7".into(),
                    room_id: None,
                    room_name: None,
                },
            },
            ApplyCtx { aggregate_id: "a", at_ms: 0 },
        )
        .unwrap();
        let r = s.sessions.get("a").unwrap();
        assert!(r.spot.is_table());
        assert_eq!(r.spot.id(), 7);
    }

    #[test]
    fn split_marks_source_split() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a", at_ms: 0 }).unwrap();
        apply(
            &s,
            &DomainEvent::SessionSplit {
                from_session: "a".into(),
                new_sessions: vec!["b".into(), "c".into()],
            },
            ApplyCtx { aggregate_id: "a", at_ms: 0 },
        )
        .unwrap();
        assert_eq!(s.sessions.get("a").unwrap().status, SessionStatus::Split);
    }
}
