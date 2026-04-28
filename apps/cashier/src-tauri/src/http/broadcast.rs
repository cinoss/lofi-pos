use serde::Serialize;

/// Discriminator for `EventNotice` payloads. Adding a new variant here
/// forces every emitter and matcher to be updated, so future kinds
/// (`projection.refreshed`, `settings.changed`) are compile-checked.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    EventAppended,
}

/// Notification fanned out to every WebSocket subscriber when the
/// CommandService produces a `WriteOutcome::Inserted`. The actual
/// event payload stays encrypted on disk; this notice is just enough
/// for clients to refetch the affected aggregate.
#[derive(Debug, Clone, Serialize)]
pub struct EventNotice {
    pub kind: NoticeKind,
    pub event_type: String,
    pub aggregate_id: String,
    pub ts: i64,
}

impl EventNotice {
    pub fn appended(
        event_type: impl Into<String>,
        aggregate_id: impl Into<String>,
        ts: i64,
    ) -> Self {
        Self {
            kind: NoticeKind::EventAppended,
            event_type: event_type.into(),
            aggregate_id: aggregate_id.into(),
            ts,
        }
    }
}
