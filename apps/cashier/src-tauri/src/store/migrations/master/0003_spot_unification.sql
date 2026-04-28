DROP TABLE IF EXISTS "table";
DROP TABLE IF EXISTS room;

CREATE TABLE spot (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  kind         TEXT NOT NULL CHECK (kind IN ('room','table')),
  hourly_rate  INTEGER,                                -- VND, only meaningful for kind='room'
  parent_id    INTEGER REFERENCES spot(id) ON DELETE SET NULL,
                                                       -- table inside a room (optional)
  status       TEXT NOT NULL DEFAULT 'idle',
  CHECK (kind = 'room' OR hourly_rate IS NULL)
);

CREATE INDEX idx_spot_kind ON spot(kind);
