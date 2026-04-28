-- Plan: UTC key rotation. Replaces the business-day-keyed `day_key` table with
-- a UTC-day-keyed `dek` table. Pre-prod: destructive, no data preserved.
DROP TABLE IF EXISTS day_key;

CREATE TABLE dek (
  utc_day     TEXT PRIMARY KEY,         -- 'YYYY-MM-DD' UTC
  wrapped_dek BLOB NOT NULL,
  created_at  INTEGER NOT NULL
);
