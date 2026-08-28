-- RM-011-G1 account/device wording. Existing rows retain their names and receive a safe account label.
ALTER TABLE users
    ADD COLUMN account_display_name varchar(128) NOT NULL DEFAULT 'Rock account';

ALTER TABLE devices RENAME COLUMN name TO device_display_name;
ALTER TABLE devices RENAME COLUMN platform TO device_type;
ALTER TABLE pairing_requests RENAME COLUMN device_name TO device_display_name;
ALTER TABLE pairing_requests RENAME COLUMN platform TO device_type;
