mod common;

use cashier_lib::domain::apply::{apply, ApplyCtx};
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::services::validation::validate;
use cashier_lib::store::aggregate_store::AggregateStore;
use common::{item, room};

fn open_session(store: &AggregateStore, agg: &str) {
    apply(
        store,
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
        ApplyCtx { aggregate_id: agg },
    )
    .unwrap();
}

fn place_order(store: &AggregateStore, session_id: &str, order_id: &str, qty: i64) {
    apply(
        store,
        &DomainEvent::OrderPlaced {
            session_id: session_id.into(),
            order_id: order_id.into(),
            items: vec![item(1, qty, 1000)],
        },
        ApplyCtx {
            aggregate_id: order_id,
        },
    )
    .unwrap();
}

#[test]
fn double_close_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    apply(
        &store,
        &DomainEvent::SessionClosed {
            closed_by: 1,
            reason: None,
        },
        ApplyCtx { aggregate_id: "s" },
    )
    .unwrap();

    let close_again = DomainEvent::SessionClosed {
        closed_by: 1,
        reason: None,
    };
    assert!(validate(&store, "s", &close_again).is_err());
}

#[test]
fn return_more_than_remaining_rejected() {
    let store = AggregateStore::new();
    place_order(&store, "s", "o", 2);

    let bad_return = DomainEvent::OrderItemReturned {
        order_id: "o".into(),
        item_index: 0,
        qty: 5,
        reason: None,
    };
    assert!(validate(&store, "o", &bad_return).is_err());

    let good_return = DomainEvent::OrderItemReturned {
        order_id: "o".into(),
        item_index: 0,
        qty: 1,
        reason: None,
    };
    assert!(validate(&store, "o", &good_return).is_ok());
}

#[test]
fn duplicate_payment_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    apply(
        &store,
        &DomainEvent::PaymentTaken {
            session_id: "s".into(),
            subtotal: 100,
            discount_pct: 0,
            vat_pct: 8,
            total: 108,
            method: "cash".into(),
        },
        ApplyCtx { aggregate_id: "s" },
    )
    .unwrap();

    let dup = DomainEvent::PaymentTaken {
        session_id: "s".into(),
        subtotal: 999,
        discount_pct: 0,
        vat_pct: 0,
        total: 999,
        method: "card".into(),
    };
    assert!(validate(&store, "s", &dup).is_err());
}

#[test]
fn cancel_item_oob_rejected() {
    let store = AggregateStore::new();
    place_order(&store, "s", "o", 1);
    let oob = DomainEvent::OrderItemCancelled {
        order_id: "o".into(),
        item_index: 99,
        reason: None,
    };
    assert!(validate(&store, "o", &oob).is_err());
}

#[test]
fn return_item_oob_rejected() {
    let store = AggregateStore::new();
    place_order(&store, "s", "o", 1);
    let oob = DomainEvent::OrderItemReturned {
        order_id: "o".into(),
        item_index: 99,
        qty: 1,
        reason: None,
    };
    assert!(validate(&store, "o", &oob).is_err());
}
