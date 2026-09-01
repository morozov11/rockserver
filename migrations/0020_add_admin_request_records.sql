-- Bounded, secret-free operational metadata for authenticated administrator requests.
-- Raw credentials, headers, request bodies, search text, and voice transcripts are never stored.

CREATE TABLE admin_request_records (
    id uuid PRIMARY KEY,
    request_id varchar(128) NOT NULL,
    principal_id uuid NOT NULL REFERENCES admin_principals(id),
    session_id uuid NOT NULL REFERENCES admin_sessions(id),
    endpoint varchar(96) NOT NULL CHECK (endpoint ~ '^/[A-Za-z0-9_./-]+$'),
    outcome varchar(16) NOT NULL CHECK (outcome IN ('succeeded', 'rejected', 'failed')),
    duration_ms integer NOT NULL CHECK (duration_ms >= 0),
    occurred_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX admin_request_records_occurred_idx ON admin_request_records (occurred_at DESC);
CREATE INDEX admin_request_records_principal_occurred_idx
    ON admin_request_records (principal_id, occurred_at DESC);

ALTER TABLE admin_security_events DROP CONSTRAINT admin_security_events_event_type_check;
ALTER TABLE admin_security_events ADD CONSTRAINT admin_security_events_event_type_check
    CHECK (event_type IN (
        'admin_created', 'password_credential_created', 'login_succeeded', 'login_failed',
        'login_locked', 'session_created', 'session_revoked', 'session_rotated', 'logout'
    ));
