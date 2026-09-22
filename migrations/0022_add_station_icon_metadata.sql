-- Metadata only: icon bytes are stored separately and never downloaded by a migration.
CREATE TABLE station_icons (
    station_id varchar(128) PRIMARY KEY REFERENCES stations(id) ON DELETE CASCADE,
    source_url text,
    source_priority smallint NOT NULL DEFAULT 0 CHECK (source_priority BETWEEN 0 AND 2),
    storage_key text,
    content_type varchar(32),
    byte_size integer CHECK (byte_size > 0 AND byte_size <= 2097152),
    width integer CHECK (width > 0 AND width <= 256),
    height integer CHECK (height > 0 AND height <= 256),
    content_hash bytea CHECK (content_hash IS NULL OR octet_length(content_hash) = 32),
    source_etag text,
    source_last_modified text,
    status varchar(16) NOT NULL DEFAULT 'missing' CHECK (status IN ('pending', 'ready', 'missing', 'retryable_error', 'permanent_error')),
    manual_override boolean NOT NULL DEFAULT false,
    refresh_needed boolean NOT NULL DEFAULT false,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    retry_after timestamptz,
    last_error_code varchar(64),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'ready') = (storage_key IS NOT NULL AND content_type = 'image/webp' AND byte_size IS NOT NULL AND width IS NOT NULL AND height IS NOT NULL AND content_hash IS NOT NULL))
);

CREATE INDEX station_icons_due_import_idx
    ON station_icons (retry_after, station_id)
    WHERE (status IN ('pending', 'missing', 'retryable_error') OR refresh_needed) AND manual_override = false;
