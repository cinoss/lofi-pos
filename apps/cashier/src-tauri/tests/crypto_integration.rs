use cashier_lib::crypto::{Dek, Kek};

#[test]
fn full_kek_dek_event_payload_flow() {
    let kek = Kek::new_random();
    let dek = Dek::new_random();
    let wrapped = kek.wrap(&dek).unwrap();

    // Simulate persistence: drop dek, re-derive from wrapped via kek
    drop(dek);
    let dek2 = kek.unwrap(&wrapped).unwrap();

    let payload = serde_json::json!({"type":"OrderPlaced","items":[{"sku":"BIA-1","qty":2}]});
    let bytes = serde_json::to_vec(&payload).unwrap();
    let blob = dek2.encrypt(&bytes, b"event:1").unwrap();
    let pt = dek2.decrypt(&blob, b"event:1").unwrap();
    assert_eq!(pt, bytes);
}
