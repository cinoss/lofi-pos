-- Bouncer integration: rename event.key_id -> event.seed_id and add agg_seq.
-- Pre-prod: destructive recreate. No backfill.
DROP TABLE IF EXISTS event;
CREATE TABLE event (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  business_day        TEXT NOT NULL,
  ts                  INTEGER NOT NULL,
  type                TEXT NOT NULL,
  aggregate_id        TEXT NOT NULL,
  agg_seq             INTEGER NOT NULL,
  actor_staff         INTEGER,
  actor_name          TEXT,
  override_staff_id   INTEGER,
  override_staff_name TEXT,
  payload_enc         BLOB NOT NULL,
  seed_id             TEXT NOT NULL,
  UNIQUE(aggregate_id, agg_seq)
);
CREATE INDEX idx_event_day      ON event(business_day);
CREATE INDEX idx_event_agg      ON event(aggregate_id, agg_seq);
CREATE INDEX idx_event_day_type ON event(business_day, type);
