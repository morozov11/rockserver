# Project status

Last updated: 2026-08-13

## Current state

Stages 0–5 are complete in the current working tree: repository bootstrap, Axum HTTP skeleton, OpenAPI search contract, deterministic in-memory search, PostgreSQL persistence, and RS-006 controlled Radio Browser import.

`POST /v1/search` still uses the unchanged `StationRepository` boundary. PostgreSQL search executes deterministic metadata filtering/ranking in SQL; without `DATABASE_URL`, the HTTP service keeps the six-station in-memory fallback. Liveness remains process-only and readiness still checks only the selected search repository.

RS-006 adds independent `CatalogImportProvider` and `CatalogImportStore` boundaries plus provider-neutral import models. `RadioBrowserClient` is used only by the explicit `import_radio_browser` binary. It sends an explicit User-Agent, uses a configurable credential-free base URL, bounds request timeout, response bytes, page size, and page count, and deterministically maps valid upstream DTOs. The binary always requires PostgreSQL and is never invoked from search or HTTP startup.

The third migration adds `source`, source-specific station/stream identities, last-import references, and durable `import_runs`. Existing seed rows are owned by `builtin`; Radio Browser rows are owned by `radio_browser`. Repeated runs update provider-owned metadata and the primary stream in place. Missing upstream records are retained, so a partial page set or upstream outage cannot delete existing data.

## Configuration and behavior

- Generated `graphify-out/` artifacts remain local-only and ignored. This worktree had no local graph, so no graph was generated or updated.
- HTTP listener: `ROCKSERVER_BIND_ADDR`, default `127.0.0.1:3000`.
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- HTTP catalog backend: `DATABASE_URL` selects PostgreSQL; absence selects the six-station in-memory fallback.
- Import command: `cargo run --bin import_radio_browser`.
- Import database: `DATABASE_URL` is mandatory and is never logged.
- Radio Browser root: `RADIO_BROWSER_BASE_URL`, default `https://all.api.radio-browser.info`; HTTP(S), host required, no credentials/query/fragment.
- User-Agent: `RADIO_BROWSER_USER_AGENT`, default `RockServer/0.1.0`; non-empty valid header, at most 128 bytes.
- Request timeout: `RADIO_BROWSER_TIMEOUT_SECS`, default 15, range 1–60.
- Page size: `RADIO_BROWSER_PAGE_SIZE`, default 100, range 1–500.
- Maximum pages: `RADIO_BROWSER_MAX_PAGES`, default 10, range 1–100.
- Response body cap: 8 MiB per page.
- Upstream request: `/json/stations/search` with `hidebroken=true`, `order=name`, `reverse=false`, and explicit `offset`/`limit`.
- Accepted upstream rows: `lastcheckok=1`, valid UUID, non-empty normalized name, and credential-free resolved HTTP(S) stream URL.
- Optional normalization: valid HTTP(S) homepage; at most 32 sorted/deduplicated lowercase tags of at most 64 characters; first valid two-letter language code across the full upstream list, otherwise the first valid three-letter code in upstream order; valid two-letter country code; codec at most 32 characters; bitrate only from 1 through 2000 kbps.
- Run accounting: `started` becomes `completed` or `failed`; counts are fetched/imported/skipped/failed, and failures store at most 500 sanitized characters.
- Logs: run ID, page/offset progress, page counts, and final counts; no DSN, credentials, station URLs, or response bodies.
- PostgreSQL integration tests: `TEST_DATABASE_URL` must point to a disposable database and is used only by the explicitly ignored test.
- Migrations: embedded files in `migrations/` run automatically for both PostgreSQL HTTP startup and importer connection.
- Local database: `compose.yaml` provides PostgreSQL 17 with development-only defaults and a healthcheck.
- Public HTTP/OpenAPI behavior: unchanged by RS-006.

## Data ownership and failure behavior

Radio Browser station identity is `(radio_browser, stationuuid)` and stream identity uses the same provider UUID. RockServer station IDs are deterministically `rb-{canonical-uuid}`. Successful upsert updates provider-owned station metadata and one primary stream, including URL changes, without producing duplicates. Built-in rows are in a disjoint ownership namespace and are not modified by the importer.

