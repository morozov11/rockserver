-- A stable, inspectable input document for local embedding models.  It is kept
-- on the station rather than derived in every backfill so a catalog update and
-- the vector it was derived from can be audited independently.
ALTER TABLE stations
    ADD COLUMN searchable_text text;

UPDATE stations
SET searchable_text = concat_ws(' ', name, array_to_string(tags, ' '), language, country_code)
WHERE searchable_text IS NULL;

ALTER TABLE stations
    ALTER COLUMN searchable_text SET NOT NULL,
    ADD CONSTRAINT stations_searchable_text_nonempty CHECK (btrim(searchable_text) <> '');

-- `intfloat/multilingual-e5-small` produces 384-dimensional vectors.  This
-- partial HNSW index leaves development/test vectors dimension-neutral while
-- making the selected production provenance ANN-capable.
CREATE INDEX station_embeddings_e5_small_hnsw_cosine_idx
    ON station_embeddings
    USING hnsw ((embedding::vector(384)) vector_cosine_ops)
    WHERE model = 'intfloat/multilingual-e5-small'
      AND version = 'onnx-v1'
      AND dimension = 384;
