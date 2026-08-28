# RM-011-G7 — сквозная проверка account/pairing UX

Дата проверки: 2026-08-28  
Результат: **функциональные блокеры F-1/F-2/F-3 сняты в текущих checkout**. Контракт и
детерминированные проверки трёх репозиториев проходят, но полноценный live-flow с passkey,
PostgreSQL, GUI, физическим Android-телефоном и staging в этой среде не выполнялся, поэтому
эпик всё ещё требует ручной smoke-приёмки.

## Ограничения и безопасность проверки

- Проверены только текущие checkout; staging `https://alex.vault57.ru` не открывался для
  изменения, очистки, регистрации или revoke.
- Не использовались реальные passkey, токены, API keys, аккаунты или внешняя сеть.
- Local browser smoke использовал credential-free deterministic harness. Успешный live-passkey
  или physical-device test не имитировался.
- PostgreSQL migration и destructive cleanup не запускались. Disposable PostgreSQL tests
  остались skipped/ignored, потому что `TEST_DATABASE_URL` не задан.
- В RockCast и RockMobile внесены только точечные изменения pairing UX и локальных quality-gate;
  staging и git-история не затрагивались.

## Проверенные revision

| Репозиторий | Branch / revision | Рабочее дерево |
| --- | --- | --- |
| `C:\repos\rockserver` | `master` / `ad2bc3bfab05fff3dfb36e466ead75b226832a82` | dirty: исходные изменения плюс текущие server/docs/contract fixes |
| `C:\repos\rockcast` | `master` / `186c1a6cb7fd02b177e8a5c993fd02d0039e760f` | dirty: текущие pairing UX, terminal-state и quality-gate fixes |
| `C:\repos\rockmobile` | `master` / `e0249a2ddc89cb06eb1b4af22805304959683907` | dirty: текущие pairing UX и lint fixes |

## Фактический статус реализации

### RockServer G1/G2 и browser UX

Реализованы маршруты account, WebAuthn, browser session, pairing approval/completion и browser
account/device centre. Registration/authentication используют discoverable username-less passkey;
owner для assertion и pairing completion выводится сервером, а не принимается из `user_id` клиента.
Approval требует first-party HTTPS origin, trusted proxy proof, browser cookie и CSRF. Native
completion принимает только `desktop_token`, выдаёт native access/refresh session, а браузеру
native tokens не передаёт. Device revoke owner-scoped, отзывает связанные native sessions/tokens;
лимит active devices — 10.

Browser pairing page показывает тип и display name целевого устройства, verification phrase,
short code, expiry и две разные операции — «Войти с passkey» и «Создать Rock-аккаунт». UUID,
credential IDs и native tokens в основном UI не отображаются. После refresh pairing query context
сохраняется.

В ходе этой проверки исправлена рассинхронизация контракта: runtime уже делает `/v1/search` и
`/v1/voice/*` anonymous/rate-limited, а OpenAPI описывал их как Bearer-protected. OpenAPI теперь
показывает Bearer только для legacy `/api/v1/*` compatibility routes; structural contract test
проверяет обе границы. Затронуты `api/openapi.yaml`, `tests/openapi_contract.rs` и verified
описания в `docs/service-diagrams.html`.

Регистрация теперь откладывает создание пользователя до успешного WebAuthn assertion. Challenge
хранит server-only context с зарезервированным account ID и display name; транзакция атомарно
потребляет challenge, создаёт user/passkey/browser-session и пишет audit event. Ошибка ceremony,
отмена браузера, duplicate credential или ошибка БД не оставляют active account без passkey.
Completion pairing теперь возвращает различимые `202 pairing_pending`, `401 pairing_rejected`,
`409 device_limit_reached` и `410 pairing_expired`; OpenAPI и structural tests обновлены.

### RockCast G4

RockCast создаёт pairing request с display name/type, строит secure deep link, показывает QR,
phrase, short code и expiry, хранит proofs только в памяти, отправляет при completion только
`desktop_token` и сохраняет native credentials через OS-protected store. Account centre показывает
account/device names, текущий PC, список устройств, refresh/logout/revoke; локальное радио не
зависит от account service. UUID и bearer/token fields остаются внутренними.

