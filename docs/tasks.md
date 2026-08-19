# Task log

## RS-019 — 2026-08-19 — Genre hierarchy and progressive search fallback

- Goal: make subgenre queries (e.g. "heavy metal", "black metal", "smooth jazz") find stations tagged with parent genres when no exact match exists, and prefer stations matching the requested language.
- Scope: expanded canonical tag vocabulary with subgenres from the real Radio Browser catalog; a static genre-parent hierarchy (`black metal` → `metal` → `rock`); ancestor-aware `station_matches_requested_genre`; progressive fallback in `SearchService::search` (exact → parent → drop genre); a new builtin catalog station for metal; deterministic tests including the `"english heavy"` use case; and documentation updates. No public HTTP API, OpenAPI, PostgreSQL SQL, migration, or embedding changes.
- Result: `CANONICAL_TAGS` grew from 27 to 40 entries with metal subgenres (`black metal`, `death metal`, `doom metal`, `power metal`, `symphonic metal`, `thrash metal`), rock subgenres (`alternative rock`, `classic rock`, `indie rock`, `pop rock`, `progressive rock`, `psychedelic rock`, `soft rock`), and jazz subgenres (`acid jazz`, `latin jazz`, `smooth jazz`). `genre_parent` and `genre_ancestors` expose the hierarchy. `station_matches_requested_genre` accepts stations whose tags match any ancestor. `SearchService::search` progressively relaxes the genre constraint: exact match → parent genres → no genre filter, always gated by `MIN_RELEVANCE_SCORE`.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 67 regular tests (48 unit + 19 integration), 6 ignored live/database tests.
- Status: complete.

## RS-016 — 2026-08-17 — Local ONNX multilingual semantic search

