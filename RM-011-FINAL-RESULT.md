# RM-011 final integration result

Date: 2026-08-29

## Outcome

RockServer staging deploy is complete for
`e171096e55ce1b5912fb76615c584775313a8fb9`. The deploy worker reported
`status=succeeded`; public `GET /health/ready` returned `200`. No push was performed.

`/.well-known/assetlinks.json` is now a checked-in Vite public asset and was served publicly as
`200 application/json` after deploy. Its single Android target is `com.rockmobile` with relation
`delegate_permission/common.handle_all_urls` and release certificate SHA-256
`92:E7:BE:49:13:A9:4A:B5:0E:37:58:7B:AB:A1:3F:36:0D:49:47:C4:83:0C:DF:DE:10:40:21:3E:63:42:53:9`.
The public fingerprint was obtained with `apksigner verify --print-certs` from the authorized local
signed release APK; private keystore material was neither read nor recorded.

## Repository state

| Repository | Master at audit | RM-011 state |
| --- | --- | --- |
| RockServer | `e171096` | Clean; App Link asset committed and deployed. |
| RockMobile | `3b0c47e` | Root worktree clean; source declares `0.1.1` / `versionCode=2`. One separate detached user worktree is dirty (`AndroidManifest.xml`, `build.gradle.kts`) and was not touched. |
| RockCast | `00f6d1e` | Clean; crate version `0.1.0`. |

All RM-011 RESULT report commits are ancestors of their repository masters except the RockMobile
Wave 2 report's recorded implementation `a1d7acbcd49d47911dabc46bd92c747a31408838`. That commit
exists but is not an ancestor of master; `98b7e09` is the corresponding current master change. The
report was preserved as historical evidence rather than silently rewritten.

## Contract audit

- Server, RockMobile and RockCast generate the handoff as `?code=<code>#secret=<proof>`.
- The web parser keeps the secret only in memory, calls `history.replaceState` before lookup, and
  accepts legacy query input only through 2026-09-29T00:00:00Z. The 10-test web regression suite
  asserts scrubbing and absence of browser console output.
- RockMobile's verified App Link is only HTTPS `alex.vault57.ru/return/rockmobile`, without query
  or fragment. Assetlinks is intentionally app-wide at the Android association layer; the manifest
  supplies the narrow path constraint.
- Current completion compatibility remains unversioned: pending is `202 pairing_pending`; no
  `pairing_protocol_version` is implemented. Client reports record safe status/code/request-id
  behavior and raw-name/single-prefix handling.

## Wave commit/report audit

| Repository | Wave | Implementation | RESULT report |
| --- | --- | --- | --- |
| RockServer | 1 | `9bb39ac` | `ea6356c` |
| RockServer | 7 | `d26fde0` | `a5a1ab0` |
| RockServer | 8 | `550a4da` | `150627b` |
| RockServer | 9 | `2128772`, `38ecf1d` | `a595571` |
| RockMobile | 2 | `a1d7acb` (non-ancestral; current `98b7e09`) | `f4b4722` |
| RockMobile | 4 | `616f5ce` | `a409f00` |
| RockMobile | 5 | `7cb33b7` | `dc26c19` |
| RockMobile | 9 | `710c695` | `3b0c47e` |
| RockCast | 3 | `fadd5b9` | `7656090` |
| RockCast | 6 | `08c9cc6` | `e775852` |
| RockCast | 9 | `32ac717` | `00f6d1e` |

This final integration's deliberate code/asset commit is `e171096`.

## Verification

- `web`: `pnpm test` passed 10/10; `pnpm build` passed and copied the assetlinks file into `dist`.
- RockServer: `cargo fmt --check`, strict all-target/all-feature Clippy, and `cargo test` passed.
  The test run includes 103 library tests; disposable PostgreSQL and credential/live tests remain
  intentionally ignored.
- Release: documented dry-run passed; Docker Engine `29.7.2` was available; commit-bound RockServer
  and Caddy images built; remote worker status succeeded; readiness and assetlinks content were
  validated over public HTTPS.

## E2E and limitations

No disposable account/request was created and no staging E2E was run. This host had no `adb` or
connected Android device, and no running RockCast process. Real passkey selection, physical QR
scan, App Link verification on an installed release APK, Mobile/RockCast connected-first-time
states, device list, browser referrer/log observation and safe live request tuple therefore remain
unverified. No user interaction or security control was bypassed.

No account/device/session mutation, destructive database operation, credential/proof/token/cookie
inspection, or remote push occurred.
