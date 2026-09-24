-- User-owned read-only Yandex Smart Home authorization. OAuth state values are
-- represented only by a SHA-256 digest; reusable OAuth tokens are AES-GCM ciphertext.

CREATE TABLE yandex_home_oauth_states (
    state_hash bytea PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    user_id uuid NOT NULL REFERENCES users(id),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);
CREATE INDEX yandex_home_oauth_states_expiry_idx ON yandex_home_oauth_states (expires_at);

CREATE TABLE yandex_home_connections (
    user_id uuid PRIMARY KEY REFERENCES users(id),
    access_token_ciphertext bytea NOT NULL CHECK (octet_length(access_token_ciphertext) BETWEEN 17 AND 8192),
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 12),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
CREATE INDEX yandex_home_connections_active_idx ON yandex_home_connections (updated_at) WHERE revoked_at IS NULL;
