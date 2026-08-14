# RockServer

RockServer is a planned Rust service for AI-assisted internet radio discovery. A RockCast user will be able to enter a request such as "calm instrumental jazz" and receive a ranked list of playable station streams. RockCast remains responsible for playback through its existing `PlaybackController`.

## Current status

The repository contains a Rust edition 2024 Axum service. `POST /v1/search` uses provider-neutral query interpretation, optional embeddings, and a replaceable repository boundary. PostgreSQL provides exact pgvector-backed hybrid ranking when compatible query and station embeddings exist; otherwise the established deterministic metadata ranking remains authoritative. Without `DATABASE_URL`, the same six-station catalog runs in memory with metadata fallback. Versioned migrations create catalog, import-run, and provenance-aware embedding storage. Separate one-shot CLIs import Radio Browser data and backfill embeddings outside HTTP startup and the request path. `GET /health/live` depends only on the process, while `GET /health/ready` checks only PostgreSQL in database mode. There is no production LLM/embedding provider, authentication, rate limiting, stream probing, or RockCast integration yet.

## Intended architecture

The service will be built in small, independently testable layers:

- a versioned Axum HTTP API whose contract is defined in `api/openapi.yaml`;
- transport DTOs separated from search domain models;
- deterministic hard filtering and metadata/hybrid ranking over PostgreSQL or the built-in fallback;
- PostgreSQL migrations for stations and playable streams;
- pgvector storage with model/version/dimension provenance and exact cosine search;
- provider traits for query parsing and embeddings, with deterministic fakes in tests and metadata-safe failure fallback;
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
7. Semantic ranking behind query-parser and embedding provider traits — complete.
8. Small RockCast integration changes for remote search with local fallback — next.
9. Voice input only after text search is stable, followed by stream health checks, metrics, rate limiting, and deployment.

RS-007 is complete. The next work is a small RockCast integration for remote text search with the existing local catalog retained as fallback; detailed acceptance criteria remain in [TODO.md](TODO.md).

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

With no `DATABASE_URL`, startup logs `backend=in_memory` and uses the built-in metadata fallback. To run the local pgvector-capable PostgreSQL backend with documented development-only defaults:

```text
docker compose up -d --wait
DATABASE_URL=postgres://rockserver:rockserver_dev@127.0.0.1:5432/rockserver cargo run
```

On PowerShell, set the variable with `$env:DATABASE_URL='postgres://rockserver:rockserver_dev@127.0.0.1:5432/rockserver'` before `cargo run`. Startup applies pending files from `migrations/`, including `CREATE EXTENSION vector`, and logs `backend=postgresql` without logging the URL. The PostgreSQL server must have pgvector installed and the migration role must be allowed to enable it. Stop the local database with `docker compose down`; add `-v` only when the development catalog and embeddings should also be discarded.

Check readiness and seeded search after startup:

```text
curl http://127.0.0.1:3000/health/ready
curl -X POST http://127.0.0.1:3000/v1/search -H "content-type: application/json" -d '{"query":"calm instrumental jazz"}'
```

The real database integration test is opt-in so ordinary tests need no Docker or network:

```text
TEST_DATABASE_URL=postgres://USER:PASSWORD@127.0.0.1:PORT/DISPOSABLE_DATABASE cargo test --test postgres_integration --all-features -- --ignored --exact postgres_migrations_seed_search_and_readiness
```

## Semantic search and embedding backfill

The public HTTP schemas are unchanged. `QueryParser` receives only the validated query and locale and returns structured terms, tags, language, and country filters; it never receives the catalog. The deterministic parser is both the default and the fallback if a future optional parser fails. `EmbeddingProvider` receives one query string at request time. Station embeddings are generated only by the controlled backfill workflow, one station document at a time.

RS-007 ships no production model. The only concrete embedder is an explicitly named deterministic development implementation. Enable it with matching settings for backfill and server startup:

```text
docker compose up -d --wait
DATABASE_URL=postgres://rockserver:rockserver_dev@127.0.0.1:5432/rockserver \
ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev \
ROCKSERVER_EMBEDDING_DIMENSION=32 \
cargo run --bin backfill_embeddings

DATABASE_URL=postgres://rockserver:rockserver_dev@127.0.0.1:5432/rockserver \
ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev \
ROCKSERVER_EMBEDDING_DIMENSION=32 \
cargo run
```

PowerShell uses the same three environment variables through `$env:NAME='value'`. `ROCKSERVER_SEMANTIC_PROVIDER` is optional for the HTTP service; when absent, search is metadata-only. `ROCKSERVER_EMBEDDING_DIMENSION` defaults to 32 and supports 1 through 16,000 for pgvector's unbounded `vector` storage. Model and preprocessing identity are persisted as `rockserver-deterministic-dev`, version `1`, and the configured dimension. Changing dimension with the same model/version replaces those station rows on the next backfill. This implementation is a repeatable development fixture, not a semantic production model.

Migration `0004_add_station_embeddings.sql` uses an unbounded `vector` column plus model/version/dimension fields and CHECK constraints, so the schema is not locked to the deterministic fixture's dimension. Searches compare only exact matching provenance and currently use exact cosine distance; there is deliberately no dimension-specific HNSW/IVFFlat index at this foundation stage. Cosine similarity is normalized to `[0,1]`, then a compatible pair uses `0.70 * metadata_score + 0.30 * semantic_score`. A station without a compatible embedding retains its full metadata score. With no valid query embedding, the original metadata filter/ranker is used unchanged. Hard language/country filters and exclusions are applied before scoring and final limit; `station_id ASC` remains the last tie-break.

Parser or embedding provider failures are logged and fall back to metadata instead of causing a 5xx when the repository can answer. Readiness continues to reflect only the selected repository/database; optional semantic providers do not affect it. Liveness remains process-only.

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
