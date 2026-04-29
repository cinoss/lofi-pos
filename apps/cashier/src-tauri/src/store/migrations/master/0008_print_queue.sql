-- Persistent FIFO print queue. Drained by a background tokio worker that
-- POSTs each job to the bouncer. Backoff via next_try_at; rows deleted on
-- success.
CREATE TABLE print_queue (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  kind         TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  target       TEXT,
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  enqueued_at  INTEGER NOT NULL,
  next_try_at  INTEGER NOT NULL
);
CREATE INDEX idx_print_queue_next ON print_queue(next_try_at);
