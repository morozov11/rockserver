# Active follow-up

The service foundation, production deployment path, search/voice APIs, account and administrator flows, Yandex Smart Home sensor cabinet, and core device-control server are implemented. Completed work is recorded in [docs/tasks.md](docs/tasks.md); this file tracks the next useful increments.

## P0 — search retrieval correctness

These server tasks are the highest-priority open items in the [voice-search roadmap](docs/roadmap/voice-search-improvements.md).

1. **SRCH-002 — true semantic candidate retrieval.** Use the E5 HNSW index to retrieve semantic candidates independently of lexical/tag matches, and prove index use with PostgreSQL EXPLAIN. Current SQL only applies cosine scoring after tag, full-text, or trigram candidate selection.
2. **SRCH-003 — prefilter ordering.** Remove degraded-stream stations before the bounded candidate limit, and include trigram relevance in prefilter ordering so strong name matches are not displaced.
3. **SRCH-004 — safer language inference.** Run semantic language classification only when the request explicitly mentions a broadcast language; use a soft ranking signal where a hard filter was not requested.

**Recently completed:** SRCH-001 moved ONNX inference to spawn_blocking and added a bounded session pool. This is complete in the current repository; the project status does not record a production rollout yet.

## Current cross-repository handoff

- **RC-4 / RM-4 — authoritative live playback UI.** In the RockMobile and RockCast repositories, render selected station, playback state, and volume from revisioned device state. A command acknowledgement alone is not proof that playback changed. Preserve the server-resolved station playback path and do not expose stream URLs to the controller. See [the live-control handoff](docs/roadmap/rockmobile-rockcast-live-control.md).

## P1 — relevance and voice reliability

- **SRCH-005 — RRF ranking fusion**, after SRCH-002 and SRCH-003, to combine lexical and semantic retrieval ranks.
- **SRCH-006 — query and station-text normalization** for E5, including removal of conversational playback words from embeddings.
- **SRCH-007 — dynamic genre taxonomy** so LLM schemas and prompts use the database taxonomy rather than only a compiled list.
- **SRCH-008 — LLM resilience:** circuit breaker, bounded intent cache, explicit model configuration, and search rate limiting.
- **RC-VOICE-001 — audio capture quality** in RockCast: anti-aliased 48-to-16 kHz resampling and voice activity detection. Keep this work in the RockCast repository.
- Complete deterministic end-to-end voice checks and improve visible cancellation, recognition, and provider-error states in the clients.

## Device-control expansion

The server-side control plane and the RockMobile/RockCast player integration have a working foundation. Continue by dependency order in the [device-control roadmap](docs/roadmap/device-control-tasks.md).

- **DC-018 / DC-019:** ESP32 display surface and sensor modules. These belong in rock-esp32.
- **DC-020:** sensor-to-display end-to-end acceptance is paused until physical sensors are available.
- **DC-021 / DC-022:** a separate Home Assistant connection adapter and read-only entity synchronization. The existing user-linked Yandex Smart Home sensor feature does not implement this adapter.
- **DC-023 onward:** constrained actuator actions, multi-domain voice, durable operations, timers, weather, and speech delivery follow their prerequisite contracts. Complete DC-037 production hardening before broad actuator or automation rollout.

## P2 — measurement and operations

- **SRCH-009 / SRCH-010:** incremental embedding backfill and safe model-version migration.
- **SRCH-011:** golden search set with Recall@K and MRR once RRF is in place.
- **SRCH-012:** search telemetry and log-privacy audit.
- Capture production PostgreSQL query plans and capacity measurements as the search candidate path changes.

## Suggested order

1. SRCH-002 through SRCH-004, with repeatable relevance fixtures for before/after comparison.
2. RC-4 / RM-4 live playback state in the client repositories.
3. Remaining search and voice reliability work.
4. ESP32 and Home Assistant milestones when their hardware and product prerequisites are ready.
