# RM-011-C — итоговый отчёт текущего прогона

Дата: 2026-08-26
Репозиторий: RockServer
Ветка: `master`
Статус: **RM-011-C server/browser implementation verified**

## Что реализовано

- Криптографические WebAuthn registration/authentication ceremonies на `passkey-auth`.
- Фиксированные RP ID и origin: `alex.vault57.ru` и `https://alex.vault57.ru`.
- Проверки client data, challenge, ceremony, RP ID, user verification, подписи и sign-counter
  rollback/clone policy.
- Browser-сессии через `HttpOnly; Secure; SameSite=Strict` cookie и double-submit CSRF.
- Fail-closed browser state changes: точный first-party Origin, HTTPS marker и секретный proof,
  который добавляет только Caddy.
- PostgreSQL-backed pairing requests, challenge state, browser cookie hashes и rate-limit buckets.
- Pairing routes:
  - `POST /v1/pairing-requests`
  - `GET /v1/pairing-requests/lookup`
  - `POST /v1/pairing-requests/{request_id}/approve`
  - `POST /v1/pairing-requests/{request_id}/complete`
  Completion accepts only `{ "desktop_token": "…" }`; its account owner is derived atomically
  from the approved pairing request and is never supplied by the native client.
- Native session/account routes:
  - `POST /v1/auth/refresh`
  - `POST /v1/auth/logout`
  - `GET /v1/account/profile`
  - `DELETE /v1/account` (requires native bearer plus fresh browser passkey/CSRF proof)
  - `GET /v1/devices`
  - `DELETE /v1/devices/{device_id}`
- Passkey routes:
  - `POST /v1/auth/passkeys/registration/options`
  - `POST /v1/auth/passkeys/registration/verify`
  - `POST /v1/auth/passkeys/authentication/options`
  - `POST /v1/auth/passkeys/authentication/verify`
- Owner-scoped credential lookup, transactional pairing approval/completion и лимит 10 устройств.
- Единый frontend/admin bundle на TypeScript + Vite + Preact в `web/`.
- Same-origin API client, passkey controls, short-code lookup, device/verification phrase display
  и локальный QR rendering без localStorage и без токенов в bundle.
- Caddy static delivery, `/v1` proxying, CSP/security headers и deployment-only proxy secret.
- Runtime `api/openapi.yaml`, `.env.example`, Compose/Caddy wiring и проектная документация
  обновлены.

## Проверки

- `cargo fmt --check` — успешно.
- `cargo clippy --all-targets --all-features --jobs 1 -- -D warnings` — успешно.
- `cargo test --jobs 1` — успешно: 93 regular tests.
- `TEST_DATABASE_URL=postgres://… cargo test --test postgres_integration --all-features --jobs 1 -- --ignored --test-threads=1`
  — успешно: 4/4 в одноразовом локальном контейнере `pgvector/pgvector:pg17`.
- `cargo test --test openapi_contract --jobs 1` — успешно.
- Web `tsc --noEmit` — успешно.
- `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm lint` и `pnpm build` через bundled
  Node runtime — успешно.
- `git diff --check` — успешно; только стандартные предупреждения о CRLF.
- Параллельные compilation jobs не запускались.
- `git push` не выполнялся.

## Граница задачи

Autonomous server + first-party browser UI verified: cryptographic WebAuthn ceremonies, browser
cookie/CSRF/proxy policy, pairing state machine, PostgreSQL persistence/API checks and the unified
Vite + Preact build are covered by this task. Full real-client staging E2E, including the actual
RockCast completion client and RockMobile session UX, is deferred to **RM-011-D/E**; it is not a
prerequisite for RM-011-C.

## Итоговое решение

**RM-011-C complete:** server/browser implementation is verified locally, including disposable
PostgreSQL and Caddy config validation. Full real-client staging E2E is explicitly deferred to
RM-011-D/E.
