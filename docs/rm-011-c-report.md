# RM-011-C — итоговый отчёт текущего прогона

Дата: 2026-08-26
Репозиторий: RockServer
Ветка: `master`
Статус: **частично реализовано; полностью закрывать RM-011-C пока нельзя**

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
- `cargo test --jobs 1` — успешно: 92 теста; 4 PostgreSQL suites пропущены без
  `TEST_DATABASE_URL`.
- `cargo test --test openapi_contract --jobs 1` — успешно.
- Web `tsc --noEmit` — успешно.
- Vite production build через bundled Node runtime — успешно.
- `git diff --check` — успешно; только стандартные предупреждения о CRLF.
- Параллельные compilation jobs не запускались.
- `git push` не выполнялся.

## Почему задача ещё не закрыта

1. Не выполнен live прогон disposable PostgreSQL integration suites в текущем окружении.
2. End-to-end WebAuthn требует HTTPS origin `https://alex.vault57.ru`; обычный localhost HTTP
   проверяет UI/API, но не заменяет production-origin ceremony.
3. Browser UI показывает успешное approval, а native RockCast должен отдельно вызвать
   `/complete` и получить access/refresh credentials.
4. В текущем срезе не добавлены отдельные native refresh/logout/device-management HTTP routes из
   расширенного RM-011-A proposal; B2 persistence primitive для refresh rotation сохранён.
5. Caddy binary не установлен на host, поэтому локальная валидация Caddyfile выполняется в
   контейнерном/image path.

## Итоговое решение

RM-011-C имеет рабочий локальный implementation slice и пройденные deterministic checks, но
остаётся **частично закрытой** до live PostgreSQL/E2E security verification и решения по
оставшимся native session routes.
