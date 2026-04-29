use cashier_lib::crypto::Dek;

#[test]
fn dek_roundtrip_event_payload() {
    let dek = Dek::new_random();
    let payload = serde_json::json!({"type":"OrderPlaced","items":[{"sku":"BIA-1","qty":2}]});
    let bytes = serde_json::to_vec(&payload).unwrap();
    let blob = dek.encrypt(&bytes, b"event:1").unwrap();
    let pt = dek.decrypt(&blob, b"event:1").unwrap();
    assert_eq!(pt, bytes);
}

#[test]
fn dek_from_bytes_then_decrypt() {
    let bytes = [7u8; 32];
    let dek = Dek::from_bytes(&bytes).unwrap();
    let blob = dek.encrypt(b"hello", b"aad").unwrap();
    let dek2 = Dek::from_bytes(&bytes).unwrap();
    assert_eq!(dek2.decrypt(&blob, b"aad").unwrap(), b"hello");
}
