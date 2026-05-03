use crate::domain::spot::SpotRef;
use serde::{Deserialize, Serialize};

/// Top-level discriminator for the `type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    SessionOpened,
    SessionClosed,
    SessionTransferred,
    SessionMerged,
    SessionSplit,
    OrderPlaced,
    OrderItemCancelled,
    OrderItemReturned,
    PaymentTaken,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::SessionOpened => "SessionOpened",
            EventType::SessionClosed => "SessionClosed",
            EventType::SessionTransferred => "SessionTransferred",
            EventType::SessionMerged => "SessionMerged",
            EventType::SessionSplit => "SessionSplit",
            EventType::OrderPlaced => "OrderPlaced",
            EventType::OrderItemCancelled => "OrderItemCancelled",
            EventType::OrderItemReturned => "OrderItemReturned",
            EventType::PaymentTaken => "PaymentTaken",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeIngredientSnapshot {
    pub ingredient_id: i64,
    pub ingredient_name: String,
    pub qty: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    Kitchen,
    Bar,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderItemSpec {
    pub product_id: i64,
    pub product_name: String,
    pub qty: i64,
    pub unit_price: i64, // VND
    pub note: Option<String>,
    pub route: Route,
    pub recipe_snapshot: Vec<RecipeIngredientSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum DomainEvent {
    SessionOpened {
        spot: SpotRef,
        opened_by: i64,
        customer_label: Option<String>,
        team: Option<String>,
    },
    SessionClosed {
        closed_by: i64,
        reason: Option<String>,
    },
    SessionTransferred {
        from: SpotRef,
        to: SpotRef,
    },
    SessionMerged {
        into_session: String,
        sources: Vec<String>,
    },
    SessionSplit {
        from_session: String,
        new_sessions: Vec<String>,
    },
    OrderPlaced {
        session_id: String,
        order_id: String,
        items: Vec<OrderItemSpec>,
    },
    OrderItemCancelled {
        order_id: String,
        item_index: usize,
        reason: Option<String>,
    },
    OrderItemReturned {
        order_id: String,
        item_index: usize,
        qty: i64,
        reason: Option<String>,
    },
    PaymentTaken {
        session_id: String,
        subtotal: i64,
        discount_pct: u32,
        vat_pct: u32,
        total: i64,
        method: String,
    },
}

impl DomainEvent {
    pub fn event_type(&self) -> EventType {
        match self {
            DomainEvent::SessionOpened { .. } => EventType::SessionOpened,
            DomainEvent::SessionClosed { .. } => EventType::SessionClosed,
            DomainEvent::SessionTransferred { .. } => EventType::SessionTransferred,
            DomainEvent::SessionMerged { .. } => EventType::SessionMerged,
            DomainEvent::SessionSplit { .. } => EventType::SessionSplit,
            DomainEvent::OrderPlaced { .. } => EventType::OrderPlaced,
            DomainEvent::OrderItemCancelled { .. } => EventType::OrderItemCancelled,
            DomainEvent::OrderItemReturned { .. } => EventType::OrderItemReturned,
            DomainEvent::PaymentTaken { .. } => EventType::PaymentTaken,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_serialization() {
        let e = DomainEvent::SessionOpened {
            spot: SpotRef::Room {
                id: 1,
                name: "VIP-1".into(),
                billing: crate::domain::spot::RoomBilling { hourly_rate: 100_000, bucket_minutes: 1, included_minutes: 0, min_charge: 0 },
            },
            opened_by: 42,
            customer_label: Some("VIP1".into()),
            team: Some("A".into()),
        };
        let s = serde_json::to_string(&e).unwrap();
        let d: DomainEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(e, d);
    }

    #[test]
    fn event_type_strings_stable() {
        assert_eq!(EventType::SessionOpened.as_str(), "SessionOpened");
        assert_eq!(EventType::OrderItemCancelled.as_str(), "OrderItemCancelled");
    }
}
