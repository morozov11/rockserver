# Project status

Last updated: 2026-08-20

## Architecture refactor review

The package layout was reviewed and the first safe refactor was completed. Catalog import code now
lives under `src/catalog/`, and voice-command plus streaming speech boundaries live under
`src/voice/`. Compatibility facades preserve the former `rockserver::speech` and
`rockserver::voice_command` paths for existing clients and tests. HTTP and search remain the next
large seams for incremental extraction: `src/http/mod.rs` still owns several transport concerns,
and `src/search/mod.rs` still combines domain models, the in-memory repository, and orchestration.

Verification for this refactor: `cargo fmt --check`, strict all-target/all-feature Clippy, and
`cargo test` passed. Tests used an isolated `target-codex` directory because an existing local
RockServer process held the normal debug executable open. No public route or OpenAPI behavior was
changed.

## Local ONNX semantic search

`onnx-local` provides CPU-only `intfloat/multilingual-e5-small` inference (384 dimensions) using local ONNX Runtime and tokenizer assets. Migration 0005 persists normalized station text and adds an E5-provenance cosine HNSW index. On 2026-08-17 the local PostgreSQL database contained 1,005 stations (999 imported from Radio Browser), all 1,005 searchable documents were backfilled with matching E5 embeddings, and live Russian queries completed through `POST /v1/search`.

## Current state

The first API-access security slice is implemented. Production startup now requires a unique
`ROCKSERVER_API_BEARER_TOKEN` of at least 32 characters; `POST /v1/search`, both voice-command
routes, and both WebSocket voice-stream handshakes reject absent, malformed, or invalid
`Authorization: Bearer` credentials with structured `401 authentication_required` responses.
`/health/live` and `/health/ready` remain unauthenticated for process supervision. The initial
token is deployment configuration only, compared without prefix-based early exit, and is not
logged. Durable, per-RockCast client credentials, administrator sessions, and the HTMX console
remain the next deliveries described in `docs/admin-security-plan.md`.
`GET /admin` now provides a local, read-only administration-console preview: a browser operator
enters the existing application Bearer token, which stays only in the current tab's memory, then
can inspect readiness and search catalog stations. It is not an administrator-account, session,
audit, or catalog-management implementation.
`run-rockserver-local.ps1` accepts an explicit local token or reads it from `.env`; if neither is
set, it generates a cryptographically random token for that process and prints it alongside the
admin-preview URL. The generated value is not persisted.
`docs/service-diagrams.html` now distinguishes this deployed single deployment credential from
the planned persisted RockCast clients and Bearer-based administrator sessions, and records the
current RockCast Bearer voice-handshake integration.

The active product direction is Windows-first: RockCast text search, Windows microphone capture, the RockServer voice/STT path, and production hardening will be completed and validated before any ESP32 work. The ordered plan is recorded in `docs/windows-production-roadmap.md`. ESP32 is future backlog, not a current delivery stage.

RS-007 was re-verified on 2026-08-14 against the disposable local `rockserver` database on PostgreSQL 18.1 with pgvector 0.8.6. A clean `public` schema accepted migrations 1–4, the ignored real integration test passed, and two deterministic development backfills produced seven unique 32-dimensional station embeddings with no duplicate provenance keys. A live HTTP smoke check reported both health endpoints ready and returned hybrid-ranked `jazz` results. Credentials remain environment-only.

Stages 0–10 are complete in the current working tree: repository bootstrap, Axum HTTP skeleton, OpenAPI search contract, deterministic in-memory search, PostgreSQL persistence, controlled Radio Browser import, RS-007 semantic ranking foundation, RS-008 voice-command JSON contract, the provider-neutral streaming voice API, and Yandex AI Studio intent parsing.

`POST /v1/search` keeps the existing request and response schemas. `SearchService` now owns request-only query interpretation and optional query embedding before calling `StationRepository`. The in-memory repository remains metadata-only. PostgreSQL uses exact pgvector cosine similarity when the query and station embedding provenance match, and otherwise preserves metadata fallback.

