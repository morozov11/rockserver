# Исполнимый план интернет-беты

**Статус:** ниже сохранён план задач; MVP-001-A и RM-011-A contract/threat-model одобрены
владельцем. Реализация account/device API не начата.
**Основание:** [общий roadmap](shared-product-roadmap.md).  
**Правило:** одна задача Codex — один ограниченный результат. Не запускать задачи параллельно,
если не указано обратное. Перед каждой задачей прочитать `AGENTS.md` целевого репозитория,
сохранить несвязанные изменения и не использовать production secrets, сеть или живых провайдеров
без отдельного решения пользователя.

## Сквозные решения, которые нельзя менять без отдельного approval

- RockServer — единственный владелец аккаунтов, сессий, устройств, синхронизации и команд.
- RockCast/RockMobile остаются функциональны без входа и при отключённом RockServer.
- Общие station ID и lifecycle/replacement semantics берутся из RM-004.
- Новый HTTP-контракт сначала фиксируется в OpenAPI и versioned `/v1` endpoints.
- ESP32 не получает пароль, основной refresh token или полную копию каталога.

## MVP-001 — Публичный анонимный MVP без общего токена

### MVP-001-A — Public API contract and abuse model

- **Репозиторий:** RockServer; docs/OpenAPI proposal only.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** определить точный allowlist анонимных `/v1` endpoints для каталога, поиска и voice,
  а также отдельно protected/admin/account/device endpoints. Зафиксировать rate limits, лимиты
  аудио/сессий/параллельности, ошибки, метрики и правила безопасных логов. Запретить shared
  bearer token и любые клиентские секреты как способ авторизации публичного пользователя.
- **Готово, когда:** human-approved OpenAPI/threat model описывает анонимный пользовательский
  путь и стоимость/защиту voice API; будущая account auth не меняет публичный MVP contract.
- **Зависимости:** нет.
- **Статус:** approved владельцем 2026-08-26; MVP-001-B разрешена к реализации в границах
  [mvp-001-a-public-api-contract.md](mvp-001-a-public-api-contract.md).

### MVP-001-B — RockServer: anonymous public endpoint policy

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** реализовать approved allowlist и application-level ограничения для публичных
  search/voice endpoints: rate limit, payload/duration/concurrency limits, безопасные ответы и
  метрики. Сохранить bearer auth для protected endpoints; ключи Yandex остаются только в server
  environment. Не добавлять user accounts или shared client secret.
- **Готово, когда:** анонимный запрос может выполнить разрешённый сценарий, protected endpoint
  без auth отклоняется, а тесты покрывают limit/exhaustion/error paths без живого SpeechKit.
- **Зависимости:** MVP-001-A approval. Публичный rollout требует OPS-001-D, но локальная
  реализация и тесты не должны ждать его.

### MVP-001-C — RockCast: zero-config public connection

- **Репозиторий:** RockCast.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** задать production RockServer URL для official release, убрать URL/token из обычного
  пользовательского UI и не отправлять bearer для approved public routes. Сохранить isolated
  developer/test override, не попадающий в release user settings. При ошибке публичного API
  продолжать local catalog/playback.
- **Готово, когда:** чистый профиль RockCast получает search/voice без ручной настройки, а
  отключённый RockServer не блокирует радио; test build может использовать отдельный endpoint.
- **Зависимости:** MVP-001-A approval.

### MVP-001-D — RockMobile: zero-config public connection

- **Репозиторий:** RockMobile.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** задать production RockServer URL для official release, убрать URL/token из обычного
  пользовательского UI и не отправлять bearer для approved public routes. Сохранить isolated
  developer/test override, не попадающий в release user settings. При ошибке публичного API
  продолжать bundled catalog/playback.
- **Готово, когда:** чистая установка RockMobile получает search/voice без ручной настройки,
  а отключённый RockServer не блокирует радио; test build может использовать отдельный endpoint.
