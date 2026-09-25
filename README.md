# RockServer

RockServer is the Rust 2024 backend for RockCast radio discovery, voice search, accounts, and account-owned device control. It runs as a deployed service and exposes a versioned HTTP/WebSocket API. The implementation lives here; playback UI and firmware live in their own repositories.

## License

RockServer is licensed under the [GNU General Public License, version 3 or later (GPL-3.0-or-later)](https://www.gnu.org/licenses/gpl-3.0.html).

## Current product

- **Radio discovery:** POST /api/v1/search interprets a natural-language request, applies catalog filters, and returns ranked playable stations. The service imports the shared RockCatalog baseline and Radio Browser data; RockCast keeps its local catalog as an offline fallback.
- **Voice:** POST /api/v1/voice/command searches from recognized text. GET /api/v1/voice/stream accepts bounded PCM16 mono chunks and supports the configured Yandex SpeechKit buffered and streaming modes.
- **Accounts and administration:** browser passkeys, paired native devices, an administrator console, station catalog and icon operations, and a user cabinet are implemented.
- **Device control:** native-session authentication, a bounded WebSocket transport, presence, manifests and current state, an owner-scoped directory, and typed command routing are implemented. RockMobile-to-RockCast playback control has passed staging end-to-end verification.
- **Yandex Smart Home:** signed-in users can link an account and view current sensor properties in the browser cabinet. This is separate from the planned Home Assistant device-control adapter.

The project journal records the search-latency and multi-metric Yandex Home releases on 2026-09-24. The SRCH-001 ONNX runtime work is complete in the current repository; its production rollout is not recorded as a release. See [project status](docs/status.md) for the deployment and verification history.

## Architecture and boundaries

- The [OpenAPI contract](api/openapi.yaml) is the source of truth for the public HTTP API.
- HTTP DTOs, search and device-control domain types, PostgreSQL persistence, and provider adapters are kept behind separate boundaries.
- PostgreSQL is the production backend. Migrations cover station catalog/search, accounts, administration, Yandex Home connections, and device-control state.
- Radio Browser imports, stream probes, catalog imports, and embedding backfills run as explicit maintenance commands, outside HTTP startup and request handling.
- LLM providers translate a request into bounded structured intent; they do not scan the station catalog.
- RockCast retains its local station catalog for offline search fallback. Client and firmware changes belong in the RockCast, RockMobile, and rock-esp32 repositories.

A cosine HNSW index exists for the E5 embedding provenance, but current SQL still builds candidates from tags, full-text search, and station-name similarity before applying vector scores. Semantic-only candidate retrieval and related search-prefilter fixes are open work in the [voice-search roadmap](docs/roadmap/voice-search-improvements.md).

See [architecture notes](docs/architecture.md), the [source map](src/ARCHITECTURE.md), the [device-control roadmap](docs/roadmap/device-control-tasks.md), and the offline [service diagrams](docs/service-diagrams.html).

## Current priorities

1. Finish the P0 search retrieval fixes: true HNSW candidate retrieval, healthy-stream and trigram-aware prefilter ordering, and an explicit-language gate for semantic language filters.
2. Continue the RockMobile/RockCast handoff for authoritative live playback state. The UI should reflect revisioned device state, not infer playback from command acceptance.
3. Improve voice-path cancellation and error states, then measure search quality before further ranking changes.
4. Continue ESP32 display/sensor and Home Assistant work when their dependencies and hardware are ready. ESP32 sensor-display end-to-end acceptance is paused pending physical sensors.

The active, ordered backlog and cross-repository pointers are in [TODO.md](TODO.md).

## Build and verify

Use a stable Rust toolchain that supports edition 2024.

~~~text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
~~~

Regular tests use deterministic fakes and do not call live providers or external services. PostgreSQL, ONNX, and provider-backed checks are opt-in and require their documented disposable database, local model assets, or explicit credentials.

## Run locally

For the local PostgreSQL-backed Compose setup, copy .env.example to an ignored .env, review its development-only values, then run:

~~~text
powershell -ExecutionPolicy Bypass -File deploy/verify-compose.ps1 -Mode local -Start
~~~

This starts an isolated local Compose project, checks readiness through Caddy, and removes its containers, network, and volume unless -Keep is supplied. To run the service directly, use run-rockserver-local.ps1 after starting the local database.

Production configuration is managed through the protected deployment workflow described in [deploy/README.md](deploy/README.md). Do not commit secrets or pass production credentials on command lines. The service requires the configured application Bearer and trusted-proxy credentials and does not log their values.

Useful local checks after startup:

~~~text
curl http://127.0.0.1:3000/health/ready
curl -X POST http://127.0.0.1:3000/api/v1/search -H "content-type: application/json" -d '{"query":"calm instrumental jazz"}'
~~~

The API contract describes voice, browser, administrator, account, and device-control routes, limits, and errors.

## Catalog and operator commands

- import_shared_catalog activates the vendored RockCatalog baseline.
- import_radio_browser performs a bounded, explicit Radio Browser import.
- probe_streams checks catalog stream health.
- backfill_embeddings updates embeddings for the configured provider; ONNX E5 requires the onnx-local feature and local model assets.
- export_mobile_catalog builds the documented RockMobile SQLite catalog artifact from PostgreSQL.
- account_cleanup is a preview-first staging operator tool. Follow [administrator operations](docs/admin-operations-runbook.md); never use cleanup against production.

See [RockMobile catalog export](docs/rockmobile-extended-catalog.md) and [deployment operations](deploy/README.md) for detailed procedures. Live SpeechKit and PostgreSQL integration tests are explicitly gated; they are not part of the ordinary deterministic test run.
