CREATE TABLE idempotency_key (
  key           TEXT PRIMARY KEY,
  command       TEXT NOT NULL,
  result_json   TEXT NOT NULL,
  created_at    INTEGER NOT NULL
);