RS-008 stabilizes the Windows voice-command transport contract without introducing audio upload or an STT provider. The canonical route is `POST /api/v1/voice/command`; `POST /v1/voice/command` is a deprecated compatibility alias with identical behavior. Both accept only an already-recognized `transcript` plus the established locale, limit, and station-exclusion controls, then delegate to the existing `SearchService`. A successful response returns the trimmed transcript, normalized query, full deterministic result list, and `selected_station` equal to the first result or `null` when there is no match. Existing `POST /v1/search` remains unchanged.

The JSON voice route retains its existing limits and error behavior. Canonical WebSocket `GET /api/v1/voice/stream` (deprecated alias `/v1/voice/stream`) now accepts a validated start event and bounded PCM16 mono binary chunks, emits partial/final transcript events, and resolves the final transcript through `SearchService`. Chunks are limited to 65,536 bytes, sessions to 10 MiB, provider operations to ten seconds, and search to five seconds. Terminal WebSocket errors retain `code`, `message`, `request_id`, and `details`.

The new `StreamingSpeechRecognizer`/`SpeechStreamSession` traits make Yandex and OpenAI replaceable without exposing credentials or upstream protocol details to clients. Startup currently installs an unavailable recognizer, so the wire protocol is implemented and deterministically tested but real audio decoding requires the next provider-adapter task.

An ignored `yandex_speechkit_live` integration test covers a real, pre-recorded mono Ogg/Opus command against SpeechKit's synchronous endpoint. Its committed fixture and expected phrase are generated explicitly by `generate_speechkit_fixture` through Yandex TTS; both network operations are billable and only run when selected. The recognition test is gated by explicit credentials, while optional variables can override the committed audio and phrase. Service-account API-key requests intentionally omit `folderId`, because SpeechKit derives the folder from the service account. The generator's opt-in `YANDEX_SPEECHKIT_DEBUG=1` mode prints non-secret request metadata, response status and headers, plus a 16 KiB bounded redacted error body; it also removes key-like fragments echoed by the provider. For explicit mismatch diagnosis, `TEST_YANDEX_STT_DEBUG=1` logs the non-secret STT request metadata and actual/expected transcript; its authorization is always redacted. It does not make the streaming `SpeechStreamSession` production-ready. After correcting local authentication, the 2026-08-14 live TTS generation and STT recognition succeeded: the committed 18,341-byte Ogg fixture produced HTTP 200 and matched the expected transcript. Earlier 401 attempts were retained only as safe diagnostic history.

The query parser, LLM provider, and embedding provider are traits. `LlmQueryParser` turns a bounded provider JSON response into the existing `QueryIntent`; it receives no catalog. The deterministic metadata parser is the default and the parser failure fallback. `YandexLlmProvider` is enabled only when both `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` are present; absent configuration preserves deterministic startup, while partial configuration fails safely without revealing values. It uses the documented synchronous AI Studio completion API, `Api-Key` authorization, `gpt://<folder>/yandexgpt/latest` by default, JSON Schema output, a 3-second timeout, and response/token bounds. Malformed/oversized/non-2xx/timeout responses and invalid hard filters all degrade through the existing deterministic path. Optional local `.env` loading is provided by `dotenvy`; `.env` and `.env.*` stay ignored. This is intent parsing for text and already-recognized voice transcripts, not SpeechKit STT. Ordinary tests use loopback mocks and never contact Yandex or another external provider.

RS-015 adds a provider-neutral `CommandInterpreter` above the same replaceable `LlmProvider`. `LlmCommandInterpreter` sends bounded STT text and locale with a strict Yandex-compatible JSON Schema, deserializes directly into serde-backed `VoiceCommand`, `Intent`, and `RadioQuery`, and then performs only semantic validation and normalization. It supports radio play/search, stop, next/previous station, relative volume change, and unknown intent without an heuristic command parser. The separate ignored `yandex_llm_live` target has four exact tests; its primary calm-jazz case passed live on 2026-08-14 and safely logged the POST endpoint, redacted authorization/folder, request body, HTTP 200 response body, and final typed command.

Station embedding generation is a separate `backfill_embeddings` command. It is never called by HTTP startup or `POST /v1/search`. Radio Browser import ownership and update semantics remain unchanged.

