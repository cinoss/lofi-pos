//! Synchronous "printer" stub. Plan-F: side-effect after a domain event is
//! durably appended + broadcast. v1 just emits a single grep-friendly line to
//! stdout; a real printer router can later replace `print` without touching
//! call sites.

use serde_json::Value;

/// Write a single compact `[print]` line to stdout for the given kind/payload.
pub fn print(kind: &str, payload: &Value) {
    println!("{}", format_print_line(kind, payload));
}

pub(crate) fn format_print_line(kind: &str, payload: &Value) -> String {
    // Compact, single-line, easy to grep. `Display` for serde_json::Value is
    // already the compact form (no pretty-printing, no trailing newline).
    format!("[print] kind={kind} payload={payload}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn print_emits_kind_and_compact_json() {
        let v = json!({"spot": "Table 1", "items": [{"name":"Coke","qty":1}]});
        let line = format_print_line("order_ticket", &v);
        assert!(line.starts_with("[print] kind=order_ticket "));
        assert!(line.contains("\"spot\":\"Table 1\""));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn print_handles_empty_object() {
        let line = format_print_line("session_closed", &json!({}));
        assert_eq!(line, "[print] kind=session_closed payload={}");
    }
}
