-- Durable, owner-scoped device-control v1 projections.  These are not a second device identity.
CREATE TABLE device_control_manifests (
    device_id uuid PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    revision bigint NOT NULL CHECK (revision > 0),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    accepted_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE device_control_capabilities (
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    capability_name varchar(96) NOT NULL,
    version smallint NOT NULL CHECK (version > 0),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    manifest_revision bigint NOT NULL CHECK (manifest_revision > 0),
    removed_at timestamptz,
    PRIMARY KEY (device_id, capability_name)
);

CREATE TABLE device_control_entities (
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    entity_id varchar(128) NOT NULL,
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    manifest_revision bigint NOT NULL CHECK (manifest_revision > 0),
    removed_at timestamptz,
    PRIMARY KEY (device_id, entity_id)
);
CREATE INDEX device_control_entities_current_idx ON device_control_entities (device_id, entity_id) WHERE removed_at IS NULL;

CREATE TABLE device_control_surfaces (
    device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    surface_id varchar(128) NOT NULL,
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    manifest_revision bigint NOT NULL CHECK (manifest_revision > 0),
    removed_at timestamptz,
    PRIMARY KEY (device_id, surface_id)
);
CREATE INDEX device_control_surfaces_current_idx ON device_control_surfaces (device_id, surface_id) WHERE removed_at IS NULL;

CREATE TABLE device_control_state_snapshots (
    device_id uuid PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    revision bigint NOT NULL CHECK (revision > 0),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    observed_at timestamptz NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE device_control_entity_states (
    device_id uuid NOT NULL,
    entity_id varchar(128) NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    payload jsonb NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    observed_at timestamptz NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    stale_after timestamptz NOT NULL,
    PRIMARY KEY (device_id, entity_id),
    FOREIGN KEY (device_id, entity_id) REFERENCES device_control_entities(device_id, entity_id) ON DELETE CASCADE
);

CREATE TABLE device_control_commands (
    command_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_device_id uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    request_fingerprint bytea NOT NULL CHECK (octet_length(request_fingerprint) = 32),
    request_payload jsonb NOT NULL CHECK (jsonb_typeof(request_payload) = 'object'),
    status varchar(16) NOT NULL CHECK (status IN ('reserved', 'succeeded', 'failed')),
    result_payload jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deadline_at timestamptz NOT NULL,
    completed_at timestamptz,
    CHECK ((status = 'reserved' AND result_payload IS NULL AND completed_at IS NULL) OR
           (status IN ('succeeded', 'failed') AND result_payload IS NOT NULL AND completed_at IS NOT NULL)),
    CHECK (deadline_at >= created_at)
);
CREATE INDEX device_control_commands_prune_idx ON device_control_commands (completed_at, command_id) WHERE status IN ('succeeded', 'failed');
