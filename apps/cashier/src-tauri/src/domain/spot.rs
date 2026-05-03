use serde::{Deserialize, Serialize};

/// Snapshot of a room's billing policy. Captured into `SpotRef::Room` at
/// session-open / transfer time so historical sessions bill against the
/// policy that was in effect even after admin edits.
///
/// Fields:
/// - `hourly_rate`: VND per hour, applied to overage past `included_minutes`.
/// - `bucket_minutes`: minimum overage granularity (1 = per minute).
/// - `included_minutes`: minutes covered by `min_charge` at no extra cost.
/// - `min_charge`: VND minimum for the included period.
///
/// Defaults for a new spot are pure per-minute billing
/// (`bucket_minutes=1, included_minutes=0, min_charge=0`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomBilling {
    pub hourly_rate: i64,
    pub bucket_minutes: u32,
    pub included_minutes: u32,
    pub min_charge: i64,
}

impl Default for RoomBilling {
    fn default() -> Self {
        Self {
            hourly_rate: 0,
            bucket_minutes: 1,
            included_minutes: 0,
            min_charge: 0,
        }
    }
}

/// Snapshot of a spot at session-open / transfer time. Self-contained — does
/// not depend on the live `spot` master row, so historical reports reproduce
/// regardless of subsequent renames or deletions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SpotRef {
    Room {
        id: i64,
        name: String,
        /// Snapshotted billing policy. `#[serde(default)]` keeps decoding
        /// stable for any pre-existing event payloads that lack the field
        /// (pre-prod, but cheap belt-and-braces).
        #[serde(default)]
        billing: RoomBilling,
    },
    Table {
        id: i64,
        name: String,
        room_id: Option<i64>,
        room_name: Option<String>,
    },
}

impl SpotRef {
    pub fn id(&self) -> i64 {
        match self {
            SpotRef::Room { id, .. } | SpotRef::Table { id, .. } => *id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            SpotRef::Room { name, .. } | SpotRef::Table { name, .. } => name,
        }
    }
    pub fn is_room(&self) -> bool {
        matches!(self, SpotRef::Room { .. })
    }
    pub fn is_table(&self) -> bool {
        matches!(self, SpotRef::Table { .. })
    }
}
