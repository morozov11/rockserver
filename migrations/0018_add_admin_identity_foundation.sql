-- Durable administrator authentication state.  It is intentionally separate from
-- passkey account/device state and future RockCast machine-client credentials.
-- Passwords and opaque credential material are represented only by hashes.

CREATE TABLE admin_principals (
    id uuid PRIMARY KEY,
    status varchar(16) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    disabled_at timestamptz
);
CREATE INDEX admin_principals_active_idx ON admin_principals (created_at) WHERE status = 'active';

CREATE TABLE admin_password_credentials (
    id uuid PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES admin_principals(id),
    password_hash text NOT NULL CHECK (password_hash LIKE '$argon2id$%'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE UNIQUE INDEX admin_password_credentials_active_principal_idx
    ON admin_password_credentials (principal_id) WHERE revoked_at IS NULL;

CREATE TABLE admin_sessions (
    id uuid PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES admin_principals(id),
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    revoked_at timestamptz,
    replaced_by_id uuid UNIQUE REFERENCES admin_sessions(id),
    CHECK (expires_at > created_at)
);
CREATE INDEX admin_sessions_active_principal_expiry_idx
    ON admin_sessions (principal_id, expires_at) WHERE revoked_at IS NULL;

CREATE TABLE admin_login_attempts (
    id uuid PRIMARY KEY,
    principal_id uuid REFERENCES admin_principals(id),
    account_key_hash bytea NOT NULL CHECK (octet_length(account_key_hash) = 32),
    source_ip_hash bytea NOT NULL CHECK (octet_length(source_ip_hash) = 32),
    outcome varchar(16) NOT NULL CHECK (outcome IN ('succeeded', 'failed', 'locked')),
    occurred_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX admin_login_attempts_throttle_idx
    ON admin_login_attempts (account_key_hash, source_ip_hash, occurred_at DESC);

CREATE TABLE admin_security_events (
    id uuid PRIMARY KEY,
    principal_id uuid REFERENCES admin_principals(id),
    session_id uuid REFERENCES admin_sessions(id),
    source_ip_hash bytea CHECK (source_ip_hash IS NULL OR octet_length(source_ip_hash) = 32),
    event_type varchar(64) NOT NULL CHECK (event_type IN (
        'admin_created', 'password_credential_created', 'login_succeeded', 'login_failed',
        'login_locked', 'session_created', 'session_revoked', 'logout'
    )),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    details jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX admin_security_events_principal_occurred_idx
    ON admin_security_events (principal_id, occurred_at DESC);
CREATE INDEX admin_security_events_occurred_idx ON admin_security_events (occurred_at DESC);
