#![allow(dead_code)]

use cashier_lib::domain::event::{OrderItemSpec, Route};
use cashier_lib::domain::spot::SpotRef;
use cashier_lib::http::broadcast::EventNotice;

/// Build a discardable broadcast channel for test rigs that don't care
/// about WS notifications. Receiver is dropped immediately so `send`
/// returns `Err(SendError)`, which CommandService swallows.
pub fn dummy_broadcast() -> tokio::sync::broadcast::Sender<EventNotice> {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    tx
}

pub fn room(id: i64) -> SpotRef {
    SpotRef::Room {
        id,
        name: format!("R{id}"),
        hourly_rate: 50_000,
    }
}

pub fn table(id: i64) -> SpotRef {
    SpotRef::Table {
        id,
        name: format!("T{id}"),
        room_id: None,
        room_name: None,
    }
}

pub fn item(product_id: i64, qty: i64, unit_price: i64) -> OrderItemSpec {
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