- **Зависимости:** MVP-001-A approval, GATE-001.

### MVP-001-E — Anonymous MVP end-to-end gate

- **Репозитории:** RockServer, RockCast, RockMobile; tests/docs/runbook.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** проверить чистую установку обоих клиентов без URL/token, public search/voice
  contract, лимиты и fallback при outage; подтвердить, что protected endpoints не стали
  публичными. Живой Yandex и публичный нагрузочный тест запускаются только по отдельному решению
  владельца.
- **Готово, когда:** выпущен severity-ranked go/no-go report для анонимного MVP, без открытого
  shared secret или unresolved high-severity abuse/security issue.
- **Зависимости:** MVP-001-B, MVP-001-C, MVP-001-D, OPS-001-D.

## Нулевая готовность среды

### GATE-001 — Подтверждение RM-004-I на Android

- **Репозиторий:** RockMobile.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** предоставить читаемый Android API-36 SDK, принять требуемые лицензии в окружении,
  запустить существующие unit/lint и короткий offline smoke с выключенным RockServer.
- **Готово, когда:** RM-004-I больше не имеет environment blocker. Никаких новых функций.
- **Зависимости:** нет.

## RM-007 — Локальные избранное и история

### RM-007-A — Общая модель личных данных и миграция идентификаторов

- **Репозитории:** RockServer (только contract/docs), RockCast, RockMobile.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** зафиксировать переносимую модель `Favourite`, `PlaybackHistoryEntry` и `LocalProfile`;
  правила retention, дедупликации, URL-change/replacement/tombstone handling и будущей
  синхронизации. Не создавать серверную авторизацию или sync endpoints.
- **Готово, когда:** один документ/contract описывает совместимые поля, stable station ID и
  migration policy для обоих клиентов; открытые решения выделены для approval.
- **Зависимости:** RM-004 baseline и GATE-001.

### RM-007-B — RockMobile: избранное и история офлайн

- **Репозиторий:** RockMobile.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** реализовать локальное хранение избранного, истории, last-played и launch count;
  добавить UI управления и миграцию от существующего состояния, если оно есть.
- **Готово, когда:** данные переживают перезапуск, не меняются от URL change и работают с
  baseline/extended/remote catalog без сети; unit tests покрывают replacement и retired station.
- **Зависимости:** RM-007-A.

### RM-007-C — RockCast: избранное и история офлайн

- **Репозиторий:** RockCast.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** реализовать эквивалентное локальное хранение и UI без изменения playback/relay
  поведения.
- **Готово, когда:** offline persistence и migration policy соответствуют RM-007-A; тесты
  покрывают add/remove/history/replacement.
- **Зависимости:** RM-007-A.

### RM-007-D — Межклиентская проверка RM-007

- **Репозитории:** RockServer, RockCast, RockMobile; только tests/docs.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** сравнить одну и ту же выборку station ID и lifecycle cases в обоих клиентах;
  проверить offline-first, миграцию и отсутствие сетевой зависимости.
- **Готово, когда:** нет incompatible local data shape и результат записан в docs.
- **Зависимости:** RM-007-B, RM-007-C.
- **Статус:** подтверждена владельцем для перехода к RM-011-A 2026-08-26. Исторический
  технический report и его evidence остаются в `rm-007-d-cross-client-review.md`.

## OPS-001 — Основа безопасного интернет-развёртывания

### OPS-001-A — Production deployment design

- **Репозиторий:** RockServer; только `deploy/` и документация.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** спроектировать single-VPS Docker Compose: `caddy`, `rockserver`, `postgres`,
  закрытая сеть БД, volumes, production env contract, health/readiness, firewall matrix,
  backup/restore и rollback. Не создавать VPS и не публиковать сервис.
- **Готово, когда:** design review одобрен; в нём нет секретов и определён точный домен/ports,
  ownership и recovery procedure.
