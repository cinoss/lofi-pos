-- 0004_token_denylist.sql
-- Per-token revocation list. `jti` is the UUID v4 generated at login;
-- `expires_at` matches the token's exp so a janitor can prune expired rows.
CREATE TABLE token_denylist (
  jti        TEXT PRIMARY KEY,
  expires_at INTEGER NOT NULL,
  revoked_at INTEGER NOT NULL
);

CREATE INDEX idx_token_denylist_expires ON token_denylist(expires_at);