## Configuration and behavior

- HTTP listener: `ROCKSERVER_BIND_ADDR`, default `127.0.0.1:3000`.
- Required application access gate: `ROCKSERVER_API_BEARER_TOKEN`, a unique secret of at least
  32 characters. Clients send it as `Authorization: Bearer <token>`; do not put it in logs or
  commit it to `.env` files.
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- HTTP catalog backend: `DATABASE_URL` selects PostgreSQL; absence selects the six-station in-memory metadata fallback.
- Optional query embeddings: `ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev` for tests/development or `onnx-e5-local` with the `onnx-local` Cargo feature and local model/tokenizer/runtime paths; absence means metadata-only search.
- Optional LLM parser: `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` together enable Yandex AI Studio; absence of both selects deterministic parsing, and a partial configuration is a safe startup error.
- Yandex parser tuning: `YANDEX_LLM_MODEL=yandexgpt` by default and `YANDEX_LLM_TIMEOUT_MS=3000`, bounded to 100–10,000 ms.
- Development embedding dimension: `ROCKSERVER_EMBEDDING_DIMENSION`, default 32, valid range 1–16,000.
- Development embedding provenance: model `rockserver-deterministic-dev`, version `1`, plus configured dimension.
- Embedding command: `cargo run --features onnx-local --bin backfill_embeddings`; it requires `DATABASE_URL`, explicit provider selection, and local assets for the E5 provider.
- Radio Browser import command and configuration remain as documented in `README.md`; no import provider is called by search.
- Local database: `compose.yaml` uses `pgvector/pgvector:pg17` with development-only credentials and a PostgreSQL healthcheck.
- Migrations: embedded versioned files run for PostgreSQL search, import, and embedding store connections.
- Public HTTP/OpenAPI schemas: unchanged by RS-007; the description now records optional semantic ranking and metadata fallback.

## Query, ranking, and failure semantics

`QueryParser` receives only validated query text and locale. It returns terms, tags, language, and country intent and has no catalog parameter. Provider intent is normalized and hard filters are validated before repository use. A parser error or invalid intent uses the deterministic metadata parser.

`EmbeddingProvider` receives one query string during search or one station document during the controlled backfill. It never receives the full catalog. Invalid embeddings are rejected before persistence/search: model and version must be non-empty, dimension must be 1–16,000 and equal to the vector length, values must be finite, and the vector must be non-zero. Embedding provider failure omits semantic input and continues with metadata search.

PostgreSQL applies language/country hard filters and station exclusions in the candidate CTE, before score ordering and final limit. Metadata score retains the RS-003/RS-004 exact-token formula. Compatible cosine similarity is normalized and clamped with `1 - cosine_distance / 2`. Final hybrid score is `0.70 * metadata_score + 0.30 * semantic_score`. A station without compatible model/version/dimension provenance retains its full metadata score. Results order by final score descending and station ID ascending; station ID is the last tie-break.

Liveness remains process-only. Readiness calls only the selected repository, so optional parser/embedding failure does not report false unready. PostgreSQL loss still returns 503/`not_ready`; in-memory readiness remains 200/`ok`.

## Persistence and backfill

Migration `0004_add_station_embeddings.sql` enables pgvector and adds `station_embeddings` with station ownership, model, version, dimension, unbounded `vector`, and created/updated timestamps. CHECK constraints enforce declared dimension and non-zero norm. The table key is `(station_id, model, version)`; changing dimension for the same development model/version replaces that station embedding.

The unbounded vector column remains dimension-neutral for tests and future providers. Exact search filters compatible provenance before the cosine operator; migration 0005 adds a partial 384-dimensional HNSW cosine index only for `intfloat/multilingual-e5-small:onnx-v1`.

`backfill_embeddings` reads station documents in stable station-ID pages and upserts one embedding at a time. Repeating the workflow is idempotent for the same provenance and updates existing rows. It does not change Radio Browser ownership, delete catalog rows, probe streams, or make network requests.

## Known limitations

