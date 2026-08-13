# Near-term TODO

1. Repository hygiene and documentation — complete
   - Acceptance: Rust edition remains 2024; local build, IDE, and environment artifacts are ignored; repository purpose and boundaries are documented; formatting, linting, and tests pass.

2. HTTP service skeleton — complete
   - Added Axum routing, health endpoints, structured tracing, graceful shutdown, and router-level tests.
   - Acceptance met: readiness and liveness endpoints return documented responses; shutdown is graceful; router tests require no network service; all required checks pass.

3. Search API contract — complete
   - Added `api/openapi.yaml` with `POST /v1/search`, request/response schemas, examples, and the standard error shape.
   - Acceptance met: the contract defines validation and status codes, includes `code`, `message`, `request_id`, and `details` for errors, and is covered by a contract validation check.

4. Deterministic in-memory search — complete
   - Implemented separate HTTP DTOs, domain models, a `StationRepository` trait, a small built-in in-memory catalog, explicit locale/country/exclusion constraints, and stable ranking.
   - Acceptance met: identical inputs use score-descending then station-ID-ascending order; DTO mapping and validation are covered by HTTP tests; malformed JSON returns 400 and well-formed invalid requests return 422 in the standard error shape; tests use neither external network nor AI providers.

5. PostgreSQL persistence foundation — complete
   - Added versioned station/stream migrations, an idempotent six-station development seed, PostgreSQL `StationRepository`, automatic startup migrations, `DATABASE_URL` backend selection, in-memory fallback, database-aware readiness, and a local Compose service.
   - Acceptance met: SQL preserves language/country/exclusion filters, limit, score-descending order and station-ID tie-break; unit and real PostgreSQL integration tests cover conversions, migrations, seed, search, exclusions, limit, ranking, and readiness; no compile-time SQL query macros require a live database.

6. Controlled Radio Browser import — complete
   - Added separate provider/import-store boundaries, a bounded Radio Browser client, deterministic validation and normalization, provider-owned idempotent PostgreSQL upserts, durable import-run accounting, and a manual one-shot CLI outside HTTP startup and search.
   - Acceptance met: the importer requires `DATABASE_URL`, sends an explicit User-Agent, bounds timeout/page size/page count/response bytes, preserves the six built-in stations, never deletes missing upstream records, logs run/page/count progress without credentials or stream URLs, and is covered by deterministic unit, local mock HTTP, and opt-in real PostgreSQL tests.
   - pgvector, embeddings, LLM parsing, authentication, rate limiting, stream probing, and RockCast changes remain excluded.

7. Semantic ranking — next
   - Add query-parser and embedding provider traits with deterministic fakes, pgvector persistence, and a metadata-safe ranking path that never sends the full catalog to an LLM.
   - Preserve the current HTTP contract and deterministic fallback while semantic behavior is introduced incrementally.
