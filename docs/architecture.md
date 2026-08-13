# Architecture outline

## Scope and boundaries

RockServer owns remote station discovery: accepting a natural-language query, interpreting it, searching a catalog, ranking candidates, and returning stream metadata. It does not own the Windows UI or audio playback. Those remain in RockCast, which also retains a local catalog fallback.

## Search request flow

1. RockCast sends a request to the versioned search endpoint.
2. The HTTP layer validates the DTO and assigns a request ID.
3. The deterministic normalizer converts free text into lowercase terms, recognized tags, and supported locale/country constraints.
4. The search domain owns the meaning of hard language, country, and exclusion filters plus metadata scoring, stable ordering, and limit.
5. The selected `StationRepository` executes those rules: the in-memory backend uses the shared Rust ranker, while PostgreSQL performs equivalent parameterized filtering and ranking in one SQL query ordered by score descending and station ID ascending.
6. The HTTP layer maps ranked domain results to the contract DTO.
7. RockCast selects a result and passes its stream URL to the existing playback controller.

Future provider failures must degrade predictably. Query parsing and embeddings will live behind traits, allowing deterministic fake implementations in unit tests. The request path must not depend on crawling Radio Browser or probing streams; importing and health checking are background concerns.

## Current HTTP service

The crate is split into a reusable library and a thin process entry point. The library builds the Axum router, owns configuration and telemetry setup, and exposes the serving boundary so application behavior can be tested without starting a process. The binary loads configuration, binds the listener, and supplies the Ctrl+C shutdown signal.

The operational routes remain outside the versioned public API:

- `GET /health/live` reports that the process is running.
- `GET /health/ready` returns 200/`ok` for the in-memory backend and checks `SELECT 1` for PostgreSQL, returning 503/`not_ready` if the database is unavailable.

Liveness always returns the stable JSON model `{"status":"ok"}` and never depends on PostgreSQL. Request spans are produced by the HTTP tracing middleware and application events use structured JSON tracing. The listener defaults to `127.0.0.1:3000` and can be overridden with `ROCKSERVER_BIND_ADDR`.

## Search API

`api/openapi.yaml` is the source of truth for `POST /v1/search`. It defines bounded natural-language input, locale and result-limit defaults, station exclusions, normalized query output, ranked station metadata, and the standard error shape. The Axum route implements its 200, 400, and 422 outcomes: a 400 means the body is missing, not JSON, or malformed; a 422 means syntactically valid JSON failed schema or semantic validation. The 429 and 500 outcomes remain contractual because rate limiting and richer failure handling are not implemented.

The HTTP layer owns DTO deserialization, defaults, validation, request IDs, and `ErrorResponse` mapping. The search domain owns query normalization and the repository-neutral meaning of filtering, deterministic scoring, score-descending/station-ID-ascending ordering, and limit. `StationRepository` returns ranked domain models and also exposes a readiness check. PostgreSQL intentionally executes the domain rules in SQL so it can filter, order, and limit before transferring rows; the in-memory fallback applies the same rules in Rust. This avoids leaking SQL types into the domain while keeping large-catalog work out of the HTTP layer.

## Persistence

When `DATABASE_URL` is present, startup connects through SQLx, applies embedded versioned migrations from `migrations/`, and selects `PostgresStationRepository`. The schema separates station metadata from playable stream URLs, enforces stable IDs, positive bitrates, valid health states, at most one primary stream per station, timestamps, and useful metadata/relationship indexes. A second idempotent migration seeds PostgreSQL with the same six development stations as the offline in-memory fallback.

When `DATABASE_URL` is absent, startup selects `InMemoryStationRepository`; both choices are logged by backend name without logging a DSN or password. SQL uses runtime-checked queries rather than compile-time query macros, so normal builds do not require a live database. `compose.yaml` is development-only and has a PostgreSQL healthcheck. Radio Browser import and pgvector remain outside the current stage.

The standalone [service diagrams](service-diagrams.html) summarize the current, next, and future architecture in Russian and work locally without a server or network access.

## Planned modules

- **API:** Axum health and search routing, DTO validation, error mapping, and request IDs are present.
- **Domain:** normalized queries, filters, station candidates, scoring, and stable ranking are present.
- **Persistence:** PostgreSQL migrations, development seed, SQL repository, startup backend selection, and in-memory fallback are present; controlled Radio Browser import is next and pgvector remains future work.
- **Providers:** replaceable query-parser and embedding implementations.
- **Operations:** tracing, process liveness, backend-aware readiness, configurable binding, local PostgreSQL Compose, and Ctrl+C shutdown are present; metrics, rate limiting, broader shutdown triggers, and deployment configuration remain planned.

Dependencies should point inward toward domain concepts. HTTP, database, and provider-specific types must not leak into the core ranking model.