- `deterministic-dev` remains a repeatable hash fixture; production-like semantic behavior requires explicitly provisioned local E5/ONNX assets.
- Yandex AI Studio is the only production LLM query parser; it has bounded per-request fallback but no retry policy or circuit breaker.
- The deterministic parser currently derives a station-language hard filter from the request locale. Live checks therefore used `en-US` to inspect cross-language E5 ranking over the broader English catalog; `ru-RU` intentionally restricts candidates to Russian-language stations.
- The backfill currently visits all stations and upserts every embedding; it does not yet skip unchanged station/model inputs or resume a failed run from durable workflow state.
- A model/version is expected to imply one dimension, but RS-007 records/enforces compatibility per row rather than introducing a separate model registry.
- Existing Radio Browser pagination, stale-run, language-code, and upstream-health limitations from RS-006 remain.
- RockCast does not yet call the canonical voice-command route or distinguish its timeout/error outcomes in the UI.
- The streaming wire protocol and validation are implemented, but no production Yandex/OpenAI adapter, provider retries/circuit breaking, credentials, or RockCast microphone client is configured yet.
- Per-client credential persistence/revocation, administrator authentication, login throttling,
  request audit storage, admin console state-changing operations, rate limiting, metrics, scheduler,
  stream probing, and deployment hardening remain future work.

## Verification

The regular suite passes with 67 deterministic unit, HTTP, contract, and WebSocket tests; the real PostgreSQL, SpeechKit, and four Yandex LLM integration cases remain opt-in and ignored by default. Coverage includes typed command serde/schema/semantic validation, loopback Yandex LLM mocks for header/model/schema, non-2xx, timeout, malformed/oversized response, and secret-safe errors; a real loopback WebSocket upgrade with fake recognition; application Bearer rejection while health stays public; the unauthenticated admin-preview shell; and existing query, provider-fallback, embedding, ranking, and persistence boundaries.

Docker Engine 29.7.2 and Compose 5.3.1 ran isolated project `rockserver-rs007-test` on host port 55438 with `pgvector/pgvector:pg17`. The real integration test passed for extension/migrations, embedding insert and repeat update, provenance/dimension storage, exact cosine similarity, hard filters, exclusions, final limit, semantic tie-break, metadata fallback, HTTP search, and repository-only readiness. The real deterministic backfill command also completed twice; inspection showed seven unique development rows for `rockserver-deterministic-dev:1:8`. Only that project's container, network, and volume were removed afterward.

For RS-015, `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed. The exact ignored live command test also passed against Yandex with a typed calm-jazz result. `graphify update .` then rebuilt the local graph to 774 nodes, 1,692 edges, and 37 communities; it reported the pre-existing zero-node `hooks.json` warning and stale community labels.

For RS-016, `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test --all-features` passed with 50 regular tests; six credential/asset/database-dependent tests remained ignored. The real local import produced 999 Radio Browser stations alongside six built-ins, and E5 backfill produced 1,005 `intfloat/multilingual-e5-small:onnx-v1:384` rows. Live search ranked `Midnight Jazz Lounge` first for `спокойный джаз` and `# RdMix Classic Rock 70s 80s 90s` first for `рок 80-х`. `классическая музыка без слов` returned weaker ambient/jazz results, documenting a current catalog/locale-filter quality limitation. `graphify update .` rebuilt 809 nodes, 1,774 edges, and 42 communities with the known zero-node `hooks.json` and stale-label warnings.

## Genre hierarchy and search fallback

RS-019 introduces a genre taxonomy hierarchy and progressive search fallback. `CANONICAL_TAGS` now includes subgenres present in the real Radio Browser catalog (`black metal`, `death metal`, `alternative rock`, `classic rock`, `pop rock`, `smooth jazz`, etc.). A `genre_parent` function maps each subgenre to its nearest parent (`black metal` → `metal` → `rock`, `smooth jazz` → `jazz`). `station_matches_requested_genre` accepts stations whose tags match any ancestor of the requested genre, so a `"heavy metal"` query matches stations tagged `"rock"`.