- Goal: add an opt-in CPU/local ONNX embedding provider suitable for Russian and English station discovery.
- Result: selected `intfloat/multilingual-e5-small` (384 dimensions), added local ONNX Runtime/tokenizer inference, E5 query/document prefixes, normalized station text, importer maintenance, backfill support, and a provenance-specific pgvector HNSW cosine index.
- Verification: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` pass with 50 regular tests. The local PostgreSQL database was migrated, 999 real Radio Browser stations were imported, all 1,005 catalog rows received E5 embeddings, and three Russian live queries ran through the HTTP search path. Jazz and 80s-rock ranked relevant stations first; the classical/instrumental query exposed a catalog/locale-filter quality limitation.
- Status: complete; local model assets remain intentionally outside Git.

## RS-000 — 2026-08-13 — Repository bootstrap

- Goal: establish the Rust crate, repository hygiene, contributor guidance, project scope, architecture outline, and near-term roadmap.
- Scope: Rust edition 2024 bootstrap binary and documentation only.
- Result: completed by commit `4117786` (`Document RockServer project setup`); the commit added the crate files, ignore rules, contributor instructions, README, TODO, and architecture outline.
- Checks: the commit and resulting files were inspected for this log entry; historical command output is not available in Git and is therefore not claimed.
- Status: complete.

## RS-001 — 2026-08-13 — HTTP service skeleton

- Goal: introduce a minimal, testable Axum HTTP service with operational health endpoints.
- Scope: library plus thin binary, stable JSON health models, request and application tracing, configurable local listener, Ctrl+C graceful shutdown, in-memory router tests, and project documentation. Search, OpenAPI, persistence, containers, external providers, and client work are excluded.
- Result: added the Axum library application and thin binary, liveness and readiness routes with a shared stable serde model, JSON application tracing and HTTP request spans, `ROCKSERVER_BIND_ADDR` configuration with the local-only `127.0.0.1:3000` default, an extensible graceful-serving boundary with Ctrl+C as the current signal, and in-memory router tests. Updated the contributor rules and current project documentation.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 2 tests; `git diff --check` passed.
- Status: complete.

## RS-002 — 2026-08-13 — Search API contract and service diagrams

- Goal: define the public search contract before implementation and provide a clear offline architecture reference.
- Scope: OpenAPI 3.1 contract for `POST /v1/search` and existing health routes; validation rules, defaults, response/error schemas, status semantics, and Russian and English examples; a structural contract test; autonomous Russian HTML diagrams; contributor and project documentation. Search routing, catalog/domain implementation, persistence, providers, ranking, authentication, and client changes are excluded.
- Result: added `api/openapi.yaml` with the required schemas and 200/400/422/429/500 outcomes, explicitly separating malformed JSON (400) from well-formed but invalid input (422). Added a `cargo test` integration check using test-only `serde_yaml`; it parses the YAML and guards the required OpenAPI version, paths, POST operation, and schemas. Added `docs/service-diagrams.html`, which distinguishes CURRENT, NEXT, and FUTURE architecture and works without external resources. The Axum router was intentionally unchanged, so `POST /v1/search` remained 404 at that stage. Added permanent Rustdoc/comment guidance and a diagram-accuracy rule to `AGENTS.md`.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 4 tests, including an explicit assertion that `POST /v1/search` remained 404; `git diff --check` passed. Python's standard-library `html.parser` accepted the complete HTML file and additional local checks confirmed one doctype, one `<html>` root, no external resource URLs, and balanced SVG elements.
- Status: complete.

## RS-003 — 2026-08-13 — Deterministic in-memory station search

- Goal: make the contract-defined `POST /v1/search` route usable without persistence, providers, embeddings, LLMs, or external network access.
- Scope: Axum search routing and DTO validation; contract-compliant 400/422 errors with request IDs; a small built-in in-memory catalog; separated search domain, `StationRepository` trait, normalization, filtering, deterministic ranking, and result mapping; HTTP and domain tests; current-state documentation and diagrams. PostgreSQL, imports, external providers, semantic ranking, rate limiting, client changes, and authentication are excluded.
- Result: registered `POST /v1/search` alongside the unchanged health routes. The route accepts all existing request fields, defaults locale and limit, rejects malformed JSON with 400 and syntactically valid invalid input with 422, and always returns the standard `ErrorResponse` fields on errors. The domain applies inferred language/country and exclusion constraints to a six-station catalog, scores matching metadata, and breaks equal scores by station ID ascending. No OpenAPI changes were needed because the existing schema already described this behavior.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 13 tests; `git diff --check` passed. Search tests cover success, empty results, limit, stable ordering/tie-break, exclusions, malformed JSON 400, and semantic validation 422. The suite uses no network listener, external network, AI provider, or LLM. A standard-library HTML parser accepted the updated service diagrams.
- Status: complete.

## RS-004 — 2026-08-13 — PostgreSQL persistence foundation

- Goal: add real PostgreSQL catalog storage behind `StationRepository` without changing the public search contract or introducing import, pgvector, LLM, authentication, or rate limiting.
- Scope: versioned station/stream migrations, constraints and indexes, idempotent development seed, SQLx PostgreSQL repository, automatic startup migrations, `DATABASE_URL` selection with in-memory fallback, backend logging without DSN disclosure, dependency-aware readiness, local PostgreSQL Compose, deterministic unit tests, real opt-in PostgreSQL integration coverage, OpenAPI readiness alignment, and current-state documentation.
- Result: added separate `stations` and `station_streams` tables with timestamps and catalog invariants; seeded the same six development stations as the fallback; implemented parameterized SQL for hard filters, exclusions, deterministic metadata scoring, score-descending/station-ID-ascending order, and limit; made search repository operations asynchronous; added startup backend selection and migrations; kept liveness process-only and made PostgreSQL readiness return 503/`not_ready` after dependency loss. The responsibility boundary is explicit: the domain owns normalized inputs and rule meaning, while each repository executes those rules efficiently in its storage model.
- Checks: Docker Engine 29.7.2 and Compose 5.3.1 were available. A dedicated `rockserver-rs004-test` project ran PostgreSQL 17 and the opt-in integration test passed for migrations, seed, HTTP search, exclusions, limit, tie-break, and readiness loss; only that project's container, network, and volume were removed afterward. `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 15 regular tests and the opt-in PostgreSQL test ignored by default; the same PostgreSQL test passed when explicitly run with `TEST_DATABASE_URL`; `git diff --check` passed; the standard-library HTML parser accepted `docs/service-diagrams.html`.
- Status: complete.

## RS-005 — 2026-08-13 — Keep Graphify artifacts local

- Goal: prevent generated Graphify output from being included in commits or pushed to GitHub while preserving the local knowledge graph.
- Scope: root ignore rule and Git-index cleanup only; no Graphify files were deleted from disk.
- Result: normalized `.gitignore` to one `/graphify-out/` rule and removed all staged Graphify artifacts from the Git index. No `graphify-out` path existed in `HEAD`, so no committed repository content required deletion.
- Checks: `git ls-files -- graphify-out` returned no paths; `git check-ignore -v` matched both root and nested Graphify artifacts; the local directory remained present with 43 files; `git diff --check -- .gitignore docs/status.md docs/tasks.md` passed.
- Status: complete.

