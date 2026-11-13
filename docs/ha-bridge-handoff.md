# Home Assistant bridge: work handoff

Updated: 2026-11-13. Work in progress in the main `C:\repos\rockserver` checkout.

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
- Yandex Home uses account-bound persistence and revocation. HA now also has browser account
  management, but its local installation and outbound bearer differ from Yandex OAuth.
- At task start `docs/status.md` and `docs/tasks.md` were already modified, and the roadmap file
  was untracked. Preserve those changes.

## Progress

- On 2026-11-13 the owner ran the staged one-time `sudo` installer on `alex-center`. Independent
  SSH confirmed all five installed file hashes, HA 2026.9.4 restarted, local HTTP 200, and only
  the standard custom-integration warning. Host config mount is `/var/lib/homeassistant` →
  container `/config`. No key has been issued or entered and no live WSS data has been checked.
- New owner request: all connection management should happen in the RockServer cabinet like
  Yandex Home. The owner accepted one-time local HA component installation. Added browser
  provision/revoke with cookie, CSRF and trusted proxy checks; owner-chosen allowlist; one-time
  key display in the card; HA visual config/options flow with password field. Strict Clippy,
  TypeScript, 48 web tests, Vite build, three Python fake tests and a disposable PostgreSQL
  lifecycle and browser HTTP/CSRF tests passed. Final sequential Rust checks passed. Commit
  `f38ad61a2c7a8e9c1cb112c4f6e4a22a413a5c78` was deployed by OPS-001-D; remote worker
  succeeded, matching containers are healthy, public readiness/page/bundle returned 200 and
  anonymous credential issuance returned 403. The local watcher hit an SSH banner timeout after
  worker success; read-only SSH independently confirmed the result. Live HA remains untouched.
- Follow-up account work added `GET /api/v1/browser/home-assistant`, a Devices tab card for
  approved sensors/current weather/forecast, public hashed sensor IDs, display names, explicit
  freshness, OpenAPI fixtures, and UI tests. Required Rust checks passed before the final
  PostgreSQL test addition (231 library tests). TypeScript, 48 web tests, Vite build, two Python
  fake tests, and one disposable PostgreSQL migration/owner/revocation test passed. The test
  container was removed. Final sequential Rust checks passed again. Commit
  `1d6eaa630d5acd59bb499b66981f59ed5a4dd5ee` was deployed by OPS-001-D; worker succeeded,
  public readiness and web bundle returned 200, anonymous overview returned 401, and production
  migration 29/admin binary were confirmed. No production backup was present before migration.
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

1. Select real sensor/weather IDs in HA and confirm daily forecast support.
2. Issue a key in the RockServer cabinet and paste it into HA's visual config flow. Then test
   reconnect/stale/revocation on the live HA and server.
   The disposable PostgreSQL and HTTP tests covered browser CSRF, owner isolation, replacement
   and revoke; they did not exercise every legacy operator CLI branch.

The owner authorized and received the RockServer commit/deployment and installed the HA files.
`alex-center` runs `python3 -P -m homeassistant --config /config`; the install script verified the
host mount `/var/lib/homeassistant`. Live credential/config-flow and WSS checks remain.