`SearchService::search` applies progressive fallback when the exact genre filter yields no results: first broadening to parent genres, then dropping the genre constraint entirely while keeping the `MIN_RELEVANCE_SCORE` gate. The LLM system prompt automatically includes the expanded tag vocabulary. A new builtin catalog station (`Iron Forge Radio`, tags `heavy metal`/`metal`, language `en`) validates the hierarchy. Deterministic tests confirm that `"english heavy"` finds English rock/metal stations, that subgenre queries match parent-tagged stations, and that unrelated genres remain rejected.

## Genre database and stream probing

RS-020 moves the genre hierarchy from compiled-in constants to a PostgreSQL `genre_hierarchy` table seeded with ~250 genres across 25 root categories. `GenreTaxonomy` loads from the database at startup with a builtin fallback for in-memory mode. A new `probe_streams` binary connects to each stream URL (8-second timeout, 50 concurrent), marking unreachable streams as `degraded` with the error stored in `last_probe_error`. The PostgreSQL search CTE now excludes `degraded` streams. The Radio Browser importer supports `RADIO_BROWSER_TAGS` for per-tag import passes, enabling broader genre coverage. Migrations 0006 and 0007 are applied automatically on connect.

## Station name search improvements (RS-021)

RS-021 improves station name search accuracy for voice queries such as "включи радио диджей" → `radioDJ`:

- **Stop-word filtering**: Command verbs (`включи`, `поставь`, `найди`, `play`, `find`, etc.) are removed from query terms before matching, preventing score dilution.
- **CamelCase splitting**: `tokenize()` now splits `"radioDJ"` into `["radio", "dj"]` and `"XMLParser"` into `["XML", "Parser"]`. This affects both query tokenization and `searchable_text()` at import time.
- **Transliteration expansion**: A word-level mapping (`радио`↔`radio`, `диджей`↔`dj`, `фм`↔`fm`, `рок`↔`rock`, ~50 entries) expands query terms so that Cyrillic queries match Latin station names and vice versa.
- **Substring matching**: Both in-memory ranking and the PostgreSQL search CTE now award partial credit (0.5 per term) for substring matches in station names (terms ≥ 3 chars), so "радио" finds stations with "radio" in their name even without exact token match.
- **Backfill script**: `fill-database.ps1` now includes `backfill_embeddings` as the final step after import and stream probing.
- **LLM intent normalization + fallback**: Provider `terms` are normalized into atomic tokens (`tokenize()`), expanded with transliteration aliases, and if the provider returns empty `terms` and empty `tags` the system switches to deterministic parsing for more reliable station-name matching.
- **Search performance**: Added a PostgreSQL `pg_trgm` + trigram index on `lower(stations.name)` to accelerate `LIKE '%term%'` prefiltering/substrings.

## Full-text search and similarity scoring (RS-023)

RS-023 replaces the expensive sequential `LIKE '%term%'` prefilter with three index-backed search layers:

1. **PostgreSQL FTS (tsvector)**: Migration 0009 adds a `searchable_tsv` tsvector column derived from `searchable_text` with a GIN index and auto-update trigger (config `simple` for language-neutral exact tokens). The prefilter uses `plainto_tsquery('simple', $14)` for O(log n) candidate selection.
2. **pg_trgm similarity**: The prefilter now uses the `%` operator (`lower(s.name) % qt.term`) instead of `LIKE '%term%'`, leveraging the existing GIN trigram index for fuzzy name matching. A `trgm_score` (max similarity) contributes 0.3 weight to the metadata score.
3. **Embedding cosine (unchanged)**: Semantic fallback via pgvector HNSW index remains as before.

`raw_query` (stop-words removed) is passed through `QueryIntent` → `SearchQuery` → `PostgresSearchParameters` → `$14` for FTS matching.

## Correct name ranking when LLM omits terms (RS-024)

RS-024 fixes incorrect ranking and extra latency when the LLM returns `tags` but omits `terms` (e.g. "включи радио рокс"). Previously this caused name-token matching to be skipped and ranking fell back to mostly genre-tag + semantic similarity.

Now in `SearchService::interpret_and_search`:

- if `intent.terms` is empty, we run `DeterministicQueryParser` and:
  - keep provider genre `tags` (when present),
  - but replace `terms`, `core_term_count`, and `raw_query` with deterministic tokenization output.
