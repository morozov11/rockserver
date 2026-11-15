-- Preferred communication language code (e.g. 'ru', 'en') for the account.
-- NULL indicates no explicit user override (falls back to request header or 'en').
ALTER TABLE users ADD COLUMN language varchar(8);
