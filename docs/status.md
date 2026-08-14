# Project status

Last updated: 2026-08-14

## Current state

Stages 0–7 are complete in the current working tree: repository bootstrap, Axum HTTP skeleton, OpenAPI search contract, deterministic in-memory search, PostgreSQL persistence, controlled Radio Browser import, and RS-007 semantic ranking foundation.

`POST /v1/search` keeps the existing request and response schemas. `SearchService` now owns request-only query interpretation and optional query embedding before calling `StationRepository`. The in-memory repository remains metadata-only. PostgreSQL uses exact pgvector cosine similarity when the query and station embedding provenance match, and otherwise preserves metadata fallback.

The query parser and embedding provider are traits. The deterministic metadata parser is the default and the parser failure fallback. The only concrete embedding provider is `deterministic-dev`, an explicitly non-production, hash-based local fixture. No LLM, OpenAI, or external model provider is present, and ordinary tests make no provider or external network call.

Station embedding generation is a separate `backfill_embeddings` command. It is never called by HTTP startup or `POST /v1/search`. Radio Browser import ownership and update semantics remain unchanged.

## Configuration and behavior

- HTTP listener: `ROCKSERVER_BIND_ADDR`, default `127.0.0.1:3000`.
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- HTTP catalog backend: `DATABASE_URL` selects PostgreSQL; absence selects the six-station in-memory metadata fallback.
- Optional query embeddings: `ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev`; absence means metadata-only search.
- Development embedding dimension: `ROCKSERVER_EMBEDDING_DIMENSION`, default 32, valid range 1–16,000.
- Development embedding provenance: model `rockserver-deterministic-dev`, version `1`, plus configured dimension.
- Embedding command: `cargo run --bin backfill_embeddings`; it requires both `DATABASE_URL` and explicit deterministic provider selection.
- Radio Browser import command and configuration remain as documented in `README.md`; no import provider is called by search.
- Local database: `compose.yaml` uses `pgvector/pgvector:pg17` with development-only credentials and a PostgreSQL healthcheck.
- Migrations: embedded versioned files run for PostgreSQL search, import, and embedding store connections.
- Public HTTP/OpenAPI schemas: unchanged by RS-007; the description now records optional semantic ranking and metadata fallback.

## Query, ranking, and failure semantics

`QueryParser` receives only validated query text and locale. It returns terms, tags, language, and country intent and has no catalog parameter. Provider intent is normalized and hard filters are validated before repository use. A parser error or invalid intent uses the deterministic metadata parser.

`EmbeddingProvider` receives one query string during search or one station document during the controlled backfill. It never receives the full catalog. Invalid embeddings are rejected before persistence/search: model and version must be non-empty, dimension must be 1–16,000 and equal to the vector length, values must be finite, and the vector must be non-zero. Embedding provider failure omits semantic input and continues with metadata search.

PostgreSQL applies language/country hard filters and station exclusions in the candidate CTE, before score ordering and final limit. Metadata score retains the RS-003/RS-004 exact-token formula. Compatible cosine similarity is normalized and clamped with `1 - cosine_distance / 2`. Final hybrid score is `0.70 * metadata_score + 0.30 * semantic_score`. A station without compatible model/version/dimension provenance retains its full metadata score. Results order by final score descending and station ID ascending; station ID is the last tie-break.

Liveness remains process-only. Readiness calls only the selected repository, so optional parser/embedding failure does not report false unready. PostgreSQL loss still returns 503/`not_ready`; in-memory readiness remains 200/`ok`.

## Persistence and backfill

Migration `0004_add_station_embeddings.sql` enables pgvector and adds `station_embeddings` with station ownership, model, version, dimension, unbounded `vector`, and created/updated timestamps. CHECK constraints enforce declared dimension and non-zero norm. The table key is `(station_id, model, version)`; changing dimension for the same development model/version replaces that station embedding.

The unbounded vector column deliberately avoids locking schema to a random test/dev dimension. Exact search filters compatible provenance before the cosine operator. RS-007 does not add HNSW or IVFFlat: those indexes require a fixed-dimensional expression/operator class and should be introduced only for a selected production model and measured catalog scale.

`backfill_embeddings` reads station documents in stable station-ID pages and upserts one embedding at a time. Repeating the workflow is idempotent for the same provenance and updates existing rows. It does not change Radio Browser ownership, delete catalog rows, probe streams, or make network requests.

## Known limitations

- `deterministic-dev` is a repeatable hash fixture, not a meaningful production semantic model.
- There is no LLM query parser, production embedding provider, provider authentication, retry policy, or circuit breaker.
- Exact vector search has no ANN index and is intended as a correctness foundation, not a scale claim.
- The backfill currently visits all stations and upserts every embedding; it does not yet skip unchanged station/model inputs or resume a failed run from durable workflow state.
- A model/version is expected to imply one dimension, but RS-007 records/enforces compatibility per row rather than introducing a separate model registry.
- Existing Radio Browser pagination, stale-run, language-code, and upstream-health limitations from RS-006 remain.
- Authentication, rate limiting, metrics, scheduler, stream probing, voice input, deployment hardening, and RockCast client integration remain out of scope.

## Verification

The regular all-target/all-feature suite passes with deterministic unit and HTTP coverage; the real PostgreSQL integration is opt-in. Unit coverage includes query-parser request boundaries, parser and embedding failure fallback, deterministic fake embeddings, model/dimension/value validation, controlled backfill, hybrid weights, exclusions, limits, and stable station-ID ordering.

Docker Engine 29.7.2 and Compose 5.3.1 ran isolated project `rockserver-rs007-test` on host port 55438 with `pgvector/pgvector:pg17`. The real integration test passed for extension/migrations, embedding insert and repeat update, provenance/dimension storage, exact cosine similarity, hard filters, exclusions, final limit, semantic tie-break, metadata fallback, HTTP search, and repository-only readiness. The real deterministic backfill command also completed twice; inspection showed seven unique development rows for `rockserver-deterministic-dev:1:8`. Only that project's container, network, and volume were removed afterward.

Final formatting, strict Clippy, all-target/all-feature tests, diff whitespace, Compose, HTML, and Graphify results are recorded in `docs/tasks.md` after completion.

## Next step

Make the smallest RockCast change needed for remote text search while retaining its local station catalog as offline/unavailable-service fallback. Do not add voice input or broader operations in that stage.