- **Зависимости:** нет; можно выполнять параллельно с RM-007-B/C после RM-007-A.

### OPS-001-B — Reproducible container and Compose stack

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** добавить reproducible Dockerfile, Compose base/prod override, Caddyfile templates,
  non-secret environment example, healthchecks and local deployment verification.
- **Готово, когда:** чистая машина способна поднять стек из документации; наружу публикуется
  только reverse proxy; Postgres недоступен извне.
- **Зависимости:** OPS-001-A approval.

### OPS-001-C — CI image, release, backup and rollback runbook

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** CI builds/tests immutable commit-SHA image; manual release gate; deploy script
  performs backup, preflight/migration, readiness and supports previous-image rollback. Добавить
  pg_dump/pg_restore restore rehearsal в non-production environment.
- **Готово, когда:** documented dry-run проходит без production secrets, rollback проверен,
  а release не зависит от ручной правки контейнера на VPS.
- **Зависимости:** OPS-001-B.

### OPS-001-D — Automated single-VPS bootstrap and staging launcher

- **Репозиторий:** RockServer; `deploy/`, tests и docs/runbook.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** добавить owner-only ignored inventory, interactive one-time SSH-key bootstrap,
  provisioning Docker/Compose/directories/protected env-file, selective Yandex secret transfer,
  checksum-gated ONNX assets, pinned RM-004 catalog seed and one-command registry-free staging
  deploy. Команда сама получает полный SHA текущего чистого Git commit, локально строит и проверяет
  image, передаёт checksummed `docker save` artifact по SSH и сверяет ID/label после `docker load`.
  Проверить всё локально через dry-run/mocks без VPS или сети.
- **Готово, когда:** владелец заполняет один ignored inventory и запускает documented bootstrap
  либо staging deploy; rollout использует backup, migration/seed и HTTPS readiness, а secrets не
  попадают в Git, argv, logs или release metadata. Реальный публичный запуск остаётся явным
  действием владельца.
- **Зависимости:** OPS-001-C.

## RM-011 — Регистрация, вход и устройства

### RM-011-A — Auth/device contract and threat model

- **Репозиторий:** RockServer; docs/OpenAPI proposal only.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** определить account/session/device/passkey schemas, WebAuthn policy, access/refresh
  lifecycle, token revocation/rotation, rate limits, safe error semantics, delete-account and
  desktop QR/short-code pairing through an optional mobile browser.
- **Готово, когда:** OpenAPI proposal, migration and security review checklist approved человеком.
- **Зависимости:** RM-007-D, OPS-001-B.
- **Статус:** approved владельцем 2026-08-26: contract/threat model recorded in
  `rm-011-a-auth-device-contract.md` and `rm-011-a-openapi.proposed.yaml`. Explicit alternatives
  for recovery, retention and operations remain RM-011-B implementation decisions until separately
  selected; runtime/OpenAPI and implementation unchanged.

### RM-011-B — RockServer: account and session persistence

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** migrations and domain/persistence for users, passkey credentials, browser/desktop
  sessions, refresh-token rotation/revocation, pairing requests, device ownership and audit-safe events.
- **Готово, когда:** deterministic unit/PostgreSQL tests cover WebAuthn validation, QR/code
  approval/expiry/replay, token revoke/rotate, account deletion and ownership isolation.
- **Зависимости:** RM-011-A approval.
- **Уточнение:** задача разделена на `RM-011-B1` (уже выполненный безопасный persistence-subset)
  и `RM-011-B2` (полная WebAuthn/browser-session/pairing persistence). Владелец одобрил
  implementation policy для B2 2026-08-26: RP ID и first-party origin `alex.vault57.ru`,
  синхронизированные passkey разрешены, автоматического recovery после потери всех passkey нет,
  максимум 10 устройств на аккаунт, audit retention 90 дней; также приняты рекомендованные
  сроки access/refresh/browser/pairing и rate-limit значения из RM-011-A.
