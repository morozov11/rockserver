# RockServer

RockServer is a planned Rust service for AI-assisted internet radio discovery. A RockCast user will be able to enter a request such as "calm instrumental jazz" and receive a ranked list of playable station streams. RockCast remains responsible for playback through its existing `PlaybackController`.

## Current status

The repository contains a Rust edition 2024 Axum service. `POST /v1/search` implements deterministic metadata search through a replaceable repository boundary. PostgreSQL is selected when `DATABASE_URL` is set; otherwise the same six-station catalog runs in memory. Versioned migrations create station and stream tables and seed the development catalog automatically. `GET /health/live` depends only on the process, while `GET /health/ready` checks PostgreSQL in database mode. There is no Radio Browser import, pgvector, LLM integration, external provider, or RockCast integration yet.

## Intended architecture

The service will be built in small, independently testable layers:

- a versioned Axum HTTP API whose contract is defined in `api/openapi.yaml`;
- transport DTOs separated from search domain models;
- deterministic catalog filtering and ranking over PostgreSQL or the built-in fallback;
- PostgreSQL migrations for stations and playable streams;
- pgvector as a future semantic-ranking store, not part of the current persistence stage;
- provider traits for query parsing and embeddings, with deterministic fakes in tests;
- Radio Browser import and stream health checks outside the request path.

An LLM will convert natural-language requests into structured filters; it will not inspect the full station catalog. RockCast will retain its local catalog as a fallback if the service is unavailable.

See [docs/architecture.md](docs/architecture.md) for boundaries and the planned request flow. The standalone, offline [service diagrams](docs/service-diagrams.html) distinguish current behavior from the next and future stages.

## Roadmap

1. Repository hygiene and contributor documentation.
2. Axum HTTP skeleton with health endpoints, tracing, graceful shutdown, and router tests.
3. OpenAPI contract for `POST /v1/search`.
4. Deterministic in-memory search with domain/DTO separation — complete.
5. PostgreSQL persistence, migrations, development seed, and dependency-aware readiness — complete.
6. Controlled Radio Browser import outside the request path — next.
7. Semantic ranking behind query-parser and embedding provider traits.
8. Small RockCast integration changes for remote search with local fallback.
9. Voice input only after text search is stable, followed by stream health checks, metrics, rate limiting, and deployment.

RS-004 is complete. The next work is a controlled Radio Browser catalog import; detailed acceptance criteria remain in [TODO.md](TODO.md).

## Build and test

Install a stable Rust toolchain that supports edition 2024, then run:

```text
cargo build
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Tests must be deterministic and must not access real LLM providers or the external network.

## Run locally

By default, RockServer listens only on `127.0.0.1:3000`. Override the complete socket address with `ROCKSERVER_BIND_ADDR`, for example:

```text
ROCKSERVER_BIND_ADDR=127.0.0.1:8080 cargo run
```

Use `RUST_LOG` to adjust the tracing filter. If it is unset or invalid, the service uses `info`.

With no `DATABASE_URL`, startup logs `backend=in_memory` and uses the built-in fallback. To run the local PostgreSQL backend with documented development-only defaults:

```text
docker compose up -d --wait
DATABASE_URL=postgres://rockserver:rockserver_dev@127.0.0.1:5432/rockserver cargo run
```

On PowerShell, set the variable with `$env:DATABASE_URL='postgres://rockserver:rockserver_dev@127.0.0.1:5432/rockserver'` before `cargo run`. Startup applies pending files from `migrations/` and logs `backend=postgresql` without logging the URL. Stop the local database with `docker compose down`; add `-v` only when the development catalog data should also be discarded.

Check readiness and seeded search after startup:

```text
curl http://127.0.0.1:3000/health/ready
curl -X POST http://127.0.0.1:3000/v1/search -H "content-type: application/json" -d '{"query":"calm instrumental jazz"}'
```

The real database integration test is opt-in so ordinary tests need no Docker or network:

```text
TEST_DATABASE_URL=postgres://USER:PASSWORD@127.0.0.1:PORT/DISPOSABLE_DATABASE cargo test --test postgres_integration --all-features -- --ignored --exact postgres_migrations_seed_search_and_readiness
```
