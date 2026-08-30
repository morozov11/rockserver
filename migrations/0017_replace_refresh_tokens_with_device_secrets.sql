-- Native pairing is a durable device binding, not a rotating refresh-token family.
-- Staging has no compatibility requirement: existing native devices must pair again.
UPDATE account_audit_events SET device_id = NULL
WHERE device_id IN (SELECT id FROM devices);
DELETE FROM sessions;
DELETE FROM devices;
DROP TABLE refresh_tokens;
ALTER TABLE sessions DROP COLUMN refresh_family_id;
ALTER TABLE devices ADD COLUMN device_secret_hash bytea NOT NULL UNIQUE
    CHECK (octet_length(device_secret_hash) = 32);
