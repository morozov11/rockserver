# RockServer

RockServer is a planned Rust service for AI-assisted internet radio discovery. A RockCast user will be able to enter a request such as "calm instrumental jazz" and receive a ranked list of playable station streams. RockCast remains responsible for playback through its existing `PlaybackController`.

## Current status

The repository contains a Rust edition 2024 Axum service skeleton. `GET /health/live` and `GET /health/ready` return `{"status":"ok"}` as JSON. The application emits structured JSON logs, traces HTTP requests, and shuts down gracefully on Ctrl+C. There is no search API, database, LLM integration, or RockCast integration yet.

## Intended architecture

The service will be built in small, independently testable layers:

- a versioned Axum HTTP API whose contract is defined in `api/openapi.yaml`;
- transport DTOs separated from search domain models;
- deterministic catalog filtering and ranking before semantic ranking is introduced;
- PostgreSQL with pgvector as the catalog and vector store;
- provider traits for query parsing and embeddings, with deterministic fakes in tests;
- Radio Browser import and stream health checks outside the request path.

An LLM will convert natural-language requests into structured filters; it will not inspect the full station catalog. RockCast will retain its local catalog as a fallback if the service is unavailable.

See [docs/architecture.md](docs/architecture.md) for boundaries and the planned request flow.

## Roadmap

1. Repository hygiene and contributor documentation.
2. Axum HTTP skeleton with health endpoints, tracing, graceful shutdown, and router tests.
3. OpenAPI contract for `POST /v1/search`.
4. Deterministic in-memory search with domain/DTO separation.
5. PostgreSQL, pgvector, migrations, and Radio Browser import.
6. Semantic ranking behind query-parser and embedding provider traits.
7. Small RockCast integration changes for remote search with local fallback.
8. Voice input only after text search is stable, followed by stream health checks, metrics, rate limiting, and deployment.

Near-term work and acceptance criteria are tracked in [TODO.md](TODO.md).

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
