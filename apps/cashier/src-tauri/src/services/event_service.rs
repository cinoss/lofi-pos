use crate::business_day::business_day_of;
use crate::crypto::Kek;
use crate::domain::event::DomainEvent;
use crate::error::AppResult;
use crate::services::day_key;
use crate::store::events::{AppendEvent, EventStore};
use crate::store::master::Master;
use crate::time::Clock;
use chrono::DateTime;
use chrono::FixedOffset;
use chrono::Utc;
use std::sync::{Arc, Mutex};

// Note: `business_day_of` requires a `FixedOffset` parameter (Wave 1 fix). The venue's
// fixed timezone offset is supplied at construction; production wires it from the
// `business_day_tz_offset_seconds` setting.
pub struct EventService {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub kek: Arc<Kek>,
    pub clock: Arc<dyn Clock>,
    pub cutoff_hour: u32,
    pub tz: FixedOffset,
}

pub struct WriteCtx<'a> {
    pub aggregate_id: &'a str,
    pub actor_staff: Option<i64>,
    /// Plaintext display name for the actor, captured at write time. Stored
    /// alongside the event row so audit views never have to decrypt the
    /// payload nor join master.
    pub actor_name: Option<&'a str>,
    /// Authorizer staff id when an override PIN was used to satisfy the ACL.
    /// `None` on the normal Allow path. Recorded plaintext alongside the
    /// requester so audit views can show "X did Y, authorized by Z" without
    /// decrypting the payload.
    pub override_staff_id: Option<i64>,
    pub override_staff_name: Option<&'a str>,
    /// Override "now" for testability. Production = None (use clock).
    pub at: Option<DateTime<Utc>>,
}

impl EventService {
    pub fn write(&self, ctx: WriteCtx<'_>, ev: &DomainEvent) -> AppResult<i64> {
        let now = ctx.at.unwrap_or_else(|| self.clock.now());
        let ts = now.timestamp_millis();
        let day = business_day_of(now, self.tz, self.cutoff_hour);
        let dek = {
            let master = self.master.lock().unwrap();
            day_key::get_or_create(&master, &self.kek, &day)?
        };

        let payload = serde_json::to_vec(ev)
            .map_err(|e| crate::error::AppError::Internal(format!("serialize event: {e}")))?;
        // AAD binds ciphertext to (business_day, event_type, aggregate_id, key_id).
        // key_id is included so future key-rotation that decouples it from business_day
        // stays authenticated.
        let aad = format!(
            "{day}|{}|{}|{day}",
            ev.event_type().as_str(),
            ctx.aggregate_id
        );
        let blob = dek.encrypt(&payload, aad.as_bytes())?;

        self.events.append(AppendEvent {
            business_day: &day,
            ts,
            event_type: ev.event_type().as_str(),
            aggregate_id: ctx.aggregate_id,
            actor_staff: ctx.actor_staff,
            actor_name: ctx.actor_name,
            override_staff_id: ctx.override_staff_id,
            override_staff_name: ctx.override_staff_name,
            payload_enc: &blob,
            key_id: &day,
        })
    }

