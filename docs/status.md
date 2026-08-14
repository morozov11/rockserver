# Project status

Last updated: 2026-08-14

## Current state

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
- Logging filter: `RUST_LOG`, default `info` when unset or invalid.
- HTTP catalog backend: `DATABASE_URL` selects PostgreSQL; absence selects the six-station in-memory metadata fallback.
- Optional query embeddings: `ROCKSERVER_SEMANTIC_PROVIDER=deterministic-dev`; absence means metadata-only search.
- Optional LLM parser: `YANDEX_AI_API_KEY` and `YANDEX_FOLDER_ID` together enable Yandex AI Studio; absence of both selects deterministic parsing, and a partial configuration is a safe startup error.
- Yandex parser tuning: `YANDEX_LLM_MODEL=yandexgpt` by default and `YANDEX_LLM_TIMEOUT_MS=3000`, bounded to 100–10,000 ms.
- Development embedding dimension: `ROCKSERVER_EMBEDDING_DIMENSION`, default 32, valid range 1–16,000.
- Development embedding provenance: model `rockserver-deterministic-dev`, version `1`, plus configured dimension.
- Embedding command: `cargo run --bin backfill_embeddings`; it requires both `DATABASE_URL` and explicit deterministic provider selection.
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

The unbounded vector column deliberately avoids locking schema to a random test/dev dimension. Exact search filters compatible provenance before the cosine operator. RS-007 does not add HNSW or IVFFlat: those indexes require a fixed-dimensional expression/operator class and should be introduced only for a selected production model and measured catalog scale.

`backfill_embeddings` reads station documents in stable station-ID pages and upserts one embedding at a time. Repeating the workflow is idempotent for the same provenance and updates existing rows. It does not change Radio Browser ownership, delete catalog rows, probe streams, or make network requests.

## Known limitations

- `deterministic-dev` is a repeatable hash fixture, not a meaningful production semantic model.
- Yandex AI Studio is the only production LLM query parser; it has bounded per-request fallback but no retry policy or circuit breaker. Production embeddings remain absent.
- Exact vector search has no ANN index and is intended as a correctness foundation, not a scale claim.
- The backfill currently visits all stations and upserts every embedding; it does not yet skip unchanged station/model inputs or resume a failed run from durable workflow state.
- A model/version is expected to imply one dimension, but RS-007 records/enforces compatibility per row rather than introducing a separate model registry.
- Existing Radio Browser pagination, stale-run, language-code, and upstream-health limitations from RS-006 remain.
- RockCast does not yet call the canonical voice-command route or distinguish its timeout/error outcomes in the UI.
- The streaming wire protocol and validation are implemented, but no production Yandex/OpenAI adapter, provider retries/circuit breaking, credentials, or RockCast microphone client is configured yet.
- Authentication, rate limiting, metrics, scheduler, stream probing, and deployment hardening remain out of scope.

## Verification

The regular suite passes with 50 deterministic unit, HTTP, contract, and WebSocket tests; the real PostgreSQL, SpeechKit, and four Yandex LLM integration cases remain opt-in and ignored by default. Coverage includes typed command serde/schema/semantic validation, loopback Yandex LLM mocks for header/model/schema, non-2xx, timeout, malformed/oversized response, and secret-safe errors; a real loopback WebSocket upgrade with fake recognition; and existing query, provider-fallback, embedding, ranking, and persistence boundaries.

Docker Engine 29.7.2 and Compose 5.3.1 ran isolated project `rockserver-rs007-test` on host port 55438 with `pgvector/pgvector:pg17`. The real integration test passed for extension/migrations, embedding insert and repeat update, provenance/dimension storage, exact cosine similarity, hard filters, exclusions, final limit, semantic tie-break, metadata fallback, HTTP search, and repository-only readiness. The real deterministic backfill command also completed twice; inspection showed seven unique development rows for `rockserver-deterministic-dev:1:8`. Only that project's container, network, and volume were removed afterward.

For RS-015, `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed. The exact ignored live command test also passed against Yandex with a typed calm-jazz result. `graphify update .` then rebuilt the local graph to 774 nodes, 1,692 edges, and 37 communities; it reported the pre-existing zero-node `hooks.json` warning and stale community labels.

## Next step

Implement and select the first production `StreamingSpeechRecognizer` adapter (Yandex SpeechKit v3 is the preferred Russian MVP), then connect RockCast microphone capture with timeout/cancellation and local-catalog fallback. Add OpenAI behind the same trait after the shared conformance tests pass.
