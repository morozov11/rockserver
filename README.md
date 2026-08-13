# RockServer

RockServer is a planned Rust service for AI-assisted internet radio discovery. A RockCast user will be able to enter a request such as "calm instrumental jazz" and receive a ranked list of playable station streams. RockCast remains responsible for playback through its existing `PlaybackController`.

## Current status

The repository contains a Rust edition 2024 Axum service. `POST /v1/search` implements deterministic metadata search through a replaceable repository boundary. PostgreSQL is selected when `DATABASE_URL` is set; otherwise the same six-station catalog runs in memory. Versioned migrations create station, stream, provider-identity, and import-run storage while preserving the development catalog. A separate one-shot CLI can import a bounded Radio Browser slice into PostgreSQL; it is never called by search or HTTP startup. `GET /health/live` depends only on the process, while `GET /health/ready` checks PostgreSQL in database mode. There is no pgvector, LLM integration, authentication, rate limiting, stream probing, or RockCast integration yet.

## Intended architecture

The service will be built in small, independently testable layers:

- a versioned Axum HTTP API whose contract is defined in `api/openapi.yaml`;
- transport DTOs separated from search domain models;
- deterministic catalog filtering and ranking over PostgreSQL or the built-in fallback;
- PostgreSQL migrations for stations and playable streams;
- pgvector as a future semantic-ranking store, not part of the current persistence stage;
- provider traits for query parsing and embeddings, with deterministic fakes in tests;
- controlled Radio Browser import outside the request path, with stream probing still future work.

An LLM will convert natural-language requests into structured filters; it will not inspect the full station catalog. RockCast will retain its local catalog as a fallback if the service is unavailable.

See [docs/architecture.md](docs/architecture.md) for boundaries and the planned request flow. The standalone, offline [service diagrams](docs/service-diagrams.html) distinguish current behavior from the next and future stages.

## Roadmap

1. Repository hygiene and contributor documentation.
2. Axum HTTP skeleton with health endpoints, tracing, graceful shutdown, and router tests.
3. OpenAPI contract for `POST /v1/search`.
4. Deterministic in-memory search with domain/DTO separation — complete.
5. PostgreSQL persistence, migrations, development seed, and dependency-aware readiness — complete.
6. Controlled Radio Browser import outside the request path — complete.
7. Semantic ranking behind query-parser and embedding provider traits — next.
8. Small RockCast integration changes for remote search with local fallback.
9. Voice input only after text search is stable, followed by stream health checks, metrics, rate limiting, and deployment.

RS-006 is complete. The next work is semantic ranking behind query-parser and embedding provider traits plus pgvector; detailed acceptance criteria remain in [TODO.md](TODO.md).

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

## Import Radio Browser

The importer is a separate process and requires `DATABASE_URL`; it does not run from `POST /v1/search` or when the HTTP server starts. Run one bounded import with:

```text
DATABASE_URL=postgres://USER:PASSWORD@HOST:PORT/DATABASE cargo run --bin import_radio_browser
```

PowerShell example:

```text
$env:DATABASE_URL='postgres://USER:PASSWORD@HOST:PORT/DATABASE'
cargo run --bin import_radio_browser
```

Configuration is validated before the first request:

| Variable | Default | Allowed range or rule |
| --- | --- | --- |
| `DATABASE_URL` | none | Required; never logged. |
| `RADIO_BROWSER_BASE_URL` | `https://all.api.radio-browser.info` | HTTP(S) root with a host and no credentials, query, or fragment. |
| `RADIO_BROWSER_USER_AGENT` | `RockServer/0.1.0` | Non-empty valid HTTP header value, at most 128 bytes. |
| `RADIO_BROWSER_TIMEOUT_SECS` | `15` | 1–60 seconds per request. |
| `RADIO_BROWSER_PAGE_SIZE` | `100` | 1–500 upstream rows per page. |
| `RADIO_BROWSER_MAX_PAGES` | `10` | 1–100 pages per run. |

Every request sends the explicit User-Agent and asks `/json/stations/search` for `hidebroken=true`, `order=name`, an absolute `offset`, and the bounded `limit`. Responses are capped at 8 MiB. A short page ends the run; a full final page at `RADIO_BROWSER_MAX_PAGES` ends successfully at the configured safety boundary.

Only upstream rows with `lastcheckok=1`, a valid UUID, a non-empty normalized name, and a resolved HTTP(S) stream URL are imported. URLs with embedded credentials and invalid schemes are rejected; homepage failures only clear the optional homepage. Tags are lowercase, sorted, deduplicated, capped at 32 values and 64 characters each. Language normalization prefers the first valid two-letter code anywhere in `languagecodes`, regardless of its upstream position; only when none exists does it retain the first valid three-letter code in upstream order. This makes values such as `yue,zh` searchable by locale `zh`. A three-letter-only fallback remains useful metadata but cannot satisfy the current strict two-letter locale filter without a future ISO 639 mapping. A valid two-letter country code is retained. Codec labels are capped at 32 characters, and only bitrates from 1 through 2000 kbps are stored.

Ownership is `(source, source_station_id)` with source `radio_browser`; stream identity uses the same provider UUID. `CatalogImporter` rejects an entire page before upsert if any record source differs from the provider/run source, counts the normalized page as failed, and records a sanitized terminal error. The PostgreSQL store independently ties its explicit source argument to the still-started run and uses that value for writes instead of trusting record fields. Re-running the command updates that provider-owned station and stream in place. The six `builtin` stations are not provider-owned and are preserved. Missing upstream records are never deleted or disabled automatically. Each attempt creates an `import_runs` row with terminal `completed` or `failed` status, timestamps, counts, and a sanitized error summary. Logs include run ID, page progress, and final counts without DSNs or stream URLs.

The selected upstream fields and behavior follow the official Radio Browser [API usage guidance](https://docs.radio-browser.info/#using-the-api), [Station structure](https://docs.radio-browser.info/#station), [advanced station search](https://docs.radio-browser.info/#advanced-station-search), and [mirror discovery guidance](https://docs.radio-browser.info/#server-mirrors). The upstream implementation is maintained in the official [Radio Browser API repository](https://gitlab.com/radiobrowser/radiobrowser-api-rust).