- **Operational policy:** владелец подтвердил, что доверенным proxy является только Caddy;
  прямые подключения fail-closed, а состояние rate limits хранится в PostgreSQL.
- **Статус:** persistence-часть B2 реализована и проверена unit-тестами и disposable PostgreSQL;
  cryptographic WebAuthn verifier и HTTP API остаются в RM-011-C. Публичный runtime API B2 не
  изменяет.

### RM-011-C — RockServer: public auth and device API

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** implement approved `/v1` OpenAPI endpoints, rate limits and request-safe logging;
  include QR/short-code pairing issuance and redemption without ESP32 firmware.
- **Готово, когда:** API/integration tests pass; tokens and WebAuthn/QR/code secrets never appear in logs/errors;
  anonymous radio endpoints remain compatible.
- **Зависимости:** `RM-011-B2` review passed, применённые миграции и reconciled runtime
  `api/openapi.yaml`. Частичный `RM-011-B1` сам по себе не является достаточной зависимостью.
- **Frontend decision (2026-08-26):** один first-party bundle на TypeScript + Vite + Preact
  обслуживает и passkey/pairing, и будущую админку; общие API-клиент, типы и компоненты живут в
  `web/`. Caddy раздаёт build как статику, а `/v1` и `/api/v1` проксирует в RockServer.
- **Статус реализации (2026-08-26):** локально добавлены passkey registration/authentication,
  криптографическая WebAuthn-проверка с RP/origin/challenge и sign-count guard, browser cookie/CSRF,
  PostgreSQL rate limits, QR/short-code pairing approval/completion и proxy proof от Caddy.
  Единый Preact bundle прошёл TypeScript и Vite production build; native completion по-прежнему
  вызывается RockCast после browser approval, а disposable PostgreSQL прогон остаётся внешним gate.
  Итоговый отчёт `docs/rm-011-c-report.md` фиксирует, что RM-011-C пока частично закрыта: live
  PostgreSQL/E2E security verification и расширенные native session routes остаются незавершёнными.

### RM-011-D — RockMobile: account and secure session UX

- **Репозиторий:** RockMobile.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** optional passkey/browser onboarding, logout, secure platform token storage,
  account/profile state and device list/revoke UI; preserve anonymous and offline flows.
- **Готово, когда:** no token in ordinary app storage/logs; logout clears session; unreachable
  server never blocks radio.
- **Зависимости:** RM-011-C, GATE-001.

### RM-011-E — RockCast: account and secure session UX

- **Репозиторий:** RockCast.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** desktop QR/short-code pairing, passkey/browser approval handoff, session/device
  management using OS-appropriate secure storage; do not put secrets into config files or CLI output.
- **Готово, когда:** same contract tests and anonymous fallback guarantees as RockMobile.
- **Зависимости:** RM-011-C.

### RM-011-F — Closed beta security and integration review

- **Репозитории:** all current clients and RockServer; tests/docs only.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** review threat model implementation, auth failure paths, device isolation, account
  deletion, rate limits, logs and HTTPS deployment assumptions.
- **Готово, когда:** no unresolved high-severity auth/privacy issue; explicit go/no-go report for
  small closed registration beta.
- **Зависимости:** RM-011-D, RM-011-E, OPS-001-D.

## RM-012 — Синхронизация и управление

### RM-012-A — Sync and remote-command contract

- **Репозиторий:** RockServer; docs/OpenAPI proposal only.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** define sync conflict rules for favourites/history/preferences and command model:
  `play(station_id)`, `pause`, `resume`, `stop`, `set_volume`, state acknowledgement,
  idempotency key, offline/unsupported state and authorization.
- **Готово, когда:** human-approved contract resolves concurrent updates and does not leak data
  across account/device boundaries.
- **Зависимости:** RM-011-F.

