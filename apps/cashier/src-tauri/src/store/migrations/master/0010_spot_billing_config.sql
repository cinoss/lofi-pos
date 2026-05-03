-- Pre-prod destructive migration: spot now carries a JSON billing_config
-- blob in place of the flat hourly_rate column. NULL for tables.
DROP TABLE IF EXISTS spot;
CREATE TABLE spot (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  name            TEXT NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN ('room','table')),
  parent_id       INTEGER REFERENCES spot(id) ON DELETE SET NULL,
  status          TEXT NOT NULL DEFAULT 'idle',
  -- Room billing policy as JSON: {hourly_rate, bucket_minutes,
  -- included_minutes, min_charge}. NULL for tables. Snapshotted into
  -- SpotRef::Room at session-open / transfer time.
  billing_config  TEXT
);
CREATE INDEX idx_spot_kind ON spot(kind);
