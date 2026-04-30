-- 0009_venue_settings.sql
-- Venue identity / display rows. Empty values are the "needs setup" sentinel.
INSERT OR IGNORE INTO setting(key, value) VALUES
  ('venue_name',     ''),
  ('venue_address',  ''),
  ('venue_phone',    ''),
  ('currency',       'VND'),
  ('locale',         'vi-VN'),
  ('tax_id',         ''),
  ('receipt_footer', '');
