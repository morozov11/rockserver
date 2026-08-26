-- RM-011-B2 browser approval, pairing, WebAuthn challenge, and rate-limit state.
-- Secrets are accepted only as fixed-size keyed hashes; raw values never persist.

CREATE TABLE browser_sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    csrf_token_hash bytea NOT NULL UNIQUE,
    passkey_reauthenticated_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    revoked_at timestamptz,
    CHECK (expires_at > created_at),
    CHECK (octet_length(csrf_token_hash) = 32)
);
CREATE INDEX browser_sessions_active_user_idx
    ON browser_sessions (user_id, expires_at) WHERE revoked_at IS NULL;

CREATE TABLE pairing_requests (
    id uuid PRIMARY KEY,
    desktop_token_hash bytea NOT NULL UNIQUE,
    approval_secret_hash bytea NOT NULL UNIQUE,
    short_code_hash bytea NOT NULL UNIQUE,
    verification_phrase varchar(64) NOT NULL,
    device_name varchar(128) NOT NULL,
    platform varchar(64) NOT NULL,
    app_version varchar(64),
    expires_at timestamptz NOT NULL,
    approved_by_user_id uuid REFERENCES users(id),
    approved_at timestamptz,
    consumed_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at),
    CHECK ((approved_by_user_id IS NULL) = (approved_at IS NULL)),
    CHECK (consumed_at IS NULL OR approved_at IS NOT NULL),
    CHECK (octet_length(desktop_token_hash) = 32),
    CHECK (octet_length(approval_secret_hash) = 32),
    CHECK (octet_length(short_code_hash) = 32)
);
CREATE INDEX pairing_requests_active_expiry_idx
    ON pairing_requests (expires_at) WHERE consumed_at IS NULL AND revoked_at IS NULL;

CREATE TABLE webauthn_challenges (
    id uuid PRIMARY KEY,
    challenge_hash bytea NOT NULL UNIQUE,
    ceremony varchar(16) NOT NULL CHECK (ceremony IN ('registration', 'authentication')),
    rp_id varchar(255) NOT NULL,
    origin varchar(2048) NOT NULL,
    user_id uuid REFERENCES users(id),
    browser_session_id uuid REFERENCES browser_sessions(id),
    pairing_request_id uuid REFERENCES pairing_requests(id),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at),
    CHECK (octet_length(challenge_hash) = 32)
);
CREATE INDEX webauthn_challenges_active_expiry_idx
    ON webauthn_challenges (expires_at) WHERE consumed_at IS NULL AND revoked_at IS NULL;

CREATE TABLE rate_limit_buckets (
    key_hash bytea NOT NULL,
    bucket_started_at timestamptz NOT NULL,
    request_count bigint NOT NULL DEFAULT 0 CHECK (request_count >= 0),
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (key_hash, bucket_started_at),
    CHECK (octet_length(key_hash) = 32)
);
CREATE INDEX rate_limit_buckets_expiry_idx ON rate_limit_buckets (expires_at);
