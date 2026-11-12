CREATE TABLE home_assistant_integrations (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    credential_hash bytea NOT NULL UNIQUE,
    allowed_sensors text[] NOT NULL,
    weather_entity text NOT NULL,
    revoked_at timestamptz,
    epoch uuid,
    revision bigint NOT NULL DEFAULT 0,
    last_payload_hash bytea,
    last_ingest_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX home_assistant_integrations_owner ON home_assistant_integrations(user_id);

CREATE TABLE home_assistant_sensor_states (
    integration_id uuid NOT NULL REFERENCES home_assistant_integrations(id) ON DELETE CASCADE,
    entity_id text NOT NULL,
    reading jsonb NOT NULL,
    received_at text NOT NULL,
    PRIMARY KEY (integration_id, entity_id)
);

CREATE TABLE home_assistant_weather_states (
    integration_id uuid PRIMARY KEY REFERENCES home_assistant_integrations(id) ON DELETE CASCADE,
    reading jsonb NOT NULL,
    received_at text NOT NULL
);