Rows rejected by validation increment `skipped`. A source mismatch between any normalized record and the provider/run source rejects the whole page before upsert, increments `failed` by the normalized page size, and stores a generic terminal error. The PostgreSQL store also verifies that its explicit source belongs to the still-started run and uses that source for all writes rather than trusting record fields. A persistence batch failure rolls back that page, increments `failed` by the normalized batch size, marks the run failed, and stops. A provider request/status/JSON failure marks the run failed with counters accumulated through prior pages. Error summaries are deliberately generic and do not include configured endpoints, response bodies, DSNs, credentials, station fields, or stream URLs. No failure path automatically deletes catalog data.

The upstream contract choices are documented from the official Radio Browser [usage](https://docs.radio-browser.info/#using-the-api), [Station](https://docs.radio-browser.info/#station), [advanced search](https://docs.radio-browser.info/#advanced-station-search), and [mirror](https://docs.radio-browser.info/#server-mirrors) references plus the official [API repository](https://gitlab.com/radiobrowser/radiobrowser-api-rust).

## Known limitations

Pagination uses upstream `order=name` because the public API does not expose an order by station UUID. Radio Browser does not provide a transactional catalog snapshot, so concurrent upstream name/catalog changes can shift offset boundaries. The configured page/page-count ceiling intentionally imports a bounded slice rather than claiming a full mirror. Local sorting by canonical provider UUID makes each received page deterministic, and the no-delete policy makes partial runs safe.

`lastcheckok=1` is trusted as upstream metadata; RockServer does not probe streams in RS-006. Language prefers a two-letter candidate so it is compatible with locale-derived search filters. When upstream supplies only three-letter codes, the first valid one is retained but cannot match the current strict two-letter locale filter; a comprehensive ISO 639-2 to ISO 639-1 mapping is future work. An abrupt process kill or host loss can leave a run in `started`; there is no stale-run reconciler yet. Search remains exact metadata matching. There is no pgvector, semantic similarity, query-parser/embedding provider, LLM, authentication, rate limiting, scheduler, automatic retry, metrics, RockCast client integration, or stream health checker.

## Verification

On 2026-08-13, the regular all-target/all-feature suite passed with 25 tests plus the one opt-in PostgreSQL test ignored by default. Unit coverage includes DTO parsing, deterministic mapping, two-letter language preference with documented three-letter fallback, normalization, skip rules, configuration bounds, maximum pagination, partial-count failure recording, source-ownership rejection before upsert, terminal failure counts, and idempotent input equality. Local loopback mock HTTP tests verify User-Agent/query parameters, successful client parsing, and sanitized HTTP errors; ordinary tests make no external request.

Docker Engine 29.7.2 and Compose 5.3.1 were available. An isolated `rockserver-rs006-test` Compose project started PostgreSQL 17 on host port 55436. The opt-in integration test passed and verified all migrations, six built-in rows, first import, repeat update without station/stream duplication, changed metadata/URL, completed and failed `import_runs` counts/status/timestamps/error behavior, search over the imported station, and dependency-aware readiness. Only that project's container, network, and volume were removed afterward.

After review hardening added the run/source ownership SQL check, the same opt-in integration test passed again in a separate `rockserver-rs006-review-test` PostgreSQL 17 project on host port 55437. That project's container, network, and volume were removed afterward.

A read-only smoke request with `limit=1`, explicit `RockServer-Smoke/0.1.0` User-Agent, and 15-second timeout succeeded against `https://all.api.radio-browser.info`; it returned one row with a non-empty station UUID and `lastcheckok=1`. It did not connect to or write any database and is not part of the ordinary test suite.

Final formatting, strict Clippy, all-target/all-feature tests, diff whitespace, Compose validity, and HTML validity results are recorded in `docs/tasks.md` after completion.

## Next step

Add semantic ranking through query-parser and embedding provider traits with deterministic fakes and pgvector, while preserving the current HTTP contract and metadata-only fallback. LLMs must receive structured query work only, never the full catalog.
