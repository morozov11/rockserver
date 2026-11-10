# RockServer documentation

Code-oriented documentation for developers and coding agents. Service overview and user instructions stay in the root [`README.md`](../README.md).

Production environment: **`https://rockplatform.win`**

| Document | Purpose |
| --- | --- |
| [architecture.md](architecture.md) | Search request flow, catalog import, HTTP service, boundaries, and persistence |
| [codex-project-context.md](codex-project-context.md) | Technical context for AI coding agents, crate layout, invariants, and testing |
| [status.md](status.md) | Current working state, verified capabilities, active stage, known limitations |
| [tasks.md](tasks.md) | Chronological task log with goals, scopes, verification results, and statuses |
| [admin-security-plan.md](admin-security-plan.md) | Admin identity foundation, WebAuthn/Passkey auth, session tokens, threat model |
| [admin-operations-runbook.md](admin-operations-runbook.md) | Admin deployment, database migrations, backup/restore, bootstrap runbook |
| [authenticated-radio-stream-relay-plan.md](authenticated-radio-stream-relay-plan.md) | Authenticated stream relay (`/api/v1/stations/{id}/stream`), ICY metadata, RR-001…RR-006 |
| [listening-search-analytics-plan.md](listening-search-analytics-plan.md) | Proposal: minimal search and listening history for personalization (not implemented) |
| [frontend-redesign-plan.md](frontend-redesign-plan.md) | Web cabinet (`web/`) redesign plan and stages |
| [rockmobile-extended-catalog.md](rockmobile-extended-catalog.md) | Historical catalog export pipeline for RockMobile; the app is server-driven since 2026-10-22 and no longer bundles it |
| [service-diagrams.html](service-diagrams.html) | Standalone interactive architecture and request-flow diagrams (open in browser) |
| [live-control-mockups.html](live-control-mockups.html) | Standalone UI mockups for RockMobile & RockCast live device control |
| [roadmap/](roadmap/) | Milestones, roadmap stages, and future capabilities (detailed below) |
| [rfc/](rfc/) | Proposals not yet accepted for implementation (index in [rfc/README.md](rfc/README.md)) |
| [archive/](archive/) | Historical staging reports, migration postmortems, and completed task notes |

### Key Roadmaps in [`roadmap/`](roadmap/)

- **[roadmap/ai-provider-architecture.md](roadmap/ai-provider-architecture.md)** — TZ for the neural-service layer (consumers: RockServer, web cabinet, RockMobile, RockCast). The service interface is [`rock-llm-service/docs/contract.md`](../../rock-llm-service/docs/contract.md); the services TZ is `rock-llm-service/docs/roadmap/services-tz.md`. Draft, pending owner approval.
- **[roadmap/voice-search-improvements.md](roadmap/voice-search-improvements.md)** — Search quality roadmap SRCH-001…SRCH-012 (ONNX pool, prefilter, ANN branch, ranking, explicit language, embedding lifecycle, genre vocabulary, LLM breaker, metrics and log audit).
- **[roadmap/rockmobile-rockcast-live-control.md](roadmap/rockmobile-rockcast-live-control.md)** — Authoritative live playback state synchronization and control between RockMobile and RockCast.
- **[roadmap/station-icons.md](roadmap/station-icons.md)** — Server-owned station icons pipeline, caching, and resize proxy.
- **[roadmap/device-control-rockcast-esp32.md](roadmap/device-control-rockcast-esp32.md)** & **[roadmap/device-control-tasks.md](roadmap/device-control-tasks.md)** — Cross-device control protocol and hardware targets.

## Invariants & Principles

- **Separation of Concerns:** Keep server API, search, persistence, and external providers isolated. Never combine server and client code in a single change.
- **Contract as Source of Truth:** `api/openapi.yaml` defines the HTTP contract. All public endpoints are versioned under `/api/v1` (with legacy aliases deprecated).
- **Graceful Degradation:** Provider failures (LLM, embeddings, speech) degrade predictably to deterministic metadata search.
- **Server-driven clients:** RockCast and RockMobile load the catalog, icons, and audio streams from RockServer (since 2026-10-16/22); they no longer bundle an offline catalog.
- **Always Keep Docs in Sync:** Every task affecting architecture, state, or tasks must update `docs/status.md` and `docs/tasks.md`.
