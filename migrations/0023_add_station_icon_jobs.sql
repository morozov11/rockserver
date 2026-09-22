CREATE TABLE station_icon_jobs (
    id uuid PRIMARY KEY,
    status varchar(16) NOT NULL CHECK (status IN ('running', 'completed', 'interrupted', 'failed')),
    selected_count integer NOT NULL DEFAULT 0 CHECK (selected_count >= 0),
    processed_count integer NOT NULL DEFAULT 0 CHECK (processed_count >= 0),
    ready_count integer NOT NULL DEFAULT 0 CHECK (ready_count >= 0),
    missing_count integer NOT NULL DEFAULT 0 CHECK (missing_count >= 0),
    retryable_error_count integer NOT NULL DEFAULT 0 CHECK (retryable_error_count >= 0),
    permanent_error_count integer NOT NULL DEFAULT 0 CHECK (permanent_error_count >= 0),
    skipped_count integer NOT NULL DEFAULT 0 CHECK (skipped_count >= 0),
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    last_error_code varchar(64),
    CHECK ((status IN ('running', 'failed')) = (finished_at IS NULL))
);
CREATE UNIQUE INDEX station_icon_jobs_one_active_idx ON station_icon_jobs ((1)) WHERE status = 'running';

CREATE TABLE station_icon_job_items (
    job_id uuid NOT NULL REFERENCES station_icon_jobs(id) ON DELETE CASCADE,
    station_id varchar(128) NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    status varchar(16) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processing', 'ready', 'missing', 'retryable_error', 'permanent_error', 'skipped')),
    PRIMARY KEY (job_id, station_id)
);
CREATE INDEX station_icon_job_items_pending_idx ON station_icon_job_items (job_id, station_id) WHERE status = 'pending';