- the transliteration vocabulary was extended with common voice tokens:
  - `ультра`↔`ultra`, `рокс`↔`roks`, `викер`↔`viker`.

## Narrow prefilter candidate set before heavy scoring (RS-025)

RS-025 reduces slow broad queries by making the `prefiltered` CTE `MATERIALIZED`, assigning each row a cheap `prefilter_score` (exact tag hits + `ts_rank_cd` + max trigram similarity), and limiting the candidate pool to `GREATEST(limit * 20, 200)` before the expensive exact-token, substring, and embedding scoring phase.

## Indexed candidate-branch search (RS-030)

RS-030 addresses slow PostgreSQL station searches observed in request logs. The search SQL now obtains candidate IDs through three independent, index-backed branches (tag GIN, FTS GIN, and name trigram GIN), unions and de-duplicates those IDs, and only then calculates the unchanged prefilter and final ranking scores. Full-text term queries are generated once per search term, and each candidate station name is normalized and tokenized once before its exact-name, substring, and trigram scores reuse those values. This removes repeated parsing and regex tokenization for expanded/transliterated voice terms without changing matching semantics. Query interpretation, matching rules, limits, hard filters, exclusion handling, score formulae, and deterministic ordering are unchanged. Debug logs now record parser, embedding, repository, and database-search elapsed milliseconds plus result count. Live `POST /api/v1/voice/command` checks against the configured PostgreSQL database returned 30 stations in 1,387 ms for `Включи немецкий рок или хэви метал` and in 1,388 ms for the STT transcription `Включи немецкий рок или хейли метал`; the latter spent 1,097 ms in parsing and 233 ms in PostgreSQL, below the five-second voice timeout. A production `EXPLAIN (ANALYZE, BUFFERS)` comparison remains useful for broader capacity monitoring.

## Explicit world-country filters (RS-031)

RS-031 expands deterministic country recognition from three countries to the ISO 3166-1 alpha-2 country set. It recognizes common Russian and English country names plus demonyms, including `немецкий`/`Germany` -> `DE`. The parser still derives `country_code` only from an explicit word sequence in the original request, never from locale or an LLM guess. If a phrase is ambiguous (for example, `Конго` without a republic qualifier), it emits no hard country filter.

## Remove per-candidate regex tokenization from exact matching (RS-026)

RS-026 replaces the exact-term part of `matched_count` with `searchable_tsv @@ plainto_tsquery('simple', qt.term)` so PostgreSQL reuses the precomputed normalized document instead of re-running `regexp_split_to_table(...)` over station names and tags for every candidate row. The cheap prefilter ordering also now avoids early full-table trigram scoring and keeps trigram similarity only in the limited candidate phase.

## Fast empty result when prefilter has no candidates (RS-027)

RS-027 removes semantic-only fallback when metadata prefilter returns zero rows. For unknown or unsupported requests this makes search return empty quickly instead of spending extra time on embedding-neighbor fallback and returning low-value semantic matches.

## Generic bidirectional transliteration (RS-028)

RS-028 adds generic bidirectional transliteration for query terms (Russian Cyrillic ↔ Latin) as a standard step in `expand_transliterations`, instead of relying on point-by-point station-brand mappings. This improves cross-script station-name recall (for example, spoken Russian name forms matching Latin station names and vice versa) while keeping existing dictionary-based genre/radio-word normalization.

## Name-priority mode for "включи радио ..." (RS-029)

RS-029 introduces a station-name priority path for commands that explicitly start with `включи радио` / `поставь радио` (and English equivalents). The parser derives an ordered station-name hint phrase from the tail of the command, the query carries that hint through to PostgreSQL, and SQL ranking gives a strong bonus when the normalized station name contains the same words in the same order. This helps short exact station names outrank longer related variants.

## Next step

Implement and select the first production `StreamingSpeechRecognizer` adapter (Yandex SpeechKit v3 is the preferred Russian MVP), connect RockCast microphone capture with timeout/cancellation and local-catalog fallback, and add OpenAI behind the same trait after the shared conformance tests pass.
