# RM-011-01 result — A1 + A2

Status: complete locally on 2026-08-29.

## Commit

- Implementation commit: `9bb39aca30fb5a69e5143388c53e97686110132d`

## Changed files

- `web/src/app.tsx`
- `web/tests/ux-regression.mjs`
- `docs/status.md`
- `docs/tasks.md`

## Implemented requirements

- Split editable `registrationName` from verified `authenticatedAccountName`.
- Registration input no longer changes the pairing lifecycle or unlocks approval.
- Approval requires a pending preview, authenticated account, CSRF token, and the exclusive
  `authenticated` pairing state.
- Added the existing-Preact-only pairing states `loading`, `anonymous`, `authenticated`,
  `approving`, `approved`, `terminal`, and `unavailable`; no router or state-machine dependency was
  added.
- A successful approve `204` enters `approved`, hides pending target/phrase/code/expiry and all
  sign-in, registration, and approval controls, and shows an unambiguous success screen.
- RockMobile success offers `Вернуться в RockMobile`; RockCast tells the user to return to RockCast;
  both offer `Открыть аккаунт и устройства`.
- Reload of an approved/consumed request receives no pending preview and therefore renders no
  enabled approval action.
- Added regressions for first-letter registration input, missing authenticated session/CSRF,
  post-204 success, and approved/consumed reload.

## Verification

- `cd web && pnpm test` — passed, 8/8.
- `cd web && pnpm build` — passed; TypeScript typecheck, lint, and Vite production build.
- `cargo fmt --check` — passed.
- `cargo clippy --all-targets --all-features --jobs 1 -- -D warnings` — passed.
- `cargo test --all-targets --all-features --jobs 1` — passed for all available tests: 103 library
  tests plus binary/integration suites.
- `git diff --check` — passed before commit.

## Known limitations

- No staging deploy or manual browser/passkey smoke was performed.
- The RockMobile return CTA targets the planned `/return/rockmobile` path; verified App Link
  handling belongs to A4 and was intentionally not implemented in this wave.
- Six PostgreSQL tests require a disposable `TEST_DATABASE_URL` and remained ignored.
- Six live tests require external credentials or local ONNX assets and remained ignored.
- The checked-in web suite is source-level regression coverage; no browser DOM runner is installed
  in this project.
- OpenAPI, server DTOs/handlers, RockMobile, and RockCast were intentionally unchanged.
