//! Runtime in-memory aggregate state. Disk events.db is the durable log
//! and crash-recovery source; this struct holds the live projection that
//! commands read and mutate.

use crate::domain::order::OrderState;
use crate::domain::payment::PaymentState;
use crate::domain::session::SessionState;
use dashmap::DashMap;

pub struct AggregateStore {
    pub sessions: DashMap<String, SessionState>,
    pub orders: DashMap<String, OrderState>,
    pub payments: DashMap<String, PaymentState>,
    /// Idempotency cache (key -> result_json). Mirrored to master.db for restart.
    pub idem: DashMap<String, String>,
}

impl AggregateStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            orders: DashMap::new(),
            payments: DashMap::new(),
            idem: DashMap::new(),
        }
    }
}

impl Default for AggregateStore {
    fn default() -> Self {
        Self::new()
    }
}

use crate::domain::apply::{apply, ApplyCtx};
use crate::domain::event::DomainEvent;
use crate::error::AppResult;
use crate::services::key_manager::KeyManager;
use crate::store::events::{EventRow, EventStore};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct WarmUpReport {
    pub aggregates_replayed: usize,
    pub events_replayed: usize,
    pub aggregates_dropped: Vec<(String, String)>,
}

impl AggregateStore {
    /// Restore in-memory state from durable storage.
    ///
    /// Hole-tolerant: if any event in an aggregate fails to decrypt, OR the
    /// per-aggregate `agg_seq` column has a gap, the entire aggregate is
    /// dropped (logged with reason). Surviving aggregates are replayed in
    /// global (ts, id) order so cross-aggregate causality is preserved.
    pub fn warm_up(
        &self,
        events: &EventStore,
        key_manager: &KeyManager,
    ) -> AppResult<WarmUpReport> {
        let rows = events.list_all()?;
        let mut by_agg: HashMap<String, Vec<(EventRow, AppResult<DomainEvent>)>> = HashMap::new();
        for row in rows {
            let decrypted = decrypt_row(key_manager, &row);
            by_agg
                .entry(row.aggregate_id.clone())
                .or_default()
                .push((row, decrypted));
        }

        let mut to_apply: Vec<(EventRow, DomainEvent)> = Vec::new();
        let mut aggregates_dropped: Vec<(String, String)> = Vec::new();
        let mut aggregates_replayed = 0usize;

        for (aggregate_id, mut events_for_agg) in by_agg {
            events_for_agg.sort_by_key(|(row, _)| row.agg_seq);

            let seq_ok = events_for_agg
                .iter()
                .enumerate()
                .all(|(i, (row, _))| row.agg_seq == (i as i64) + 1);
            let all_decrypted = events_for_agg.iter().all(|(_, r)| r.is_ok());

            if !seq_ok || !all_decrypted {
                let reason = if !seq_ok {
                    "sequence gap".to_string()
                } else {
                    let first_err = events_for_agg
                        .iter()
                        .find_map(|(_, r)| r.as_ref().err())
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "decrypt failed".into());
                    format!("decrypt failed: {first_err}")
                };
                tracing::warn!(
                    aggregate_id = %aggregate_id,
                    reason = %reason,
                    "warm-up dropping aggregate"
                );
                aggregates_dropped.push((aggregate_id, reason));
                continue;
            }

            aggregates_replayed += 1;
            for (row, decrypted) in events_for_agg {
                to_apply.push((row, decrypted.unwrap()));
            }
        }

        to_apply.sort_by_key(|(row, _)| (row.ts, row.id));
        let events_replayed = to_apply.len();
        for (row, ev) in to_apply {
            apply(
                self,
                &ev,
                ApplyCtx {
                    aggregate_id: &row.aggregate_id,
                    at_ms: row.ts,
                },
            )?;
        }

        Ok(WarmUpReport {
            aggregates_replayed,
            events_replayed,
            aggregates_dropped,
        })
    }
}