Unit tests покрывают request/response shape, отсутствие Authorization на create, deep link,
отсутствие `user_id` в completion, refresh replay, offline logout и polling, включая terminal
статусы `401/409/410`. Background worker сохраняет credentials только после проверки отмены, а
закрытие account window отменяет pending pairing. Build/check/clippy проходят. Физический
Windows GUI и DPAPI profile здесь не запускались.

### RockMobile G5

RockMobile создаёт pairing request для `rockmobile_android`, формирует QR/secure link, хранит
pending context в ViewModel, возобновляет polling после возврата activity из браузера, сохраняет
native credentials в Android Keystore-encrypted store и показывает account/device names без UUID.
На disconnected screen явно сказано, что новый аккаунт здесь не создаётся; anonymous radio и
offline catalog остаются доступны. Refresh rotation, logout и revoke реализованы.

Account unit tests покрывают completion без `user_id`, URL, refresh/logout, timeout, terminal
`202/401/409/410`, unavailable, device-list failure и revoke/logout. Закрытие диалога вызывает
cancel перед dismiss, а ViewModel проверяет, что отменённый pending request не сохраняет результат.
Android emulator/install не запускались.

## E2E checklist / test matrix

Обозначения: **PASS** — подтверждено автоматикой или deterministic local smoke; **PARTIAL** —
часть подтверждена, live contour нужен; **BLOCKED** — найден дефект; **MANUAL** — намеренно не
выполнялось в этой среде.

| Сценарий | Evidence | Результат |
| --- | --- | --- |
| Чистый пользователь создаёт один account через discoverable passkey | Server WebAuthn unit/negative tests; browser UI has separate create action; no live credential assertion | PARTIAL / MANUAL |
| RockCast подключается к существующему account | G4 source contract + 89 client tests + compile/check/clippy | PARTIAL; staging/Windows GUI MANUAL |
| RockMobile подключается к тому же account | G5 source contract + Android unit tests + lint + debug assemble | PARTIAL; physical Android MANUAL |
| Browser показывает target device/account без self-selection | Local harness showed `RockMobile — Этот телефон`, phrase, short code, expiry; source binds approval to request/account | PASS for deterministic UI; full account approval MANUAL |
| После passkey login pairing context не теряется | Local browser refresh preserved `?code=...&secret=...`; Mobile lifecycle has resume; native GUI MANUAL | PASS local / PARTIAL live |
| Cancelled passkey / cancelled pairing | User-facing error mapping and client cancel tests exist; DB/browser ceremony not run | PARTIAL |
| Expired / already-used pairing | Server returns `410`; RockCast/Mobile stop on terminal response; SQL expiry/consume integration remains opt-in | PARTIAL; disposable DB + live smoke MANUAL |
| Повторное подключение уже подключённого устройства | Consumed/revoked/expired requests return terminal `410`; clients no longer retry them as pending | PARTIAL; live account smoke MANUAL |
| Browser/server unavailable | Web source maps unavailable state; RockCast/Mobile preserve local radio; local authenticated harness displayed safe unavailable alert | PASS for fallback handling; live network outage MANUAL |
| Native refresh rotation | Server unit/source and RockCast/Mobile client tests cover rotation/replay clearing | PARTIAL; PostgreSQL integration ignored |
| Logout current device | Client tests and source clear local credentials; server revoke/logout paths exist | PARTIAL; live device/session check MANUAL |
| Revoke another device and current-device marker | UI/source and client tests present; server ownership/cascade integration is ignored | PARTIAL; live account centre MANUAL |
| 10-device limit and useful error | Server returns `409 device_limit_reached` with limit detail; both clients stop and show a useful message | PARTIAL; disposable DB + live 11th-device smoke MANUAL |
| No UUID, manual bearer token, or native token in normal UX | Source scan, OpenAPI DTO boundaries, UI snapshots, client tests; no account token settings in Mobile | PASS static/deterministic |
| No secret leakage in browser logs/server account sources | Browser console logs empty; redaction tests and scoped source scan passed | PASS for checked surfaces |
| Anonymous radio and fallback catalog without server | RockCast local/fallback tests, RockMobile local baseline/SQLite paths and source; no server needed | PASS static/unit; physical offline playback MANUAL |

