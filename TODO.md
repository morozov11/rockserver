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

7. Semantic ranking — complete
   - Added provider-neutral query-parser and embedding traits, deterministic fakes and an explicit development embedder, dimension-neutral pgvector persistence with provenance, controlled backfill/update, and deterministic exact hybrid ranking.
   - Acceptance met: providers never receive the full catalog; failures and missing/incompatible embeddings preserve metadata fallback; hard filters/exclusions precede final limit; station ID is the last tie-break; in-memory mode and public HTTP schemas remain compatible; real pgvector integration is covered.

8. Windows RockCast production path — next
   - First verify RS-007 against the disposable local PostgreSQL/pgvector database, then connect RockCast text search to `POST /v1/search` with the local catalog retained as fallback.
   - The versioned JSON and WebSocket voice endpoints are implemented. Next add the Yandex SpeechKit adapter, Windows microphone capture, provider conformance/end-to-end tests, and production hardening in the order defined by `docs/windows-production-roadmap.md`.
   - ESP32 is outside the current delivery plan and remains a future client of the stabilized RockServer API.