fn decrypt_row(km: &KeyManager, row: &EventRow) -> AppResult<DomainEvent> {
    let dek = km.dek_for(&row.seed_id, &row.business_day)?;
    let aad = format!(
        "{}|{}|{}|{}",
        row.business_day, row.event_type, row.aggregate_id, row.seed_id
    );
    let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
    let ev: DomainEvent = serde_json::from_slice(&pt)
        .map_err(|e| crate::error::AppError::Internal(format!("warm_up deserialize: {e}")))?;
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bouncer::seed_cache::SeedCache;
    use crate::services::event_service::{EventService, WriteCtx};
    use crate::time::test_support::MockClock;
    use crate::time::Clock;
    use chrono::FixedOffset;
    use std::sync::Arc;

    fn cache() -> Arc<SeedCache> {
        Arc::new(SeedCache::from_seeds(
            "test",
            vec![("test".into(), [7u8; 32])],
        ))
    }

    fn rig() -> (Arc<EventStore>, Arc<KeyManager>, EventService) {
        let events = Arc::new(EventStore::open_in_memory().unwrap());
        let km = Arc::new(KeyManager::new(cache()));
        let clock: Arc<dyn Clock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
        let svc = EventService {
            events: events.clone(),
            key_manager: km.clone(),
            clock,
            cutoff_hour: 11,
            tz: FixedOffset::east_opt(7 * 3600).unwrap(),
        };
        (events, km, svc)
    }

    fn open(svc: &EventService, sid: &str) {
        let ev = DomainEvent::SessionOpened {
            spot: crate::domain::spot::SpotRef::Room {
                id: 1,
                name: "R1".into(),
                billing: crate::domain::spot::RoomBilling { hourly_rate: 50_000, bucket_minutes: 1, included_minutes: 0, min_charge: 0 },
            },
            opened_by: 1,
            customer_label: None,
            team: None,
        };
        svc.write(
            WriteCtx {
                aggregate_id: sid,
                actor_staff: Some(1),
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &ev,
        )
        .unwrap();
    }

    #[test]
    fn warm_up_applies_clean_aggregates_normally() {
        let (events, km, svc) = rig();
        open(&svc, "a");
        open(&svc, "b");
        let store = AggregateStore::new();
        let report = store.warm_up(&events, &km).unwrap();
        assert_eq!(report.events_replayed, 2);
        assert!(report.aggregates_dropped.is_empty());
        assert_eq!(report.aggregates_replayed, 2);
    }

    #[test]
    fn warm_up_drops_aggregate_with_seq_gap() {
        let (events, km, svc) = rig();
        open(&svc, "a"); // clean aggregate, agg_seq=1
        // Aggregate "g": insert two events, then delete the first row so the
        // remaining row has agg_seq=2 with no agg_seq=1 — a hole.
        open(&svc, "g");
        open(&svc, "g");
        let rows_g = events.list_for_aggregate("g").unwrap();
        assert_eq!(rows_g.len(), 2);
        events.delete_by_id(rows_g[0].id).unwrap();

        let store = AggregateStore::new();
        let report = store.warm_up(&events, &km).unwrap();
        let dropped_ids: Vec<&String> =
            report.aggregates_dropped.iter().map(|(a, _)| a).collect();
        assert!(
            dropped_ids.contains(&&"g".to_string()),
            "g should be dropped: {report:?}"
        );
        assert!(!dropped_ids.contains(&&"a".to_string()));
    }

    #[test]
    fn warm_up_drops_aggregate_with_undecryptable_event() {
        let (events, _km_real, svc) = rig();
        open(&svc, "a");
        // A KeyManager with a different seed cannot decrypt rows tagged
        // seed_id="test" — the seed id is unknown -> Crypto error.
        let other_cache = Arc::new(SeedCache::from_seeds(
            "other",
            vec![("other".into(), [99u8; 32])],
        ));
        let km_bad = Arc::new(KeyManager::new(other_cache));
        let store = AggregateStore::new();
        let report = store.warm_up(&events, &km_bad).unwrap();
        let dropped_ids: Vec<&String> =
            report.aggregates_dropped.iter().map(|(a, _)| a).collect();
        assert!(dropped_ids.contains(&&"a".to_string()));
        let reason = &report
            .aggregates_dropped
            .iter()
            .find(|(a, _)| a == "a")
            .unwrap()
            .1;
        assert!(reason.contains("decrypt"), "got reason: {reason}");
    }
}
