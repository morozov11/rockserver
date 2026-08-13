CREATE TABLE import_runs (
    id uuid PRIMARY KEY,
    source varchar(32) NOT NULL CHECK (btrim(source) <> ''),
    status varchar(16) NOT NULL CHECK (status IN ('started', 'completed', 'failed')),
    fetched_count bigint NOT NULL DEFAULT 0 CHECK (fetched_count >= 0),
    imported_count bigint NOT NULL DEFAULT 0 CHECK (imported_count >= 0),
    skipped_count bigint NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    failed_count bigint NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    error_summary varchar(500),
    started_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CHECK (
        (status = 'started' AND completed_at IS NULL)
        OR (status IN ('completed', 'failed') AND completed_at IS NOT NULL)
    )
);

ALTER TABLE stations
    ADD COLUMN source varchar(32) NOT NULL DEFAULT 'builtin',
    ADD COLUMN source_station_id varchar(128),
    ADD COLUMN last_import_run_id uuid REFERENCES import_runs(id);

UPDATE stations SET source_station_id = id WHERE source_station_id IS NULL;

ALTER TABLE stations
    ALTER COLUMN source_station_id SET NOT NULL;

ALTER TABLE station_streams
    ADD COLUMN source varchar(32) NOT NULL DEFAULT 'builtin',
    ADD COLUMN source_stream_id varchar(128),
    ADD COLUMN last_import_run_id uuid REFERENCES import_runs(id);

UPDATE station_streams
SET source_stream_id = station_id
WHERE source_stream_id IS NULL;

ALTER TABLE station_streams
    ALTER COLUMN source_stream_id SET NOT NULL,
    DROP CONSTRAINT station_streams_stream_url_key;

CREATE UNIQUE INDEX stations_source_identity_idx
    ON stations (source, source_station_id);
CREATE UNIQUE INDEX station_streams_source_identity_idx
    ON station_streams (source, source_stream_id);
CREATE INDEX station_streams_stream_url_idx
    ON station_streams (stream_url);
CREATE INDEX import_runs_source_started_idx
    ON import_runs (source, started_at DESC);
