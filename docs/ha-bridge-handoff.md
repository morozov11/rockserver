# Home Assistant bridge: work handoff

Updated: 2026-11-12. Work in progress in the main `C:\repos\rockserver` checkout.

## Requested result

Read-only Home Assistant custom integration running inside HA on `alex-center`, pushing an
explicit allowlist of sensor values and one selected weather entity over outbound WSS to
RockServer. No inbound port on the home network, no paired-device identity, no other clients.

## Findings

- `docs/roadmap/home-assistant-local-bridge.md` originally proposed a separate local agent;
  the owner explicitly approved replacing it with a HA custom integration.
- SSH `alex-center` showed `python3 -P -m homeassistant --config /config`, Docker daemon, and no
  `ha` CLI. HA Container is likely but the installation type is not fully verified.
- Device-control directory currently loads only paired devices. HA needs a separate owner-bound
  projection and read endpoint. Existing `Entity`/`EntityState`, quality and freshness rules can
  guide the new normalized model, but weather has no existing runtime model.
- Yandex Home uses account-bound persistence and revocation; its OAuth/browser flow is distinct
  from this integration's operator-provisioned credential.
- At task start `docs/status.md` and `docs/tasks.md` were already modified, and the roadmap file
  was untracked. Preserve those changes.

## Progress

- Follow-up account work added `GET /api/v1/browser/home-assistant`, a Devices tab card for
  approved sensors/current weather/forecast, public hashed sensor IDs, display names, explicit
  freshness, OpenAPI fixtures, and UI tests. Required Rust checks passed before the final
  PostgreSQL test addition (231 library tests). TypeScript, 48 web tests, Vite build, two Python
  fake tests, and one disposable PostgreSQL migration/owner/revocation test passed. The test
  container was removed. Final sequential Rust checks passed again. Commit and production
  deployment are still in progress.
- Read repository instructions, roadmap, device-control and Yandex examples.
- Confirmed HA supports separate `weather.get_forecasts` daily forecasts.
- Added `src/ha_bridge.rs` domain/store trait, PostgreSQL migration 0029 and store, operator
  provisioning binary, separate WebSocket ingress, browser weather read endpoint, and draft HA
  custom integration under `home_assistant/custom_components/rockserver_bridge`.
- OpenAPI path/schemas and positive/negative fixtures added. Operator run guide is
  `docs/home-assistant-bridge.md`; roadmap header records the approved topology change.
- Initial `cargo fmt --check` and strict Clippy passed. Initial `cargo test` compiled and ran 214
  passing library tests, with the same 15 socket/local HTTP/filesystem tests known to fail in the
  restricted Windows sandbox. New OpenAPI fixture test passed separately. Two Python offline
  fake HA/RockServer tests passed when run with local socketpair permission; the default Windows
  sandbox blocks even asyncio's internal event-loop socketpair.
- Follow-up edits added heartbeat revocation checks, stable public weather ID, per-integration
  pacing, connection-aware freshness, client revision acknowledgement checks and unit validation.
- Final review then found an auth/rotation race. `BridgePrincipal` now carries the credential hash
  used at authentication, and PostgreSQL checks it again on begin, ingest and heartbeat. Required
  checks were rerun after this fix: `cargo fmt --check`, strict Clippy and `cargo test` passed
  sequentially with local loopback/file permission. The library suite passed 230/230 tests, as
  did the OpenAPI contract fixtures. The two Python offline fake tests had already passed; the
  final Rust-only race fix did not change their code.
- Official HA Core source confirms `hass.services.async_call(..., target=..., blocking=True,
  return_response=True)` and `hass.async_create_background_task(...)` signatures. The exact
  mounted HA config path, installed HA version, selected entities and actual WSS behaviour still
  need a live pilot. No real HA credentials were read.

## Next steps

1. Confirm the actual HA config mount and HA version on `alex-center`; select real sensor/weather
   IDs and confirm the weather entity supports daily forecasts.
2. Provision a credential and install the custom integration, then test reconnect/stale/revocation
   on the live HA and server. The disposable PostgreSQL test covered owner isolation and revoke,
   but did not exercise every operator CLI lifecycle branch.

The owner authorized a RockServer commit and deployment in the follow-up request. Live HA
installation has not yet been performed.
