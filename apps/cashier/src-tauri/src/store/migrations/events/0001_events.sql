CREATE TABLE event (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  business_day  TEXT NOT NULL,
  ts            INTEGER NOT NULL,
  type          TEXT NOT NULL,
  aggregate_id  TEXT NOT NULL,
  actor_staff   INTEGER,
  payload_enc   BLOB NOT NULL,
  key_id        TEXT NOT NULL
);

CREATE INDEX idx_event_day ON event(business_day);
CREATE INDEX idx_event_agg ON event(aggregate_id, id);
CREATE INDEX idx_event_day_type ON event(business_day, type);