The credential-free local browser harness covered these visible states:

1. anonymous pairing page with target `RockMobile`, display name, phrase, short code, expiry,
   separate sign-in/create actions and no native secret;
2. authenticated account heading with account name and safe server-unavailable alert;
3. refresh retaining the original pairing query;
4. empty browser console log set.

The authenticated harness does not implement the full browser device API, so it is not evidence
for live rename/revoke or a real account list.

## Commands and results

### RockServer

- `cargo fmt --check` — PASS.
- `cargo clippy --all-targets --all-features -- -D warnings` — PASS.
- `cargo test` — PASS.
- `cargo test --all-targets --all-features --jobs 1` — PASS: 103 library tests, 4 cleanup CLI
  tests, 1 diagnostic-redaction test, 6 deployment/security tests, 2 OpenAPI tests, 9 search API
  tests, 5 voice-command tests and 1 voice-stream test. Six disposable PostgreSQL tests, one
  ONNX asset test, four billable Yandex LLM tests and one SpeechKit live test remained ignored.
- `docker compose config --quiet` — PASS; Docker emitted only a local config-file access warning.
- `git diff --check` — PASS (line-ending conversion warnings only on the pre-existing dirty tree).

### RockCast

- `cargo fmt --check` — PASS.
- `cargo check --all-targets --all-features --target-dir C:\repos\rockserver\target\rm-011-g7-rockcast` — PASS.
- `cargo test --all-targets --target-dir C:\repos\rockserver\target\rm-011-g7-rockcast` — PASS:
  89 unit tests; live stream/network tests ignored.
- `cargo clippy --all-targets --all-features --target-dir C:\repos\rockserver\target\rm-011-g7-rockcast -- -D warnings` — PASS.
- `git diff --check` — PASS; checkout has only the current uncommitted changes listed above.

### RockMobile

- `:app:testDebugUnitTest --no-daemon` using the existing local Gradle 9.3.1 distribution — PASS.
- `:app:assembleDebug --no-daemon` — PASS.
- `:app:lintDebug --no-daemon` — PASS after adding the runtime microphone permission guard,
  moving the API-27 navigation-bar item to `values-v27`, and escaping the existing local SDK path.
- Emulator, install and Android GUI tests — MANUAL, not run.
- `git diff --check` — PASS; checkout has only the current uncommitted changes listed above.

### Web and browser

The normal `pnpm` wrapper was not used after it attempted a non-interactive modules-directory
mutation and stopped with `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`. Existing bundled Node and
already-installed modules were used instead:

- TypeScript `tsc --noEmit` — PASS.
- `node --test web/tests/ux-regression.mjs` — PASS, 5/5.
- Vite production build — PASS.
- Credential-free browser harness — PASS for the visible states listed above; no passkey assertion
  was attempted.

## Actionable findings

### F-1 — P1 — RESOLVED in current checkout: terminal pairing states are distinguishable

**Owners:** RockServer contract + RockCast G4 + RockMobile G5.

The server now classifies the locked request before completion: pending approval returns `202
pairing_pending`, an invalid proof remains neutral `401 pairing_rejected`, expired/consumed/revoked
requests return `410 pairing_expired`, and the active-device cap returns `409
device_limit_reached` with `{limit:10}`. OpenAPI and contract tests cover the response surface.

RockCast maps those statuses to separate `Pending`, `Rejected`, `Expired` and `DeviceLimit`
outcomes; RockMobile retries only `202` and presents a terminal limit/expired/rejected message.

**Regression evidence:** RockCast unit tests exercise `202`, `401`, `409` and `410` mappings;
server OpenAPI tests require all completion statuses. A disposable PostgreSQL run and live
11th-device/consumed-request smoke test remain manual because no test database/staging was used.

**Next verification:** run the existing disposable PostgreSQL integration contour and confirm the
same statuses through a real browser/native pairing. No further code blocker is open here.

### F-2 — P1 — RESOLVED in current checkout: dismiss cancels native pairing

