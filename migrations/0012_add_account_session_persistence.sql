-- Passkey-only account/session persistence.  No password, access token, refresh token, browser
-- cookie, pairing secret, or WebAuthn assertion is stored in plaintext.
CREATE TABLE users (
    id uuid PRIMARY KEY,
    status text NOT NULL CHECK (status IN ('active', 'deleted')) DEFAULT 'active',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);

CREATE TABLE passkey_credentials (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    credential_id bytea NOT NULL UNIQUE,
    public_key bytea NOT NULL,
    sign_count bigint NOT NULL DEFAULT 0 CHECK (sign_count >= 0),
    transports text[] NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at timestamptz
);
CREATE INDEX passkey_credentials_active_user_idx ON passkey_credentials (user_id) WHERE revoked_at IS NULL;

-- Reserved extension point.  RM-011-B does not create password, email, phone, or external identities.
CREATE TABLE account_identities (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    kind text NOT NULL,
    subject_hash bytea NOT NULL,
    subject_ciphertext bytea NOT NULL,
    verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE UNIQUE INDEX account_identities_active_subject_idx
    ON account_identities (kind, subject_hash) WHERE revoked_at IS NULL;

CREATE TABLE devices (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    name varchar(128) NOT NULL,
    platform varchar(64) NOT NULL,
    app_version varchar(64),
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    revoked_at timestamptz
);
CREATE INDEX devices_active_user_idx ON devices (user_id) WHERE revoked_at IS NULL;

CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    device_id uuid NOT NULL REFERENCES devices(id),
    access_token_hash bytea NOT NULL UNIQUE,
    access_expires_at timestamptz NOT NULL,
    refresh_family_id uuid NOT NULL,
    revoked_at timestamptz,
    last_seen_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sessions_active_user_device_idx ON sessions (user_id, device_id) WHERE revoked_at IS NULL;

CREATE TABLE refresh_tokens (
    id uuid PRIMARY KEY,
    session_id uuid NOT NULL REFERENCES sessions(id),
    family_id uuid NOT NULL,
    token_hash bytea NOT NULL UNIQUE,
    issued_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    replaced_by_id uuid UNIQUE REFERENCES refresh_tokens(id),
    revoked_at timestamptz,
    CHECK (expires_at > issued_at)
);
CREATE INDEX refresh_tokens_family_idx ON refresh_tokens (family_id);
CREATE INDEX refresh_tokens_active_hash_idx ON refresh_tokens (token_hash) WHERE used_at IS NULL AND revoked_at IS NULL;

-- Payloads are deliberately application-supplied safe classifications, never request credentials.
CREATE TABLE account_audit_events (
    id uuid PRIMARY KEY,
    user_id uuid REFERENCES users(id),
    device_id uuid REFERENCES devices(id),
    event_type varchar(64) NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    details jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX account_audit_events_user_occurred_idx ON account_audit_events (user_id, occurred_at DESC);
