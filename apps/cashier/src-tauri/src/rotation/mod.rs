//! UTC daily key rotation. Owns the lifecycle of the wrapped-DEK rows in
//! `master.dek` and is independent of the EOD pipeline (which now owns only
//! event-row deletion at the venue's local cutoff).
//!
//! See `crate::services::key_manager` for the actual rotation algorithm; this
//! module is just the tokio-task wrapper.

pub mod scheduler;
pub use scheduler::spawn;
