-- Metadata and concurrency guard for the protected one-time administrator bootstrap.
-- Existing RS-ADMIN-001 principals remain readable; bootstrap creates only a missing principal.

ALTER TABLE admin_principals ADD COLUMN username varchar(64);
ALTER TABLE admin_principals ADD CONSTRAINT admin_principals_username_format_check
    CHECK (username IS NULL OR username ~ '^[A-Za-z0-9._-]{3,64}$');
CREATE UNIQUE INDEX admin_principals_username_idx
    ON admin_principals (username) WHERE username IS NOT NULL;
CREATE UNIQUE INDEX admin_principals_singleton_idx ON admin_principals ((true));
