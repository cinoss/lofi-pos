use crate::domain::event::{DomainEvent, OrderItemSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderItem {
    pub spec: OrderItemSpec,
    pub cancelled: bool,
    pub returned_qty: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderState {
    pub order_id: String,
    pub session_id: String,
    pub items: Vec<OrderItem>,
}

impl OrderState {
    pub fn live_subtotal(&self) -> i64 {
        self.items
            .iter()
            .filter(|i| !i.cancelled)
            .map(|i| {
                debug_assert!(
                    i.spec.qty >= i.returned_qty,
                    "returned_qty ({}) exceeds qty ({}) for order {}",
                    i.returned_qty,
                    i.spec.qty,
                    self.order_id
                );
                let net_qty = (i.spec.qty - i.returned_qty).max(0);
                net_qty * i.spec.unit_price
            })
            .sum()
    }
}

/// Fold events tagged with this order_id (already filtered by caller) into state.
/// Returns None if no OrderPlaced.
pub fn fold(order_id: &str, events: &[DomainEvent]) -> Option<OrderState> {
    let mut state: Option<OrderState> = None;
    for ev in events {
        match ev {
            DomainEvent::OrderPlaced {
                session_id,
                order_id: oid,
                items,
            } if oid == order_id => {
                state = Some(OrderState {
                    order_id: order_id.into(),
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
                });
            }
            DomainEvent::OrderItemCancelled {
                order_id: oid,
                item_index,
                ..
            } if oid == order_id => {
                debug_assert!(
                    state.as_ref().is_some_and(|s| *item_index < s.items.len()),
                    "OrderItemCancelled item_index {} out of bounds",
                    item_index
                );
                if let Some(s) = state.as_mut() {
                    if let Some(it) = s.items.get_mut(*item_index) {
                        it.cancelled = true;
                    }
                }
            }
            DomainEvent::OrderItemReturned {
                order_id: oid,
                item_index,
                qty,
                ..
            } if oid == order_id => {
                debug_assert!(
                    state.as_ref().is_some_and(|s| *item_index < s.items.len()),
                    "OrderItemReturned item_index {} out of bounds",
                    item_index
                );
                if let Some(s) = state.as_mut() {
                    if let Some(it) = s.items.get_mut(*item_index) {
                        it.returned_qty += qty;
                        debug_assert!(
                            it.returned_qty <= it.spec.qty,
                            "OrderItemReturned brings returned_qty ({}) above qty ({})",
                            it.returned_qty,
                            it.spec.qty
                        );
                    }
                }
            }
            _ => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed() -> DomainEvent {
        use crate::domain::event::Route;
        DomainEvent::OrderPlaced {
            session_id: "s".into(),
            order_id: "o".into(),
            items: vec![
                OrderItemSpec {
                    product_id: 1,
                    product_name: "P1".into(),
                    qty: 2,
                    unit_price: 50_000,
                    note: None,
                    route: Route::Bar,
                    recipe_snapshot: vec![],
                },
                OrderItemSpec {
                    product_id: 2,
                    product_name: "P2".into(),
                    qty: 1,
                    unit_price: 100_000,
                    note: None,
                    route: Route::Bar,
                    recipe_snapshot: vec![],
                },
            ],
        }
    }

    #[test]
    fn placed_yields_state_with_items() {
        let s = fold("o", &[placed()]).unwrap();
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.live_subtotal(), 2 * 50_000 + 100_000);
    }

    #[test]
    fn cancel_excludes_item() {
        let evs = vec![
            placed(),
            DomainEvent::OrderItemCancelled {
                order_id: "o".into(),
                item_index: 1,
                reason: None,
            },
        ];
        let s = fold("o", &evs).unwrap();
        assert!(!s.items[0].cancelled);
        assert!(s.items[1].cancelled);
        assert_eq!(s.live_subtotal(), 2 * 50_000);
    }

    #[test]
    fn return_reduces_subtotal() {
        let evs = vec![
            placed(),
            DomainEvent::OrderItemReturned {
                order_id: "o".into(),
                item_index: 0,
                qty: 1,
                reason: None,
            },
        ];
        let s = fold("o", &evs).unwrap();
        assert_eq!(s.items[0].returned_qty, 1);
        assert_eq!(s.live_subtotal(), 50_000 + 100_000);
    }

    #[test]
    fn unrelated_order_events_ignored() {
        let evs = vec![
            placed(),
            DomainEvent::OrderItemCancelled {
                order_id: "different".into(),
                item_index: 0,
                reason: None,
            },
        ];
        let s = fold("o", &evs).unwrap();
        assert!(!s.items[0].cancelled);
    }
}
