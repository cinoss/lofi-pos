//! Print enqueue helper. Side-effect after a domain event has been
//! durably appended + broadcast. Inserts a row into the persistent
//! `print_queue` in master.db; the background worker (see
//! `crate::bouncer::print_queue`) drains FIFO and POSTs to the bouncer.

use crate::store::master::Master;
use serde_json::Value;
use std::sync::Mutex;

/// Enqueue a print job. Failures are logged but not propagated — printing
/// must never block order placement or payment.
pub fn enqueue(master: &Mutex<Master>, kind: &str, payload: &Value, now_ms: i64) {
    let payload_str = payload.to_string();
    let res = {
        let m = master.lock().unwrap();
        m.enqueue_print(kind, &payload_str, None, now_ms)
    };
    if let Err(e) = res {
        tracing::error!(?e, kind, "enqueue print failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enqueue_inserts_row() {
        let master = Mutex::new(Master::open_in_memory().unwrap());
        enqueue(&master, "kitchen", &json!({"x": 1}), 1000);
        let job = master.lock().unwrap().next_print_job(2000).unwrap();
        let job = job.expect("job present");
        assert_eq!(job.kind, "kitchen");
        assert!(job.payload_json.contains("\"x\":1"));
    }
}
