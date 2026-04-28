CREATE TABLE staff (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT NOT NULL,
  pin_hash    TEXT NOT NULL,
  role        TEXT NOT NULL CHECK (role IN ('staff','cashier','manager','owner')),
  team        TEXT,
  created_at  INTEGER NOT NULL
);

CREATE TABLE room (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL UNIQUE,
  hourly_rate  INTEGER NOT NULL,        -- VND, integer
  status       TEXT NOT NULL DEFAULT 'idle'
);

CREATE TABLE "table" (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  name      TEXT NOT NULL UNIQUE,
  room_id   INTEGER REFERENCES room(id) ON DELETE SET NULL,
  status    TEXT NOT NULL DEFAULT 'idle'
);

CREATE TABLE product (
  id     INTEGER PRIMARY KEY AUTOINCREMENT,
  name   TEXT NOT NULL,
  price  INTEGER NOT NULL,              -- VND
  route  TEXT NOT NULL CHECK (route IN ('kitchen','bar','none')),
  kind   TEXT NOT NULL CHECK (kind IN ('item','recipe','time'))
);

CREATE TABLE recipe (
  product_id    INTEGER NOT NULL REFERENCES product(id) ON DELETE CASCADE,
  ingredient_id INTEGER NOT NULL REFERENCES product(id) ON DELETE RESTRICT,
  qty           REAL NOT NULL,           -- grams or units
  unit          TEXT NOT NULL,           -- 'g' | 'unit' | 'ml'
  PRIMARY KEY (product_id, ingredient_id)
);

CREATE TABLE setting (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT INTO setting(key, value) VALUES
  ('discount_threshold_pct', '10'),
  ('cancel_grace_minutes',   '5'),
  ('idle_lock_minutes',      '10'),
  ('business_day_cutoff_hour', '11'),
  ('business_day_tz_offset_seconds', '25200');  -- +7 hours, Asia/Ho_Chi_Minh

CREATE TABLE day_key (
  business_day TEXT PRIMARY KEY,         -- 'YYYY-MM-DD'
  wrapped_dek  BLOB NOT NULL,
  created_at   INTEGER NOT NULL
);

CREATE TABLE daily_report (
  business_day            TEXT PRIMARY KEY,
  generated_at            INTEGER NOT NULL,
  order_summary_json      TEXT NOT NULL,
  inventory_summary_json  TEXT NOT NULL
);
