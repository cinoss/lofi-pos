//! End-of-day pipeline.
//!
//! - [`business_day`] — cutoff/tz arithmetic in `(ms, Cfg)` form.
//! - [`builder`] — assemble the JSON report for a closed business day.
//! - [`runner`] — single transactional EOD run (report + crypto-shred + prune).
//! - [`scheduler`] — tokio task that runs `run_eod` at each cutoff and on
//!   startup catches up any missed days.

pub mod builder;
pub mod business_day;
pub mod runner;
pub mod scheduler;
pub mod test_support;

pub use business_day::{business_day_for, days_between, next_cutoff_ms, Cfg};
