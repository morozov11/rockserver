# RM-004-H — cross-project verification and architecture review

**Review date:** 2026-08-25  
**Scope:** audit only, release `2026.08.2` and mobile package `2026.08.2-mobile.1`. No implementation, data, or configuration was changed.

## Decision

**Not ready for RM-004-I.** The immutable baseline is byte-consistent in all three consumers and the extended mobile artifact is internally valid. However, RockServer does not preserve retirement/replacement metadata and substitutes an unrelated six-row fixture if its pinned catalog is invalid. Both are High issues that require focused follow-up before legacy cleanup.

## Evidence and checks performed

| Check | Command/result |
|---|---|
| Baseline identity | `Get-FileHash -Algorithm SHA256` over release JSON and the RockServer, RockCast, and RockMobile copies: all four were 23,289 bytes, `3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d`, schema 1, catalog `2026.08.2`, 41 active stations, and zero tombstones. |
| Schema identity | The catalog-repo, RockServer, and RockCast schemas were all 9,904 bytes and SHA-256 `a46009febca8a27d0eece15ca09b29f91c4ebcb355b42420a9bca214f62ca5e0`. RockMobile intentionally does not vendor the schema. |
| Representative stations | Parsed canonical release and copies. `somafm-metal-detector`, `radio-caprice-heavy-metal`, `rock-antenne`, and `radio-x-uk-mp3` have identical IDs, primary URLs, ordered tags, country, and null language. `src/catalog/shared.rs` maps the same ID and `station-id:stream-id` source identity. |
| Drift verification | `python C:\repos\rockcast-station-catalog\tools\release_sync.py verify` passed for RockServer, RockCast, and RockMobile (with extended manifest). `python -m unittest discover -s ...\tests -v` passed 12/12, including corruption, rollback, and extended-manifest unit cases. |
| Mobile extended package | SQLite exists, is 14,192,640 bytes, and SHA-256 matches manifest: `ad469d405f177d7e476cf9b3d9985497d0e2c6132ac0f3ce14485f4eab402073`. Direct inspection: `integrity_check=ok`, `user_version=1`, 16,825 `stations`, and 16,825 `station_search` FTS rows. |
| Targeted Rust checks | `cargo test catalog --lib` in RockServer: 11 passed. `cargo test stations::catalog --target-dir C:\repos\rockserver\target\rm004h-rockcast` in RockCast: 9 passed. Live RockCast probe tests were filtered and are not counted. |
| Normal-path network/static review | No `release_sync` invocation was found in application/build sources; its implementation is local-file-only. RockMobile baseline loading is network-free. RockCast local catalog loading is file/embedded-data-only; Radio Browser is a separate enrichment path. |

## Findings

### High — retirement/replacement contract is validated but discarded by RockServer

**Evidence.** `C:\repos\rockserver\src\catalog\shared.rs` deserializes and validates `tombstones` in `validate_document`, but `PinnedSharedCatalog::load` retains only `stations`; `impl From<CatalogStation> for ImportedStation` has no tombstone output. `C:\repos\rockserver\src\catalog\import.rs` has no replacement/retirement metadata on `ImportedStation`. The current release has zero tombstones, so this is latent rather than an observed `2026.08.2` mismatch.

**Impact / likelihood.** A deleted, merged, or split station in a later release can be soft-retired as a missing provider row, but its replacement graph is unavailable to the database/API/export path. Clients with an old selected ID cannot receive the specified migration meaning. Likelihood is high on the first retirement or merge.

**Recommended action / owner.** Block RM-004-I. RM-004-D follow-up: define and persist tombstone/API/export behavior (including multi-target splits), then add cross-project old-client and rollback fixtures.

### High — a damaged pinned RockServer snapshot silently substitutes an unrelated six-row catalog

**Evidence.** `C:\repos\rockserver\src\search\mod.rs`, `InMemoryStationRepository::with_builtin_catalog`, calls `PinnedSharedCatalog::load()` then `unwrap_or_else(|_| Self::legacy_fixture_catalog())`. The fixture has six development stations; the reviewed release has 41. `src/catalog/shared.rs` correctly rejects checksum, malformed, and incompatible bytes, but the result is not a previous pinned release.

**Impact / likelihood.** A bad build artifact or incompatible pin produces a successful server startup with materially different IDs and streams, rather than a conspicuous failed activation or verified previous release. Likelihood is low for immutable bytes, but impact is high and RM-004-H explicitly gates corrupt/incompatible-snapshot fallback.

