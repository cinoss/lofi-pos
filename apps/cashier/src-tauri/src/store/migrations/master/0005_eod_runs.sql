-- 0005_eod_runs.sql
-- Audit log of EOD pipeline runs. One row per business day that the
-- scheduler (or manual `eod-now` CLI) attempted to process. Lets ops see
-- which days completed and which failed (with the error string for triage).
CREATE TABLE eod_runs (
  business_day TEXT PRIMARY KEY,
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER,
  status       TEXT NOT NULL CHECK (status IN ('running','ok','failed')),
  error        TEXT
);
