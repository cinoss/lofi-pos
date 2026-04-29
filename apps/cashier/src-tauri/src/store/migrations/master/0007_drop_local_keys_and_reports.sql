-- Bouncer integration: cashier no longer stores keys or daily reports locally.
-- Pre-prod: destructive drop is fine.
DROP TABLE IF EXISTS dek;
DROP TABLE IF EXISTS daily_report;