    pub fn read_decrypted(&self, row: &crate::store::events::EventRow) -> AppResult<DomainEvent> {
        let wrapped = {
            let master = self.master.lock().unwrap();
            master
                .get_day_key(&row.key_id)?
                .ok_or(crate::error::AppError::NotFound)?
        };
        let dek = self.kek.unwrap(&wrapped)?;
        // AAD binds ciphertext to (business_day, event_type, aggregate_id, key_id).
        // key_id is included so future key-rotation that decouples it from business_day
        // stays authenticated.
        let aad = format!(
            "{}|{}|{}|{}",
            row.business_day, row.event_type, row.aggregate_id, row.key_id
        );
        let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
        let ev: DomainEvent = serde_json::from_slice(&pt)
            .map_err(|e| crate::error::AppError::Internal(format!("deserialize event: {e}")))?;
        Ok(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Kek;
    use crate::domain::event::DomainEvent;
    use crate::store::events::EventStore;
    use crate::store::master::Master;
    use crate::time::test_support::MockClock;

    #[allow(clippy::type_complexity)]
    fn rig() -> (
        Arc<Mutex<Master>>,
        Arc<EventStore>,
        Arc<Kek>,
        Arc<MockClock>,
        FixedOffset,
    ) {
        let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
        let events = Arc::new(EventStore::open_in_memory().unwrap());
        let kek = Arc::new(Kek::new_random());
        let clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
        // Vietnam TZ +7 — the production case.
        let tz = FixedOffset::east_opt(7 * 3600).unwrap();
        (master, events, kek, clock, tz)
    }

    fn svc(
        master: Arc<Mutex<Master>>,
        events: Arc<EventStore>,
        kek: Arc<Kek>,
        clock: Arc<MockClock>,
        tz: FixedOffset,
    ) -> EventService {
        EventService {
            master,
            events,
            kek,
            clock,
            cutoff_hour: 11,
            tz,
        }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (master, events, kek, clock, tz) = rig();
        let writer = svc(
            master.clone(),
            events.clone(),
            kek.clone(),
            clock.clone(),
            tz,
        );

        let ev = DomainEvent::SessionOpened {
            spot: crate::domain::spot::SpotRef::Room {
                id: 1,
                name: "R1".into(),
                hourly_rate: 50_000,
            },
            opened_by: 7,
            customer_label: Some("L".into()),
            team: None,
        };
        let id = writer
            .write(
                WriteCtx {
                    aggregate_id: "sess-1",
                    actor_staff: Some(7),
                    actor_name: None,
                    override_staff_id: None,
                    override_staff_name: None,
                    at: None,
                },
                &ev,
            )
            .unwrap();
        assert!(id > 0);

        let rows = events.list_for_aggregate("sess-1").unwrap();
        assert_eq!(rows.len(), 1);
        let decoded = writer.read_decrypted(&rows[0]).unwrap();
        assert_eq!(decoded, ev);
    }

    #[test]
    fn cross_midnight_event_belongs_to_opening_day() {
        let (master, events, kek, clock, tz) = rig();
        let writer = svc(
            master.clone(),
            events.clone(),
            kek.clone(),
            clock.clone(),
            tz,
        );
        // UTC 2026-04-28 03:00 with Vietnam TZ (+7) is local 2026-04-28 10:00,
        // which is BEFORE the local 11:00 cutoff — so it still belongs to local
        // business day 2026-04-27. (Same expected result as the original UTC-only
        // version of the test, but for the timezone-correct reason.)
        let at = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 4, 28, 3, 0, 0).unwrap();
        let ev = DomainEvent::SessionClosed {
            closed_by: 1,
            reason: None,
        };
        writer
            .write(
                WriteCtx {
                    aggregate_id: "x",
                    actor_staff: None,
                    actor_name: None,
                    override_staff_id: None,
                    override_staff_name: None,
                    at: Some(at),
                },
                &ev,
            )
            .unwrap();
        assert_eq!(events.count_for_day("2026-04-27").unwrap(), 1);
        assert_eq!(events.count_for_day("2026-04-28").unwrap(), 0);
    }

    #[test]
    fn aad_tamper_aggregate_id_fails_decrypt() {
        let (master, events, kek, clock, tz) = rig();
        let writer = svc(
            master.clone(),
            events.clone(),
            kek.clone(),
            clock.clone(),
            tz,
        );
        writer
            .write(
                WriteCtx {
                    aggregate_id: "real",
                    actor_staff: None,
                    actor_name: None,
                    override_staff_id: None,
                    override_staff_name: None,
                    at: None,
                },
                &DomainEvent::SessionClosed {
                    closed_by: 1,
                    reason: None,
                },
            )
            .unwrap();
        let mut rows = events.list_for_aggregate("real").unwrap();
        rows[0].aggregate_id = "forged".into();
        assert!(writer.read_decrypted(&rows[0]).is_err());
    }

    #[test]
    fn aad_tamper_event_type_fails_decrypt() {
        let (master, events, kek, clock, tz) = rig();
        let writer = svc(
            master.clone(),
            events.clone(),
            kek.clone(),
            clock.clone(),
            tz,
        );
        writer
            .write(
                WriteCtx {
                    aggregate_id: "x",
                    actor_staff: None,
                    actor_name: None,
                    override_staff_id: None,
                    override_staff_name: None,
                    at: None,
                },
                &DomainEvent::SessionClosed {
                    closed_by: 1,
                    reason: None,
                },
            )
            .unwrap();
        let mut rows = events.list_for_aggregate("x").unwrap();
        rows[0].event_type = "OrderPlaced".into();
        assert!(writer.read_decrypted(&rows[0]).is_err());
    }

    #[test]
    fn aad_tamper_business_day_fails_decrypt() {
        // Note: mutating business_day ALSO mutates key_id semantics — read fails because
        // master has no day_key for the new business_day (NotFound), not because of GCM.
        // This still exercises the pipeline; documents that decoupling business_day/key_id
        // would be a real AAD failure.
        let (master, events, kek, clock, tz) = rig();
        let writer = svc(
            master.clone(),
            events.clone(),
            kek.clone(),
            clock.clone(),
            tz,
        );
        writer
            .write(
                WriteCtx {
                    aggregate_id: "x",
                    actor_staff: None,
                    actor_name: None,
                    override_staff_id: None,
                    override_staff_name: None,
                    at: None,
                },
                &DomainEvent::SessionClosed {
                    closed_by: 1,
                    reason: None,
                },
            )
            .unwrap();
        let mut rows = events.list_for_aggregate("x").unwrap();
        rows[0].business_day = "1999-01-01".into();
        assert!(writer.read_decrypted(&rows[0]).is_err());
    }
}
