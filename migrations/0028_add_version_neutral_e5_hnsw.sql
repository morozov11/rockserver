-- All E5/384 preprocessing versions share an ANN index. The onnx-v1 partial
-- index from 0005 stays available during rollout and rollback.
CREATE INDEX station_embeddings_e5_384_hnsw_cosine_idx
    ON station_embeddings
    USING hnsw ((embedding::vector(384)) vector_cosine_ops)
    WHERE model = 'intfloat/multilingual-e5-small'
      AND dimension = 384;
