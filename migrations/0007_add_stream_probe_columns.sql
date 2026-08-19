-- Track when and how each stream was last probed for liveness.

ALTER TABLE station_streams
    ADD COLUMN last_probe_at timestamptz,
    ADD COLUMN last_probe_error varchar(500);

CREATE INDEX station_streams_health_probe_idx
    ON station_streams (health, last_probe_at NULLS FIRST);