### RM-012-B — RockServer: synchronized data persistence/API

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** migrations, domain and endpoints for favourites/history/preferences sync using the
  approved conflict and retention rules.
- **Готово, когда:** deterministic PostgreSQL/API tests cover two devices, replay/idempotency,
  delete and offline catch-up without changing catalog ownership.
- **Зависимости:** RM-012-A approval.

### RM-012-C — RockServer: device-command service/API

- **Репозиторий:** RockServer.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** authorization, command persistence/delivery abstraction, acknowledgement/state,
  expiry and audit-safe events. No vendor-specific push dependency unless approved.
- **Готово, когда:** owner can command only own device; retries are idempotent and an offline
  device reports a truthful state.
- **Зависимости:** RM-012-A, RM-011-C.

### RM-012-D — RockMobile: sync and remote-control client

- **Репозиторий:** RockMobile.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** opt-in sync, last-sync status, conflict/error UI and command receive/ack path that
  does not interrupt local playback unexpectedly.
- **Готово, когда:** offline queue/errors are understandable; sync can be disabled and local
  account data can be deleted.
- **Зависимости:** RM-012-B, RM-012-C.

### RM-012-E — RockCast: sync and remote-control client

- **Репозиторий:** RockCast.
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** equivalent sync and command behaviour while preserving playback, relay and Cast.
- **Готово, когда:** cross-client scenario produces equal favourites and safe command handling.
- **Зависимости:** RM-012-B, RM-012-C.

### RM-012-F — Internet-beta end-to-end verification

- **Репозитории:** RockServer, RockMobile, RockCast; tests/docs/runbook.
- **Модель:** `gpt-5.6-sol`, `high`.
- **Работа:** verify staging HTTPS, registration, two client sessions, device revoke, sync,
  remote command, server outage and local-radio fallback; publish severity-ranked go/no-go report.
- **Готово, когда:** no unresolved high-severity user-data/auth/control problem and beta onboarding
  runbook is ready.
- **Зависимости:** RM-012-D, RM-012-E, OPS-001-D.

## ESP32 — только после получения платы

### ESP-001 — Hardware bring-up and capability inventory

- **Репозиторий:** будущий ESP32 repository (создать после подтверждения названия и платы).
- **Модель:** `gpt-5.6-terra`, `medium`.
- **Работа:** проверить Wi-Fi, input controls, screen/audio hardware, power behaviour and safe OTA
  path; никаких аккаунтов/production pairing.
- **Готово, когда:** список реальных возможностей и выбранный UX подтверждены на железе.
- **Зависимости:** плата получена, RM-012-A.

### ESP-002 — Device pairing and remote-control client

- **Репозитории:** ESP32 repository, RockServer only where contract requires.
- **Модель:** `gpt-5.6-terra`, `high`.
- **Работа:** one-time pairing, limited device credential, command/state protocol and revoke;
  implement только подтверждённые ESP-001 controls.
- **Готово, когда:** пользователь привязывает и отзывает ESP32, а утрата платы не раскрывает
  пароль/основную сессию.
- **Зависимости:** ESP-001, RM-012-C, RM-012-F.

## Порядок выдачи задач Codex

`MVP-001-A → (MVP-001-B + MVP-001-C) → OPS-001-A → OPS-001-B → OPS-001-C →
OPS-001-D → GATE-001 → MVP-001-D → MVP-001-E → RM-007-A →
(RM-007-B + RM-007-C) → RM-007-D →
RM-011-A → RM-011-B → RM-011-C → (RM-011-D + RM-011-E) →
RM-011-F → RM-012-A → (RM-012-B + RM-012-C) → (RM-012-D + RM-012-E) → RM-012-F →
ESP-001 → ESP-002`.

Параллельные пары разрешены только после завершения их общей проектной задачи и не должны менять
одни и те же файлы. Перед началом любой задачи, которая затрагивает публичный API, требуется
отдельное human approval её contract/design stage.
