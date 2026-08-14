CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE station_embeddings (
    station_id varchar(128) NOT NULL REFERENCES stations(id) ON DELETE CASCADE,
    model varchar(128) NOT NULL CHECK (btrim(model) <> ''),
    version varchar(64) NOT NULL CHECK (btrim(version) <> ''),
    dimension integer NOT NULL CHECK (dimension BETWEEN 1 AND 16000),
    embedding vector NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (station_id, model, version),
    CHECK (vector_dims(embedding) = dimension),
    CHECK (vector_norm(embedding) > 0)
);

CREATE INDEX station_embeddings_provenance_idx
    ON station_embeddings (model, version, dimension, station_id);