## RS-006 — 2026-08-13 — Controlled Radio Browser import

- Goal: add a controlled, repeatable Radio Browser import into PostgreSQL outside the HTTP request path and server startup, without pgvector, embeddings, LLMs, authentication, rate limiting, RockCast work, or stream probing.
- Scope: provider-neutral import models and orchestration; an import-provider trait independent of `StationRepository`; a bounded Radio Browser HTTP client; deterministic DTO validation/normalization; an import-only PostgreSQL persistence trait; provider ownership and stream identity; durable `import_runs`; a one-shot CLI; unit, local mock HTTP, and opt-in PostgreSQL integration coverage; current-state documentation and diagrams. The public OpenAPI/search contract, liveness, and readiness semantics remain unchanged.
- Result: added migration `0003_add_catalog_import.sql`, which preserves the six seed stations as `builtin`, adds source identities for station/stream idempotency, removes global stream-URL uniqueness in favor of provider stream identity, and adds terminal import-run status/count/timestamp/error storage. Added `CatalogImporter`, `CatalogImportProvider`, and `CatalogImportStore` boundaries. Added `RadioBrowserClient` with explicit User-Agent, official DNS-balanced default root, configurable safe root, 1–60 second timeout, 1–500 page size, 1–100 maximum pages, 8 MiB response cap, deterministic per-page UUID order, and sanitized failures. Added `PostgresImportStore` transactional upserts and `import_radio_browser`; `DATABASE_URL` is mandatory for that binary and no DSN, stream URL, or response body is logged.
- Mapping rules: consumes official `stationuuid`, `name`, `url_resolved`, `homepage`, `tags`, `languagecodes`, `countrycode`, `codec`, `bitrate`, and `lastcheckok`. Rows are skipped unless the upstream check is good, UUID/name are valid, and resolved stream URL is credential-free HTTP(S). Optional homepage is validated; tags are lowercase/sorted/deduplicated and bounded; language prefers the first valid two-letter code across the full list and falls back to the first valid three-letter code only when necessary; valid alpha-2 country, bounded codec, and 1–2000 kbps bitrate are retained. Repeat runs update `(radio_browser, stationuuid)` and its provider-owned primary stream without duplicates. Missing upstream rows are retained.
- Sources: behavior and fields were verified against the official Radio Browser [API usage](https://docs.radio-browser.info/#using-the-api), [Station](https://docs.radio-browser.info/#station), [advanced search](https://docs.radio-browser.info/#advanced-station-search), and [mirror discovery](https://docs.radio-browser.info/#server-mirrors) documentation and the official [API repository](https://gitlab.com/radiobrowser/radiobrowser-api-rust).
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 25 regular tests and one opt-in PostgreSQL test ignored by default; `git diff --check` passed. Unit tests cover mapping, language preference/fallback, normalization, skip rules, bounds, pagination, partial failure accounting, source mismatch rejection before upsert, terminal failure counts, and deterministic/idempotent input. Local loopback mock HTTP tests cover request headers/query, parsing, and sanitized HTTP errors with no real external call. `docker compose config --quiet` passed. Python's standard-library `html.parser` accepted `docs/service-diagrams.html`; checks also confirmed one doctype/root, balanced SVG elements, and no external resource URL.
- Real integration: Docker Engine 29.7.2 and Compose 5.3.1 ran a dedicated `rockserver-rs006-test` PostgreSQL 17 project on host port 55436. The ignored integration test passed for migrations, first import, repeat update without station/stream duplicates, six-seed preservation, completed/failed run counts/status/timestamps/error summary, search over imported metadata, and readiness. Only that project's container, network, and volume were removed afterward.
- Smoke: a read-only official API request with `limit=1`, a descriptive `RockServer-Smoke/0.1.0` User-Agent, and a 15-second timeout succeeded and returned one checked station with a non-empty UUID. It wrote no database and is not in the ordinary suite.
- Known limitations: upstream name-ordered offset pagination is not a transactional snapshot; configured bounds intentionally produce a safe slice, not a full mirror. No-delete ownership prevents partial runs from removing data. A hard process/host failure can leave `started` stale because reconciliation is not yet implemented. `lastcheckok` is trusted without local probing, and three-letter-only language metadata still cannot match a strict locale-derived two-letter filter without a future ISO 639 mapping.
- Status: complete.

## RS-006 review hardening — 2026-08-13 — Language and source invariants

- Goal: resolve merge-blocking compatibility between imported language codes and locale-derived search filters, and prevent provider records from crossing import-run source ownership.
- Scope: deterministic language selection, orchestration ownership validation, run-bound PostgreSQL source enforcement, focused tests, and documentation. No migration or public HTTP/OpenAPI change.
- Result: language normalization now scans the complete list and prefers the first valid two-letter code regardless of position; only a three-letter-only list falls back to its first valid three-letter code. `CatalogImporter` rejects a mismatched-source page before upsert, marks the normalized page failed, and records a generic terminal error. `PostgresImportStore` verifies the explicit source against the still-started run and uses that value for writes rather than record fields.
- Checks: focused tests cover `yue,zh -> zh`, `EN,eng -> en`, deterministic three-letter-only fallback, no-upsert ownership rejection, sanitized error text, and terminal failed counts/status. Final formatting, strict Clippy, all-target/all-feature tests, and diff checks passed.
- Real integration: because runtime SQL changed, the opt-in PostgreSQL test passed again in isolated Compose project `rockserver-rs006-review-test` on port 55437; only that project's container, network, and volume were removed afterward.
- Status: complete.

## RS-007 — 2026-08-14 — Semantic ranking foundation

- Goal: add a minimal working semantic-search foundation behind domain and repository boundaries without adding a production LLM/model provider, changing the public HTTP schemas, or moving catalog/provider network work into `POST /v1/search`.
- Scope: provider-neutral `QueryParser` and `EmbeddingProvider`; deterministic parser/failure fallback and deterministic fakes; explicit non-production development embedder; pgvector migration and provenance-aware station embedding store; controlled one-shot backfill/update CLI; PostgreSQL exact hybrid search; in-memory metadata fallback; unit, HTTP, and opt-in real PostgreSQL coverage; Compose, OpenAPI description, architecture/status/roadmap/diagram updates. Authentication, rate limiting, RockCast, voice input, stream probing, scheduler, production providers, and ANN tuning are excluded.
- Result: `SearchService` now interprets validated request-only input and obtains an optional query embedding before crossing `StationRepository`. Providers cannot receive a station collection through their interfaces. Parser errors or invalid structured filters use the deterministic parser; embedding errors omit semantic input. The in-memory repository continues to use the original deterministic metadata ranker. PostgreSQL applies language/country filters and exclusions before score/limit, joins only matching model/version/dimension provenance, performs exact cosine similarity, and orders score descending with station ID ascending as the last tie-break.
- Ranking decision: domain constants own weights. Compatible embeddings use normalized cosine `1 - cosine_distance / 2` and `0.70 * metadata_score + 0.30 * semantic_score`. Missing compatible station embeddings retain full metadata score; missing/failed query embeddings use metadata-only inclusion and scoring. SQL receives the domain weights as bind parameters rather than duplicating literals.
- Persistence decision: migration `0004_add_station_embeddings.sql` enables pgvector and creates a separate table keyed by station/model/version with dimension, unbounded `vector`, and timestamps. Dimension and non-zero norm are CHECK-enforced. The schema is not locked to the test/dev dimension; no HNSW/IVFFlat index is added before a production model and scale measurements exist. SQLx queries remain runtime-checked and require no live database at ordinary build time.
- Validation decision: an `Embedding` can be constructed only through its validating constructor; its provenance and vector values are private and exposed read-only so a provider cannot bypass model, dimension, finite-value, or non-zero-norm checks before the repository boundary.
- Workflow decision: `backfill_embeddings` requires PostgreSQL and explicit `ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev`, pages by stable station ID, embeds one station document at a time, and idempotently upserts provenance-specific rows. It runs outside HTTP startup/search and does not alter Radio Browser ownership/import behavior. The deterministic hash embedder is documented as a development fixture, not a production semantic model.
- Checks: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test --all-targets --all-features` passed with 35 regular tests and one opt-in PostgreSQL test ignored by default; `git diff --check` passed. `docker compose config --quiet` passed with the pgvector PostgreSQL 17 image. Python's standard-library HTML parser accepted `docs/service-diagrams.html`; checks confirmed one doctype/root, balanced SVG elements, and no external resource URL. Unit coverage includes parser boundaries/normalization, invalid intent fallback, embedding model/dimension/finite/non-zero validation, deterministic fake embeddings, provider-failure metadata fallback, controlled backfill, fixed hybrid weights, exclusions, limit, and stable tie-break.
- Real integration: Docker Engine 29.7.2 and Compose 5.3.1 ran isolated pgvector projects on ports 55438 and, after final runtime-SQL weight binding, 55439. The final ignored integration test passed for extension/migrations, embedding insert/repeat update, provenance/dimension storage, exact cosine similarity, hard filters, exclusions, final limit, semantic station-ID tie-break, metadata fallback, HTTP search, and repository-only readiness. The real deterministic backfill also completed twice and produced seven unique development-provenance rows. Only the created projects' containers, networks, and volumes were removed.
- Graphify: after deleting only the verified task-created `C:\repos\rockserver\target-rs007`, `graphify update .` completed. The permitted `tree-sitter-sql` extra was installed in Graphify's existing isolated `uv tool` environment; the final code update rebuilt the ignored local graph to 517 nodes, 1066 edges, and 30 communities without SQL-parser warnings. Remaining tool warnings are a skill/package version mismatch (`0.9.35` instructions versus `0.9.40` package), `hooks.json` producing no nodes, and stale community labels; optional LLM relabeling or semantic extraction of changed documentation was not run. Generated `graphify-out/` files remain untracked.
- Known limitations: exact vector scan has no ANN index; deterministic-dev has no production semantic quality; backfill recomputes all station documents and has no durable resume/unchanged-input skip; no model registry enforces one dimension globally per model/version; optional providers have no production authentication/retry/circuit breaker. Existing RS-006 upstream/import limitations remain.
- Status: complete.

## Roadmap decision — 2026-08-14 — Windows-first voice production path

- Goal: preserve the agreed delivery order in the repository so implementation follows the Windows RockCast production workflow rather than beginning with ESP32.
- Scope: planning documentation only. No server API, database schema, provider, RockCast code, or runtime behavior changed.
- Result: added `docs/windows-production-roadmap.md`; the plan verifies RS-007 on disposable local PostgreSQL first, integrates RockCast text search with local fallback, adds Windows microphone capture, adds a RockServer voice/STT path that reuses `SearchService`, introduces production providers, completes Windows end-to-end and operational hardening, and leaves ESP32 in future backlog. Updated the near-term TODO and current status to point to the same ordered plan. Database credentials remain environment-only and are not committed.
- Checks: documentation paths and Git diff were inspected; `git diff --check` passed. Rust checks were not required because no Rust, OpenAPI, dependency, migration, or runtime behavior changed.
- Status: planned.

## RS-007 verification — 2026-08-14 — Local PostgreSQL and HTTP smoke

- Goal: re-verify the completed semantic-ranking foundation on the user-designated disposable local `rockserver` database before beginning RockCast integration.
- Scope: database reset, clean migrations, the existing ignored PostgreSQL integration test, deterministic embedding backfill idempotency, database inspection, and a local HTTP smoke check. No source code, OpenAPI, migration, or runtime behavior changed.
- Result: connected to PostgreSQL 18.1 with pgvector 0.8.6, reset only the `public` schema of database `rockserver`, and successfully applied migrations 1–4. The exact ignored integration test passed. Two consecutive `deterministic-dev` backfills produced seven unique 32-dimensional embeddings for seven stations and zero duplicate station/model/version keys. The test's four 3-dimensional `integration-model` fixtures also remained valid. A temporary server on loopback reported `live=ok` and `ready=ok`; `POST /v1/search` for `jazz` returned hybrid-ranked results headed by `station-jazz-002` with reason `Hybrid match: metadata 1.000, semantic 0.878.` The temporary server was stopped after the smoke check.
- Security: the database password and connection URL were supplied only through process environment variables and were not written to repository files.
- Checks: `cargo test --test postgres_integration --all-features -- --ignored --exact postgres_migrations_seed_search_and_readiness --nocapture` passed with one test and no failures; migration, row-count, provenance, dimension, and duplicate-key queries passed; `git diff --check` passed.
- Status: complete.

## RS-008 — 2026-08-14 — Windows voice-command API contract

- Goal: stabilize the JSON contract used by the Windows voice-command flow after RS-007 without adding a second search implementation, audio upload, STT, or provider credentials.
- Scope: canonical OpenAPI/Axum `POST /api/v1/voice/command`, deprecated `/v1/voice/command` compatibility alias, transcript DTOs, selected-station response semantics, request IDs, validation, request-size and service-time boundaries, contract tests, and current-state documentation. Existing `POST /v1/search` request/response fields remain compatible.
- Result: the canonical route accepts only a bounded, already-recognized `transcript` plus locale, limit, and station exclusions, then calls the existing `SearchService`. It returns the trimmed transcript, normalized query, deterministic `stations`, and `selected_station` equal to the first result or `null`. Valid `X-Request-Id` values are echoed in the response body and header; otherwise the server generates one. JSON bodies are limited to 65,536 bytes; malformed input returns 400, oversized input 413, validation 422, unexpected search failure 500, and the five-second interpretation/search deadline returns structured 504. No audio, STT, LLM, or provider credential is accepted or simulated.
- Compatibility decision: `/api/v1/voice/command` is canonical for new Windows clients. `/v1/voice/command` maps to the same handler and DTOs but is documented as deprecated so the repository rule for `/v1` public versioning and the Windows `/api/v1` contract can coexist. Existing `/v1/search` remains registered with its existing body schemas; it now also uses the shared bounded JSON/error/request-ID transport behavior.
- Tests: `cargo fmt --check` passed; `cargo clippy --all-targets --all-features -- -D warnings` passed; `cargo test` passed with 40 regular tests and one opt-in PostgreSQL test ignored by default. Contract coverage verifies the canonical route, compatibility alias, request-ID propagation, selected/no-result behavior, transcript validation, structured 413 size failure, and deterministic 504 timeout.
- Graphify: `graphify update .` rebuilt the ignored local code graph to 559 nodes, 1,165 edges, and 32 communities. The tool reported only its pre-existing `hooks.json` zero-node warning and stale community labels; generated graph artifacts remain untracked.
- Documentation: `api/openapi.yaml` is the source of truth; `docs/status.md`, `docs/architecture.md`, `docs/windows-production-roadmap.md`, `README.md`, and `docs/service-diagrams.html` now distinguish the implemented JSON boundary from still-future microphone, audio-upload, and STT work.
- Status: complete.

## RS-009 — 2026-08-14 — Provider-neutral streaming voice API

- Goal: add a stable bidirectional audio contract without coupling RockCast to Yandex or OpenAI protocols and without changing the RS-008 JSON endpoint.
- Scope: canonical WebSocket `GET /api/v1/voice/stream`, deprecated `/v1/voice/stream` alias, PCM16 mono start/audio/commit protocol, incremental and final transcript events, final `SearchService` resolution, request IDs, size/time/error boundaries, provider traits, OpenAPI, deterministic tests, and current-state documentation. Real provider credentials and external-network tests are excluded.
- Result: added `StreamingSpeechRecognizer` and isolated per-client `SpeechStreamSession` boundaries. The WebSocket accepts 8/16/24/48 kHz PCM signed 16-bit little-endian mono chunks, limits each chunk to 65,536 bytes and the session to 10 MiB, emits `ready`, `transcript`, and terminal `result`/`error` events, and applies ten-second provider-operation and five-second search boundaries. The final transcript reuses the existing query interpretation, ranking, exclusions, and selected-station behavior. Default startup fails closed with `speech_provider_unavailable` until a concrete provider is configured.
- Tests: a real loopback WebSocket upgrade test injects a deterministic fake provider, verifies request-ID propagation, binary audio delivery, partial/final transcripts, and final station selection. OpenAPI structural coverage verifies both routes and all client/server event schemas. `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed with 41 regular tests and one opt-in PostgreSQL test ignored. Diff whitespace and the standalone HTML parser check passed. No external provider or internet call occurs.
- Graphify: `graphify update .` rebuilt the local ignored graph to 620 nodes, 1,339 edges, and 33 communities. The existing `hooks.json` zero-node warning remains, and labels are stale after the new community was introduced.
- Known limitations: this task stabilizes the service protocol and provider seam; it does not yet decode production audio because neither Yandex SpeechKit nor OpenAI Realtime is configured in `main`.
- Status: complete.

## RS-010 — 2026-08-14 — Yandex AI Studio structured intent parser

- Goal: add a production Yandex AI Studio adapter that turns text and already-recognized voice commands into the existing `QueryIntent`, without creating a second search path or changing public HTTP schemas.
- Scope: provider-neutral `LlmProvider` and `LlmQueryParser`; synchronous Yandex completion adapter; environment-only configuration and optional ignored local `.env`; strict JSON Schema plus local response validation; bounded HTTP timeout/body/token handling; deterministic failure fallback; loopback-only provider tests; and current-state documentation.
- Result: startup selects `YandexLlmProvider` only when both `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` are present. With neither value, it preserves `DeterministicQueryParser`; partial settings are a safe configuration error naming only variables. The Yandex request uses documented `Api-Key` authorization and `gpt://<folder>/<model>/latest`, fixed system rules, a JSON-encoded `{command, locale}` user message, and `jsonSchema` for the existing terms/tags/language/country intent. No station data crosses the provider boundary. Non-2xx, timeout, malformed/oversized response, malformed intent JSON, and invalid hard filters all keep the existing deterministic fallback. This parser is unrelated to SpeechKit/STT and is used by both text search and already-recognized voice commands through `SearchService`.
- Configuration: `.env.example` contains empty secret placeholders only. `dotenvy` may load an ignored local `.env`; production still reads process environment. Optional `YANDEX_LLM_MODEL` defaults to `yandexgpt`; `YANDEX_LLM_TIMEOUT_MS` defaults to 3000 and permits 100–10,000.
- Tests and checks: `cargo fmt --check`, strict all-target/all-feature Clippy, `cargo test` (47 regular tests passed; one opt-in PostgreSQL test ignored), and `git diff --check` passed. Provider tests use loopback Axum mocks and deterministic fake credentials; no test calls Yandex or reads a real `.env`. `graphify update .` was invoked but the installed Graphify environment failed before rebuilding with `No module named 'networkx.readwrite'`; generated graph artifacts remain absent and no global dependency was installed.
- Status: complete.

## RS-011 — 2026-08-14 — Opt-in real SpeechKit voice integration test

- Goal: add a manual live check for an actual recorded voice command sent to Yandex SpeechKit, with operational logs that do not disclose credentials or voice content.
- Scope: ignored integration test only; synchronous SpeechKit v1 recognition for a local mono Ogg/Opus recording; bounded request/response handling; deterministic environment validation; safe tracing; and current-state documentation. No public API, streaming protocol, or production STT adapter changes.
- Result: `tests/yandex_speechkit_live.rs` loads configured values only at explicit test execution, posts the recording to SpeechKit, verifies a 200 JSON result and normalized expected-command containment, and logs only test ID/status/timing/character count/match boolean. `generate_speechkit_fixture` explicitly synthesizes the committed Ogg/Opus fixture and its expected phrase with Yandex TTS; it requires only `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID`. The live STT test uses those committed files by default, accepts optional audio/phrase overrides, and remains ignored because it is billable.
- Tests and checks: `cargo fmt --check`, strict all-target/all-feature Clippy, `cargo test` (the live test is correctly ignored), and `git diff --check` passed. `graphify update .` was invoked but the installed Graphify environment failed before rebuilding with `No module named 'networkx.readwrite'`; no global dependency was installed. A live invocation is not run unless the user explicitly requests it with configured credentials and a real local recording.
- Status: complete; live execution remains pending the local audio input.

## RS-012 вЂ” 2026-08-14 вЂ” Generate committed SpeechKit fixture

- Goal: create the repository's real voice-command fixture through Yandex TTS using the configured environment without exposing credentials.
- Result: added opt-in `generate_speechkit_fixture`, which sends only the fixed command text to the documented TTS v1 endpoint, bounds and validates the Ogg response, and writes the Ogg fixture and expected transcript only after success. It uses service-account API-key authentication and does not send or require `folderId`, as SpeechKit derives the folder from the key's service account. The live STT test now uses these committed files by default and retains optional overrides. Two explicit TTS runs, before and after that correction, returned HTTP 401; no audio fixture was generated and no secret or provider body was logged.
- Status: complete. After local authorization was corrected, the generated Ogg fixture was sent to the live STT test on 2026-08-14 and returned HTTP 200 with a normalized expected-command match. No credential or transcript was logged.

## RS-013 вЂ” 2026-08-14 вЂ” Safe SpeechKit fixture diagnostics

- Goal: make the failed live TTS request inspectable without exposing the configured API key.
- Result: `YANDEX_SPEECHKIT_DEBUG=1 cargo run --bin generate_speechkit_fixture` prints the fixed non-secret request fields, redacted authorization, response status/headers, and a 16 KiB bounded redacted non-success response body. It also removes provider-echoed key-like fragments. It intentionally does not print the credential or raw unbounded provider content.
- Status: complete.

## RS-014 вЂ” 2026-08-14 вЂ” Opt-in STT mismatch diagnostics

- Goal: make a live SpeechKit recognition mismatch directly inspectable by the developer.
- Result: `TEST_YANDEX_STT_DEBUG=1` logs the endpoint, non-secret query fields, audio path and size, expected transcript, raw successful JSON response, and recognized transcript. The `Authorization` header is represented only as `Api-Key [REDACTED]`.
- Status: complete.

## RS-015 — 2026-08-14 — Structured voice-command interpretation

- Goal: turn final SpeechKit text into a strict typed player command through Yandex structured output, while keeping the LLM provider replaceable and ordinary tests offline.
- Scope: serde-backed `VoiceCommand`, `Intent`, and `RadioQuery`; provider-neutral `CommandInterpreter`; strict Yandex-compatible JSON Schema; semantic validation and normalization; secret-safe bounded HTTP diagnostics; unit coverage; and a separate ignored `yandex_llm_live` target with calm-jazz, Russian-rock, next-station, and volume exact tests.
- Result: `LlmCommandInterpreter` sends only transcript and locale through the existing `LlmProvider`, deserializes the provider JSON directly, and rejects invalid intent/field combinations without an heuristic parser. Yandex diagnostics log method, endpoint, redacted authorization/folder, request body, status, and bounded response body; configured API keys and folder IDs are removed. Yandex requires every schema field to be listed as required, with nullable values used for intent-specific omissions; both voice-command and existing search-intent schemas now follow that constraint.
- Checks: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed with 50 regular tests, four Yandex LLM live tests ignored, the existing SpeechKit live test ignored, and the PostgreSQL integration ignored. `cargo test --test yandex_llm_live -- --ignored --exact interprets_real_voice_command_with_safe_logs --nocapture` passed live with HTTP 200 and a typed calm-jazz command; logs exposed no credential or folder identifier. `graphify update .` rebuilt the local graph to 774 nodes, 1,692 edges, and 37 communities, with only the known zero-node `hooks.json` warning and stale community labels.
- Status: complete.

## RS-017 — 2026-08-18 — Application API Bearer gate

- Goal: establish the first fail-closed API access boundary before adding persisted RockCast
  clients and Bearer-based administrator sessions.
- Scope: production token configuration, HTTP and WebSocket authorization gate, OpenAPI security
  declaration, deterministic tests, and current-state documentation. No cookies, CSRF flow,
  database credential tables, or admin UI are included in this slice.
- Result: production startup requires `ROCKSERVER_API_BEARER_TOKEN` with at least 32 characters.
  All search and voice application routes require `Authorization: Bearer <token>`, including
  validation before a WebSocket upgrade. Missing, malformed, and invalid credentials receive the
  standard structured `401 authentication_required` response with `WWW-Authenticate: Bearer`.
  Liveness and readiness remain unauthenticated for supervision. The OpenAPI document declares
  the opaque Bearer scheme and each protected operation's 401 response.
- Documentation follow-up: updated `docs/service-diagrams.html` to show public health versus
  Bearer-protected application routes, current RockCast voice authorization, `401` semantics,
  and the separate NEXT state for persisted client/admin identity and the HTMX console.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`, and `git diff --check` passed. The ordinary suite ran 56 tests successfully;
  PostgreSQL and provider-credential-dependent live tests remained explicitly ignored.
- Status: complete.

## RS-020 — 2026-08-19 — Genre DB, stream probe, multi-tag import

- Goal: store the genre hierarchy in the database, add live stream probing, expand the import to pull stations by tag, and filter degraded streams from search.
- Scope: migration 0006 (`genre_hierarchy` table with ~250 canonical genres/subgenres); migration 0007 (`last_probe_at`/`last_probe_error` on `station_streams`); `GenreTaxonomy` struct loadable from PostgreSQL with builtin fallback; `probe_streams` binary (8s timeout, 50 concurrency); `RadioBrowserClient::with_tag()` and `RADIO_BROWSER_TAGS` env; `health <> 'degraded'` filter in search SQL; persistence helpers for taxonomy loading and stream health updates.
- Result: all 52 unit tests pass. fmt/clippy/test clean. Backward-compatible static API preserved.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test` — all clean.
- Status: complete.
## RS-018 — 2026-08-18 — Read-only administration-console preview

- Goal: make the initial administration interface visible and usable during local development.
- Scope: a server-rendered `/admin` HTML page that authenticates API calls with the already configured
  application Bearer token, shows readiness, and searches catalog stations. It deliberately excludes
  persistent administrator identities, sessions, storage, and state-changing operations.
- Result: the page keeps the entered token only in JavaScript memory for the current browser tab, calls
  `/health/ready` and the existing protected `POST /v1/search` endpoint, and removes the in-memory
  token on disconnect. The OpenAPI contract records the HTML endpoint. The local launcher accepts a
  configured token or generates a random process-only token and prints it for the preview login.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test`, and `git diff --check` passed. The regular suite ran 57 tests successfully;
  PostgreSQL and provider-credential-dependent live tests remained explicitly ignored.
- Status: complete.
 
 