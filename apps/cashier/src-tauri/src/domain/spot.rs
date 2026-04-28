use serde::{Deserialize, Serialize};

/// Snapshot of a spot at session-open / transfer time. Self-contained — does
/// not depend on the live `spot` master row, so historical reports reproduce
/// regardless of subsequent renames or deletions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SpotRef {
    Room {
        id: i64,
        name: String,
        hourly_rate: i64,
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