**Recommended action / owner.** Block RM-004-I. RM-004-D follow-up: fail activation/readiness when the pin is invalid, or vendor and verify a previous release; add an integration test proving no fixture substitution.

### Medium — extended mobile export drops favicon data despite a user-facing SQLite column

**Evidence.** `C:\repos\rockserver\src\mobile_export.rs` declares `favicon_url` in SQLite but `PostgresMobileStation` and `ELIGIBLE_STATIONS_SQL` do not select it; the insert binds literal `NULL`. The shipped package has `non_null_favicon=0` but preserves 15,810 non-null homepages. `docs/rockmobile-extended-catalog.md` does not list favicon as excluded.

**Impact / likelihood.** Any source favicon is silently erased from the extended catalog, reducing offline artwork without a manifest or validation failure. It is systemic if the server gains/preserves favicons.

**Recommended action / owner.** RM-004-D/E: explicitly allow favicon in the mobile projection or document it as excluded; add a non-null export/consumer fixture.

### Medium — `verify` does not verify local schema identity or release metadata

**Evidence.** `C:\repos\rockcast-station-catalog\tools\release_sync.py`, `verify_consumer`, compares local manifest version/checksum and JSON hash, then source pin text. It never compares consumer `stations.v1.schema.json` or `release.json`; RockMobile has no copied schema. This is weaker than `docs/RELEASE.md`'s stated schema/version/manifest drift protection.

**Impact / likelihood.** A consumer can pass `verify` with stale/modified schema artifacts, weakening the release gate. Runtime parsers enforce schema version 1, so this is not an observed runtime mismatch in `2026.08.2`.

**Recommended action / owner.** RM-004-G: include schema digest in release metadata and verify every vendored schema (or explicitly define schema omission), with a negative drift test.

### Medium — RockCast overrides bypass release pin/checksum and the TXT deadline is documentary only

**Evidence.** `C:\repos\rockcast\src\stations\catalog.rs` validates checksum only in `parse_embedded_catalog`; `parse_override` accepts JSON without manifest/version-pin validation or TXT. A unit fixture accepts catalog version `2026.08.9`. Override precedence is environment, executable directory, current working directory, then app-data; JSON precedes TXT. `LEGACY_TXT_REMOVAL` (`not before 2026-10-31`) is warning text, not enforcement.

**Impact / likelihood.** This may be intentional user-override authority, but an unverified JSON/TXT file may replace the baseline indefinitely. Migration removal is dependent on manual process, weakening old-client/rollback safety.

**Recommended action / owner.** RM-004-F/I: state whether unverified full-catalog overrides are supported. If yes, add migration/backup guidance and a deadline/removal acceptance test; otherwise require a manifest/pin for JSON overrides and constrain TXT to transition use.

## Scenario disposition

| Scenario | Review result |
|---|---|
| Primary URL, metadata-only update, secondary stream | Contract validator and server/RockCast/Mobile parsers preserve primary selection and retain secondary streams. Current baseline has one primary each; no live update transition was exercised. |
| Duplicate provider record | Canonical validation rejects duplicate IDs/stream URLs; importer has deterministic source ownership and tests. No live PostgreSQL duplicate-provider run. |
| Deleted/retired station | **Fails architecture gate**: tombstone replacement meaning is lost in RockServer. |
| Server unavailable | RockMobile remote failure routes to local fallback by unit tests. RockCast documents local operation. No device/UI or stopped-server test. |
| Corrupt/malformed/incompatible snapshot | Mobile rejects corruption and falls back from extended to baseline. RockCast rejects invalid overrides and uses embedded data. RockServer uses legacy fixture (High finding). |
| Rollback | Release-tool tests prove explicit sync/verify rollback repeatable; actual consumer verification passed. No rollback was applied in this audit. |
| Old-client compatibility | v1 consumers ignore unknown optional fields, but retirement/replacement is not end-to-end due to lost tombstones. |

## Limitations / not run

No external network calls, live RockServer/PostgreSQL import/export, device/UI smoke test, actual rollback mutation, stream playback, latency instrumentation, or full RockCast/Android suite was counted as passed. The four working trees were already dirty; no existing change was altered. This review reports local-artifact and static/code evidence only where stated.