**Owners:** RockCast G4 and RockMobile G5.

RockCast now cancels on the explicit button and on account-window close; its worker obtains the
completion result without persistence, checks the cancellation flag, then writes to secure storage.
RockMobile routes both `AlertDialog.onDismissRequest` and the close button through a callback that
cancels the ViewModel, and `AccountViewModel` rechecks the pending identity before saving.

**Regression evidence:** the changed worker/ViewModel paths are covered by local compilation and
unit tests; no live GUI race test was possible without Windows/Android UI and a real account.

**Next verification:** repeat the close-then-approve smoke sequence on real Windows and Android.

### F-3 — P2 — RESOLVED in current checkout: cancelled registration is atomic

**Owner:** RockServer G1/G2.

`registration_options` now stores only an opaque server-side ceremony context and does not insert a
user. `complete_passkey_registration` commits challenge consumption, user, passkey, browser
session and audit event in one transaction; rollback covers cancelled/failed/duplicate/DB-error
paths. Disposable PostgreSQL verification is still required to exercise the SQL transaction.

### F-4 — P2 — RESOLVED in current checkout: RockCast Clippy gate is green

**Owner:** RockCast. The callback-rich `LocalPlayer::play` API keeps its existing shape with a
targeted Clippy allowance and documentation; the full all-target/all-feature strict Clippy run
passes.

### F-5 — P2 — RESOLVED in current checkout: RockMobile lint gate is green

**Owner:** RockMobile. The microphone path now checks `RECORD_AUDIO`, the API-27 style item is
qualified, and the existing local SDK path is valid property syntax. `:app:lintDebug` passes.

## Manual staging / real-device smoke checklist

This checklist is **not executed by this task** because the task forbids staging changes and no
physical devices are attached. Run later in a separately authorized, disposable staging contour:

1. Start from a clean browser profile and clean test install. From RockCast choose «Connect this PC
   to an account», create exactly one Rock account with a discoverable passkey, and record only
   human-readable account/passkey names.
2. Verify the browser approval page says the exact account name, `RockCast`, PC display name,
   phrase, short code and expiry. Confirm there is no UUID, credential ID, bearer token or second
   account choice hidden in the normal path.
3. Install/run RockMobile, choose «Подключить этот телефон к аккаунту», open its secure link, and
   select the existing passkey. Verify the browser target is explicitly `RockMobile — <name>` and
   the approval cannot be mistaken for the PC/self device.
4. Return to both apps. Verify one account contains both named devices, current-device markers are
   correct, restart preserves sessions, and no secrets appear in UI/logs.
5. Verify logout on each device, revoke the other device, refresh-token rotation/replay behavior,
   and that the revoked device cannot silently reconnect.
6. Separately exercise cancel, expired link, already-used link, unavailable server, refresh while
   pairing, dismiss/close while pairing, and the 11th-device limit. Record whether each gives a
   terminal human-readable result promptly.
7. Disable/unreach RockServer and play from both clients. Confirm baseline/fallback catalog and
   local playback remain usable.

## Criteria to close RM-011-G

- F-1/F-2/F-3 are resolved in the current checkouts; run the disposable-DB and live
  smoke evidence below before accepting the epic.
- RockServer PostgreSQL account/pairing/browser-centre tests run against a disposable database;
  no destructive staging migration is needed.
- Manual checklist repeats on staging with one account, physical Android and Windows GUI; no
  manual UUID/token entry, no ambiguous self-selection, and offline radio remains usable.
- RockCast and RockMobile local quality gates pass; only real-device/browser/staging evidence
  remains.

Pairing fixes are in RockServer commit `99c2783`, RockCast commit `0bd9639`, and RockMobile commit
`fd1693d`. Deploy/backup hardening is in RockServer commits `392d8b4` and `26906ef`; they reached
`origin/master`, and OPS-001-D completed successfully with readiness passed for the immutable
RockServer image `sha256:bf80ed529b0a7c1ffc14c2333730851fdaddfadf401a90a1c03a6052e190cd3f`.
The real passkey/device smoke checklist remains manual; no staging account data was changed by the
verification run.
