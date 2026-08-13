# Project status

Last updated: 2026-08-13

## Current state

Stages 0–3 are complete: repository bootstrap, the Axum HTTP skeleton, the OpenAPI search contract, and deterministic in-memory search are present in the current working tree.

Stage 4, the PostgreSQL persistence foundation, is complete in the current working tree. Versioned SQL migrations create separate station and stream tables with constraints, timestamps, an at-most-one-primary-stream invariant, and catalog indexes. An idempotent development migration preserves the six-station fallback catalog in PostgreSQL. `PostgresStationRepository` executes hard filters, exclusions, scoring, deterministic score-descending/station-ID-ascending ordering, and limit in parameterized SQL behind the existing domain boundary.

Startup selects PostgreSQL when `DATABASE_URL` exists, applies pending migrations, and otherwise retains the built-in in-memory backend. The selected backend name is logged without the DSN. Liveness remains process-only. Readiness checks the active repository and returns HTTP 503 with `{"status":"not_ready"}` when PostgreSQL is unavailable; the in-memory backend remains immediately ready.

## Configuration and behavior

- Generated `graphify-out/` artifacts are local-only, ignored as a complete directory, and absent from the Git index.
- Listener: `ROCKSERVER_BIND_ADDR`, default `127.0.0.1:3000`.
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- Catalog backend: `DATABASE_URL` selects PostgreSQL; absence selects the six-station in-memory fallback.
- PostgreSQL integration tests: `TEST_DATABASE_URL` must point to a disposable database and is used only by the explicitly ignored test.
- Migrations: embedded files in `migrations/` run automatically before PostgreSQL startup completes.
- Local database: `compose.yaml` provides PostgreSQL 17 with development-only defaults and a healthcheck.
- `GET /health/live`: HTTP 200 with `{"status":"ok"}`, independent of the database.
- `GET /health/ready`: HTTP 200/`ok` when ready, or HTTP 503/`not_ready` if PostgreSQL cannot answer the dependency probe.
- `POST /v1/search`: HTTP 200 with normalized input and deterministic results from the selected backend; validation behavior remains 400/422 as documented.

## Known limitations

The catalog contains only the six development stations. There is no Radio Browser import, pgvector, semantic similarity, LLM or embedding integration, authentication, rate limiting, external provider, or RockCast client integration. SQL ranking is exact metadata matching. Migrations run at application startup, so a PostgreSQL startup fails closed if the database is unreachable or migration application fails. The Compose credentials are intentionally local development defaults and are not production configuration.

## Verification

On 2026-08-13, Docker Engine 29.7.2 was available. An isolated `rockserver-rs004-test` Compose project started PostgreSQL 17, and the opt-in integration test passed against it. The test verified migrations, the development seed, successful HTTP search, exclusions, limit, stable tie-breaking, readiness while connected, and HTTP 503/`not_ready` after the pool was closed. The test container, network, and project-scoped volume were then removed.

The regular suite has 15 passing tests plus the one opt-in PostgreSQL test ignored by default. `cargo fmt --check`, strict Clippy for all targets and features, `cargo test --all-targets --all-features`, and `git diff --check` are the required final checks; their final RS-004 results are recorded in `docs/tasks.md`.

## Next step

Add a controlled Radio Browser import outside the request path. Keep pgvector, embeddings, LLM parsing, authentication, and rate limiting for later stages.
