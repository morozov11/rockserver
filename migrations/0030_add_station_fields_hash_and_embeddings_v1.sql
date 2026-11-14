-- E2: versioned station embedding storage. The migration only creates schema;
-- rows are written by the background lifecycle task or `backfill_embeddings
-- --version N`. `stations.fields_hash` attests the fields a vector was computed
-- from (canonical form: NUL-joined name, tag count, tags, language, country;
-- SHA-256 hex) and is written by catalog imports and backfilled for legacy
-- rows when their vector is embedded. The legacy `station_embeddings` table
-- and its indexes stay untouched until manual cleanup after the transition.

ALTER TABLE stations
    ADD COLUMN IF NOT EXISTS fields_hash text
    CONSTRAINT stations_fields_hash_format
    CHECK (fields_hash IS NULL OR fields_hash ~ '^[0-9a-f]{64}$');

CREATE TABLE IF NOT EXISTS station_embeddings_v1 (
    station_id varchar(128) PRIMARY KEY REFERENCES stations(id) ON DELETE CASCADE,
    embedding vector(384) NOT NULL,
    fields_hash text NOT NULL CHECK (fields_hash ~ '^[0-9a-f]{64}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (vector_norm(embedding) > 0)
);

CREATE INDEX IF NOT EXISTS station_embeddings_v1_hnsw_cosine_idx
    ON station_embeddings_v1
    USING hnsw (embedding vector_cosine_ops);
