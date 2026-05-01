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
        ApplyCtx { aggregate_id: agg, at_ms: 0 },
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
        ApplyCtx { aggregate_id: order_id, at_ms: 0 },
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
        ApplyCtx { aggregate_id: "s", at_ms: 0 },
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
        ApplyCtx { aggregate_id: "s", at_ms: 0 },
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

#[test]
fn second_open_on_same_spot_rejected_while_first_open() {
    let store = AggregateStore::new();
    open_session(&store, "s1");

    let second = DomainEvent::SessionOpened {
        spot: room(1), // same spot id as the first
        opened_by: 2,
        customer_label: None,
        team: None,
    };
    let res = validate(&store, "s2", &second);
    assert!(res.is_err(), "expected conflict, got {res:?}");
}

#[test]
fn open_on_same_spot_allowed_after_first_closed() {
    let store = AggregateStore::new();
    open_session(&store, "s1");
    apply(
        &store,
        &DomainEvent::SessionClosed {
            closed_by: 1,
            reason: None,
        },
        ApplyCtx { aggregate_id: "s1", at_ms: 0 },
    )
    .unwrap();

    let second = DomainEvent::SessionOpened {
        spot: room(1),
        opened_by: 2,
        customer_label: None,
        team: None,
    };
    assert!(validate(&store, "s2", &second).is_ok());
}

#[test]
fn open_on_different_spot_allowed_while_first_open() {
    let store = AggregateStore::new();
    open_session(&store, "s1");
    let second = DomainEvent::SessionOpened {
        spot: room(2), // different spot
        opened_by: 2,
        customer_label: None,
        team: None,
    };
    assert!(validate(&store, "s2", &second).is_ok());
}

fn open_session_at(store: &AggregateStore, agg: &str, spot_id: i64) {
    apply(
        store,
        &DomainEvent::SessionOpened {
            spot: room(spot_id),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
        ApplyCtx { aggregate_id: agg, at_ms: 0 },
    )
    .unwrap();
}

#[test]
fn transfer_to_occupied_spot_rejected() {
    let store = AggregateStore::new();
    open_session_at(&store, "s1", 1);
    open_session_at(&store, "s2", 2);
    // Try to move s2 onto spot 1, which s1 still occupies
    let mv = DomainEvent::SessionTransferred {
        from: room(2),
        to: room(1),
    };
    let res = validate(&store, "s2", &mv);
    assert!(res.is_err(), "expected conflict, got {res:?}");
}

#[test]
fn transfer_to_idle_spot_allowed() {
    let store = AggregateStore::new();
    open_session_at(&store, "s1", 1);
    let mv = DomainEvent::SessionTransferred {
        from: room(1),
        to: room(2),
    };
    assert!(validate(&store, "s1", &mv).is_ok());
}

#[test]
fn order_placed_on_closed_session_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    apply(
        &store,
        &DomainEvent::SessionClosed {
            closed_by: 1,
            reason: None,
        },
        ApplyCtx { aggregate_id: "s", at_ms: 0 },
    )
    .unwrap();

    let place = DomainEvent::OrderPlaced {
        session_id: "s".into(),
        order_id: "o".into(),
        items: vec![item(1, 1, 1000)],
    };
    let res = validate(&store, "o", &place);
    assert!(res.is_err(), "expected conflict, got {res:?}");
}

#[test]
fn order_placed_on_unknown_session_rejected() {
    let store = AggregateStore::new();
    let place = DomainEvent::OrderPlaced {
        session_id: "ghost".into(),
        order_id: "o".into(),
        items: vec![item(1, 1, 1000)],
    };
    let res = validate(&store, "o", &place);
    assert!(res.is_err(), "expected validation error, got {res:?}");
}

#[test]
fn order_placed_on_open_session_allowed() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    let place = DomainEvent::OrderPlaced {
        session_id: "s".into(),
        order_id: "o".into(),
        items: vec![item(1, 1, 1000)],
    };
    assert!(validate(&store, "o", &place).is_ok());
}

#[test]
fn order_placed_with_empty_items_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    let place = DomainEvent::OrderPlaced {
        session_id: "s".into(),
        order_id: "o".into(),
        items: vec![],
    };
    assert!(validate(&store, "o", &place).is_err());
}

#[test]
fn order_placed_with_zero_qty_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    let place = DomainEvent::OrderPlaced {
        session_id: "s".into(),
        order_id: "o".into(),
        items: vec![item(1, 0, 1000)],
    };
    assert!(validate(&store, "o", &place).is_err());
}

#[test]
fn order_placed_with_negative_qty_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    let place = DomainEvent::OrderPlaced {
        session_id: "s".into(),
        order_id: "o".into(),
        items: vec![item(1, -2, 1000)],
    };
    assert!(validate(&store, "o", &place).is_err());
}

#[test]
fn order_placed_with_negative_unit_price_rejected() {
    let store = AggregateStore::new();
    open_session(&store, "s");
    let place = DomainEvent::OrderPlaced {
        session_id: "s".into(),
        order_id: "o".into(),
        items: vec![item(1, 1, -50)],
    };
    assert!(validate(&store, "o", &place).is_err());
}

#[test]
fn transfer_to_same_spot_rejected() {
    let store = AggregateStore::new();
    open_session_at(&store, "s1", 1);
    let mv = DomainEvent::SessionTransferred {
        from: room(1),
        to: room(1),
    };
    assert!(validate(&store, "s1", &mv).is_err());
}

#[test]
fn merge_with_empty_sources_rejected() {
    let store = AggregateStore::new();
    open_session_at(&store, "t", 1);
    let merge = DomainEvent::SessionMerged {
        into_session: "t".into(),
        sources: vec![],
    };
    assert!(validate(&store, "t", &merge).is_err());
}

#[test]
fn merge_with_duplicate_sources_rejected() {
    let store = AggregateStore::new();
    open_session_at(&store, "t", 1);
    open_session_at(&store, "x", 2);
    let merge = DomainEvent::SessionMerged {
        into_session: "t".into(),
        sources: vec!["x".into(), "x".into()],
    };
    assert!(validate(&store, "t", &merge).is_err());
}

#[test]
fn return_cancelled_item_rejected() {
    let store = AggregateStore::new();
    place_order(&store, "s", "o", 2);
    apply(
        &store,
        &DomainEvent::OrderItemCancelled {
            order_id: "o".into(),
            item_index: 0,
            reason: None,
        },
        ApplyCtx { aggregate_id: "o", at_ms: 0 },
    )
    .unwrap();
    let ret = DomainEvent::OrderItemReturned {
        order_id: "o".into(),
        item_index: 0,
        qty: 1,
        reason: None,
    };
    assert!(validate(&store, "o", &ret).is_err());
}

#[test]
fn merge_with_target_in_sources_rejected() {
    let store = AggregateStore::new();
    open_session_at(&store, "t", 1);
    open_session_at(&store, "x", 2);
    let merge = DomainEvent::SessionMerged {
        into_session: "t".into(),
        sources: vec!["t".into(), "x".into()], // target appears in sources
    };
    let res = validate(&store, "t", &merge);
    assert!(res.is_err(), "expected validation error, got {res:?}");
}
