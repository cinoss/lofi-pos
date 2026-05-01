use crate::domain::event::DomainEvent;
use crate::domain::spot::SpotRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    Open,
    Closed,
    Merged { into: String },
    Split,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub status: SessionStatus,
    /// Snapshot of the spot at session-open / transfer time. Self-contained;
    /// reproduces independently of the live `spot` master row.
    pub spot: SpotRef,
    pub opened_by: i64,
    /// Wall-clock timestamp (ms since epoch) when the SessionOpened event was
    /// appended. Surfaced so the UI can compute elapsed time for time-billed
    /// (room) sessions without a separate query.
    #[serde(default)]
    pub opened_at_ms: i64,
    pub customer_label: Option<String>,
    pub team: Option<String>,
    /// Order ids placed under this session, including any inherited from
    /// merged sources. Maintained by `domain::apply::apply` (not by `fold`,
    /// which only sees a single aggregate's events).
    #[serde(default)]
    pub order_ids: Vec<String>,
    /// True once a `PaymentTaken` event has been applied for this session.
    /// Surfaced so the UI can hide cancel/return controls without a separate
    /// query. Maintained by `domain::apply::apply`.
    #[serde(default)]
    pub payment_taken: bool,
}

/// Fold events into a single session's projection.
///
/// **Common case:** caller passes `events.list_for_aggregate(session_id)` —
/// events that were written WITH this aggregate_id. In this case
/// `SessionMerged`/`SessionSplit` branches are inert (the source-session
/// aggregate never sees these events) and the result reflects the events
/// THIS session emitted: opened, closed, transferred.
///
/// **Cross-aggregate case:** caller can also pass a pre-merged stream that
/// includes `SessionMerged`/`SessionSplit` events from OTHER aggregates
/// (e.g., the merge target's events) — in which case the source session
/// will be marked `Merged { into }` or `Split` once it appears in the
/// `sources` / `from_session` field. This is the only way to project
/// "this source was merged away" from the source session's perspective.
///
/// Returns None if no `SessionOpened` was seen for this `session_id`.
pub fn fold(session_id: &str, events: &[DomainEvent]) -> Option<SessionState> {
    let mut state: Option<SessionState> = None;
    for ev in events {
        match ev {
            DomainEvent::SessionOpened {
                spot,
                opened_by,
                customer_label,
                team,
            } => {
                state = Some(SessionState {
                    session_id: session_id.to_string(),
                    status: SessionStatus::Open,
                    spot: spot.clone(),
                    opened_by: *opened_by,
                    // `fold` operates on raw DomainEvents without row metadata;
                    // wall-clock `opened_at_ms` is captured by `apply` (which
                    // has the EventRow ts) and by warm_up (which replays via
                    // apply). Replays-from-fold therefore default to 0.
                    opened_at_ms: 0,
                    customer_label: customer_label.clone(),
                    team: team.clone(),
                    order_ids: Vec::new(),
                    payment_taken: false,
                });
            }
            DomainEvent::SessionClosed { .. } => {
                if let Some(s) = state.as_mut() {
                    s.status = SessionStatus::Closed;
                }
            }
            DomainEvent::SessionTransferred { from: _, to } => {
                if let Some(s) = state.as_mut() {
                    s.spot = to.clone();
                }
            }
            DomainEvent::SessionMerged {
                into_session,
                sources,
            } => {
                if let Some(s) = state.as_mut() {
                    if sources.iter().any(|src| src == &s.session_id) {
                        s.status = SessionStatus::Merged {
                            into: into_session.clone(),
                        };
                    }
                }
            }
            DomainEvent::SessionSplit { from_session, .. } => {
                if let Some(s) = state.as_mut() {
                    if from_session == &s.session_id {
                        s.status = SessionStatus::Split;
                    }
                }
            }
            _ => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> DomainEvent {
        DomainEvent::SessionOpened {
            spot: SpotRef::Room {
                id: 1,
                name: "R1".into(),
                hourly_rate: 50_000,
            },
            opened_by: 7,
            customer_label: Some("L".into()),
            team: None,
        }
    }

    #[test]
    fn no_events_yields_none() {
        assert!(fold("s", &[]).is_none());
    }

    #[test]
    fn opened_yields_open_state() {
        let s = fold("s", &[opened()]).unwrap();
        assert_eq!(s.status, SessionStatus::Open);
        assert!(s.spot.is_room());
        assert_eq!(s.spot.id(), 1);
        assert_eq!(s.opened_by, 7);
    }

    #[test]
    fn open_then_close() {
        let evs = vec![
            opened(),
            DomainEvent::SessionClosed {
                closed_by: 7,
                reason: None,
            },
        ];
        assert_eq!(fold("s", &evs).unwrap().status, SessionStatus::Closed);
    }

    #[test]
    fn transfer_updates_target() {
        let evs = vec![
            opened(),
            DomainEvent::SessionTransferred {
                from: SpotRef::Room {
                    id: 1,
                    name: "R1".into(),
                    hourly_rate: 50_000,
                },
                to: SpotRef::Table {
                    id: 5,
                    name: "T5".into(),
                    room_id: None,
                    room_name: None,
                },
            },
        ];
        let s = fold("s", &evs).unwrap();
        assert!(s.spot.is_table());
        assert_eq!(s.spot.id(), 5);
    }

    #[test]
    fn merge_marks_source_as_merged() {
        let evs = vec![
            opened(),
            DomainEvent::SessionMerged {
                into_session: "target".into(),
                sources: vec!["s".into()],
            },
        ];
        match fold("s", &evs).unwrap().status {
            SessionStatus::Merged { into } => assert_eq!(into, "target"),
            other => panic!("expected Merged, got {other:?}"),
        }
    }

    #[test]
    fn merge_source_status_via_cross_aggregate_event() {
        // Caller manually concatenates: source's SessionOpened + target's SessionMerged.
        let evs = vec![
            opened(),
            DomainEvent::SessionMerged {
                into_session: "target".into(),
                sources: vec!["s".into()],
            },
        ];
        match fold("s", &evs).unwrap().status {
            SessionStatus::Merged { into } => assert_eq!(into, "target"),
            other => panic!("expected Merged, got {other:?}"),
        }
    }

    #[test]
    fn split_marks_source_as_split() {
        let evs = vec![
            opened(),
            DomainEvent::SessionSplit {
                from_session: "s".into(),
                new_sessions: vec!["a".into(), "b".into()],
            },
        ];
        assert_eq!(fold("s", &evs).unwrap().status, SessionStatus::Split);
    }
}
