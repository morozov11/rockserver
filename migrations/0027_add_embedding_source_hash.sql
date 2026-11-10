-- NULL marks legacy or manually inserted vectors whose source text was not attested.
-- The controlled backfill replaces them once for its exact model/version.
ALTER TABLE station_embeddings
    ADD COLUMN source_text_sha256 text
    CONSTRAINT station_embeddings_source_text_sha256_format
        CHECK (source_text_sha256 ~ '^[0-9a-f]{64}$');
