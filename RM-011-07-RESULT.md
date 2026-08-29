# RM-011-07 result — A6 + A7

Status: complete locally on 2026-08-29.

## Implementation commit

- `d26fde0c170fd1db5524d3fcf8929d8de4441d40` — `fix(web): separate anonymous registration flow`

## Scope and changed files

- `web/src/app.tsx` — anonymous landing actions, `/register` state/path handling, explicit
  default-name selection, click-only/double-submit-safe ceremony, cancellation recovery, and the
  pairing-only return path.
- `web/src/style.css` — small registration-form presentation helpers.
- `web/tests/ux-regression.mjs` — A6/A7 source-level regressions.

## Verified result

- Anonymous `/` presents primary `Войти с passkey` and secondary `Создать Rock-аккаунт`, explaining
  existing-account sign-in versus new-account creation. It adds no pairing code, UUID, or manual
  token field.
- `/register` has a dedicated heading, labelled name field and example, existing-account back
  action, and explicit `Использовать «Rock account»` default selection. Empty input does not start
  a ceremony.
- Registration starts only from its primary click. Busy controls are disabled, a ref blocks a
  second submission, and cancelled passkey prompts retain the filled form with recovery text.
- Successful pairing-origin registration returns to the original pairing confirmation context.
  Standalone registration shows that the browser is signed in and links to the account centre.

## Checks

- `cd web && pnpm test` — passed, 11/11.
- `cd web && pnpm build` — passed: TypeScript typecheck, lint, Vite production build.
- `cargo fmt --check` — passed.
- `cargo clippy --all-targets --all-features -- -D warnings` — passed.
- `cargo test` — passed: 103 library tests plus all available binary/integration suites.
- `git diff --check` — passed.

## Limitations

- No staging deploy, push, browser passkey ceremony, account/device data operation, or native-client
  work was performed.
- A4 fragment/App Link/handoff URL behavior, OpenAPI/server DTO/API behavior, RockMobile, and
  RockCast are unchanged.
- Six PostgreSQL tests require a disposable `TEST_DATABASE_URL`; six external live tests require
  credentials and remain ignored.
