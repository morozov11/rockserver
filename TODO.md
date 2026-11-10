# Active follow-up

The service foundation, production deployment path, search/voice APIs, account and administrator flows, Yandex Smart Home sensor cabinet, and core device-control server are implemented. Completed work is recorded in [docs/tasks.md](docs/tasks.md); this file tracks the next useful increments.

## Search quality and relevance

The [voice-search roadmap](docs/roadmap/voice-search-improvements.md) has the authoritative order and acceptance criteria.

1. **SRCH-002 / SRCH-005 — remaining pure-vector visibility.** Calibrate a relevance gate only after reviewed labels distinguish semantic matches from unrelated requests.
2. **SRCH-008 — intent cache decision.** The search LLM breaker and fixed model-version option are implemented. Measure anonymized repeat rate, latency, and cost before deciding whether a bounded TTL cache is useful; agree a command-data policy first.

**Completed in the working checkout:** SRCH-001, SRCH-003, SRCH-004, SRCH-006, SRCH-007, SRCH-009, SRCH-010, and SRCH-011. SRCH-002 and SRCH-005 remain partial for pure-vector visibility; SRCH-008 remains partial for the cache measurement. See [project status](docs/status.md) for verification limits.

## Current cross-repository handoff

- **RC-4 / RM-4 — authoritative live playback UI.** In the RockMobile and RockCast repositories, render selected station, playback state, and volume from revisioned device state. A command acknowledgement alone is not proof that playback changed. Preserve the server-resolved station playback path and do not expose stream URLs to the controller. See [the live-control handoff](docs/roadmap/rockmobile-rockcast-live-control.md).

## P1 — voice reliability

- **RC-VOICE-001 — audio capture quality** in RockCast: anti-aliased 48-to-16 kHz resampling and voice activity detection. Keep this work in the RockCast repository.
- Complete deterministic end-to-end voice checks and improve visible cancellation, recognition, and provider-error states in the clients.

## Device-control expansion

The server-side control plane and the RockMobile/RockCast player integration have a working foundation. Continue by dependency order in the [device-control roadmap](docs/roadmap/device-control-tasks.md).

- **DC-018 / DC-019:** ESP32 display surface and sensor modules. These belong in rock-esp32.
- **DC-020:** sensor-to-display end-to-end acceptance is paused until physical sensors are available.
- **DC-021 / DC-022:** a separate Home Assistant connection adapter and read-only entity synchronization. The existing user-linked Yandex Smart Home sensor feature does not implement this adapter.
- **DC-023 onward:** constrained actuator actions, multi-domain voice, durable operations, timers, weather, and speech delivery follow their prerequisite contracts. Complete DC-037 production hardening before broad actuator or automation rollout.

## P2 — measurement and operations

- **SRCH-012:** local log-privacy and aggregate observation code is ready; coordinate canary validation and production rollout. Repeat-rate measurement still needs a command-data policy.
- Capture production PostgreSQL query plans and capacity measurements as the search candidate path changes.

## Suggested order

1. Review the remaining pure-vector relevance gate; during SRCH-012 collect privacy-safe aggregate measurements for the SRCH-008 cache decision.
2. RC-4 / RM-4 live playback state in the client repositories.
3. Remaining search and voice reliability work.
4. ESP32 and Home Assistant milestones when their hardware and product prerequisites are ready.
