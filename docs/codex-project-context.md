# Канонический контекст для Codex: RockServer, RockCast и RockMobile

> **Назначение.** Это постоянная точка входа для последующих задач Codex по трём
> проектам. Перед началом любой такой задачи сначала прочитать этот файл, затем
> проверить фактическое состояние затронутого кода и документации. Этот документ
> описывает актуальные решения и намерения, а не заменяет код, миграции, OpenAPI
> или `AGENTS.md`.
>
> **Актуальность:** 2026-09-02. Если код или утверждённые документы ниже им
> противоречат, приоритет имеют: (1) безопасность и `AGENTS.md`, (2) фактический
> код и действующий OpenAPI, (3) этот контекст, (4) старые отчёты и обсуждения.

## Как начинать задачу

1. Прочитать `AGENTS.md` каждого затрагиваемого репозитория и этот файл.
2. Прочитать `docs/status.md`, `docs/tasks.md` и документы, указанные в разделе
   задачи. Затем сверить их с кодом: часть roadmap может быть уже выполнена.
3. Не смешивать изменения серверной логики с UI/воспроизведением клиентов без
   явной необходимости. Один логический этап — одна ограниченная задача.
4. До изменения публичного HTTP-поведения обновить `api/openapi.yaml`, DTO и
   контрактные тесты. Действующие публичные пути в RockServer версионируются;
   текущий icon-roadmap планирует `/api/v1/stations/{id}/icon`.
5. Не читать, не печатать и не коммитить секреты. Не добавлять сеть в SQL migration,
   startup, readiness или request path, если это не оговорено отдельно.
6. Сохранять обратную совместимость. Если требуется несовместимое решение,
   остановиться, описать миграционный путь и запросить решение владельца.

## Репозитории и ответственность

| Проект | Путь | Технологии | Граница ответственности |
|---|---|---|---|
| RockServer | `C:\repos\rockserver` | Rust 2024, Axum, SQLx/PostgreSQL, Vite/Preact web | API, поиск, каталог, импорт, storage, auth и operations. |
| RockCast | `C:\repos\rockcast` | Rust 2024, egui/eframe | Windows UI, playback, Google Cast, локальный каталог и клиентский icon cache. |
| RockMobile | `C:\repos\rockmobile` | Kotlin, Compose, Media3, Room | Android UI/playback, offline catalogue и клиентский image cache. |

Параллельный, но отдельный authoring source baseline-каталога: `C:\repos\rockcast-station-catalog`.
Клиентские snapshot-ы обновляются только его проверяемым release workflow, не ручным копированием.

## Фактическое состояние на дату контекста

### RockServer

- Сервис уже реализует поиск/ранжирование, public catalog, голосовые команды и
  streaming voice, PostgreSQL persistence, Radio Browser и shared-catalog import,
  embeddings/backfill, passkey account/device pairing и browser account centre.
- Импорт Radio Browser — отдельный bounded one-shot binary. Он не является HTTP
  handler-ом, не запускается при startup и не выполняется в readiness. Existing
  `import_runs` фиксирует каталоговые import runs.
- Действующий OpenAPI **ещё не содержит** station-icon endpoint или `faviconUrl`
  в `StationResult`; текущая server persistence/API не публикует готовые иконки.
- `docs/roadmap/station-icons.md`, шаг 0, документально завершён; последующие
  шаги ещё являются планом. Существующий `favicon_url` в mobile export сейчас
  намеренно вставляется как `NULL`.
- `GET /admin` сейчас только local read-only preview. Persisted administrator
  principal, admin login/session и state-changing admin operations не реализованы.
- Current user accounts/passkey/browser sessions — это **не** будущая роль
  оператора admin и не должны переиспользоваться для неё без отдельного решения.

### RockCast

- Desktop client умеет local/Cast playback, station search/voice integration,
  local catalog fallback и безопасное pairing с RockServer.
- `Station` уже переносит `favicon_url`; есть `src/station_icons.rs` и bounded
  background cache. Пока server-owned icon API отсутствует, клиент использует
  explicit favicon URL либо conventional `/favicon.ico` homepage как переходный
  MVP fallback.
- После появления server icon API клиент должен предпочитать полученный
  RockServer URL. Прямые внешние загрузки нельзя считать постоянным контрактом.

### RockMobile

- Android client умеет server search/voice, Media3 playback, offline fallback и
  pairing/device sessions. Bundled baseline и extended SQLite catalog остаются
  офлайн-страховкой.
- Модель станции и DTO уже содержат `faviconUrl`; `StationIconLoader` временно
  получает explicit URL или `/favicon.ico` homepage и кеширует результат.
- После server-owned icon API mobile должен использовать только опубликованный
  URL, а при `null`/ошибке показывать локальный placeholder; playback не зависит
  от artwork.

## Архитектурные решения, которые нельзя незаметно менять

### Каталог и клиенты

```text
Radio Browser / shared catalog
        -> bounded RockServer importer -> PostgreSQL
        -> versioned API -> RockCast / RockMobile

Pinned client catalog -> local fallback when server is unavailable
```

- RockServer владеет серверным каталогом, поиском и импортом; клиенты владеют UI
  и воспроизведением.
- Importer, HTTP search и provider client разделены. HTTP не должен crawl-ить
  Radio Browser, обновлять embeddings, probe-ить streams или выполнять длительный
  импорт.
- Upsert import-а идемпотентен, использует source ownership и стабильные station/
  stream IDs. Не заменять IDs, не делать один долгий DB transaction и не выполнять
  сеть под DB lock.

### Station icons: утверждённый целевой контракт

```text
catalog source/homepage/bundle
        -> separate sync/backfill job
        -> metadata in PostgreSQL + WebP file storage
        -> GET /api/v1/stations/{id}/icon
        -> faviconUrl only for ready files -> client cache/placeholder
```

- RockServer — канонический origin готовой иконки. Внешний URL внутренний
  (`source_url`) и никогда не отдаётся клиенту как fallback.
- Иконка не хранится в PostgreSQL: там только metadata. Persistent filesystem
  storage configurable through `ROCKSERVER_STATION_ICON_DIR`; future S3/MinIO
  remains possible behind abstraction. **Единственный хранимый и выдаваемый
  формат v1 — WebP:** любой допустимый исходник нормализуется в WebP до
  публикации; оригинал не сохраняется и не выдаётся.
- До готового, проверенного файла `faviconUrl` равен `null`; icon endpoint
  возвращает `404` с `Cache-Control: no-store`. Placeholder рисует клиент. Это
  заменяет раннюю идею server-generated fallback в MVP.
- Endpoint читает только ready metadata/file; он не скачивает URL, не парсит
  homepage, не создаёт job и не держит DB lock во время сети.
- v1 accepts raster PNG/JPEG/WebP/ICO after MIME **and signature** validation;
  SVG excluded. Body <= 2 MiB; decoded image <= 1024x1024 and <= 1,048,576 px;
  output is transparent-preserving square WebP <=256x256.
- Source priority: valid explicit catalog icon URL -> favicon of valid homepage
  -> no source. A lower priority source must not displace a higher priority one.
- Downloader is an untrusted-network boundary: validate redirect and every
  resolved address (SSRF/DNS-rebinding/private/local/link-local/reserved blocking),
  bounded timeouts/redirects/retries, size/type/decode limits and sanitized logs.
- Writes are atomic; mark metadata `ready` only after file commit. Retain old ready
  file until a replacement has been verified. Metadata states: `pending`, `ready`,
  `missing`, `retryable_error`, `permanent_error`.
- Ready response: `200 image/webp`, content hash strong ETag, Last-Modified and
  `Cache-Control: public, max-age=86400, must-revalidate`; matching ETag -> `304`.

### Admin authentication: утверждённый план

The repository decision record is `docs/admin-security-plan.md`; it supersedes
the earlier chat suggestion of an HttpOnly admin cookie. The chosen first
delivery is a server-rendered Axum + HTMX console with a **short-lived opaque
Bearer access token held only in browser memory**, not `localStorage`.

- Passwords: Argon2id PHC hash only. Tokens/session identifiers: random opaque
  values; persist and compare only hashes. Never log them.
- Bootstrap one administrator through protected deploy environment or terminal;
  no public admin registration. For staging, the initial password is an ignored
  deployment secret/environment value, consumed only to create the missing admin;
  a restart must never overwrite a stored password.
- Admin principals/sessions are separate from RockCast machine-client credentials
  and from passkey user/device sessions. Client credentials are revocable, shown
  once and never grant console access.
- Admin login requires durable account+source-IP throttling, progressive delay,
  temporary lockout, generic failures and a security audit event.
- Admin Bearer sessions are short-lived, server-revocable and rotated on
  login/refresh. State-changing admin requests also validate `Origin`.
- UI: strict CSP, escaped HTML, no third-party scripts, `Cache-Control: no-store`,
  clear in-memory token on logout or `401`; no sensitive persistence by default.
- Future public/open API behaviour must declare security schemes and `401`/`403`/
  `429` responses. Existing account-cleanup operator is a separate staging-only
  root-scoped one-shot tool; it is not an admin console.

## Delivery rules for Codex

- Respect each repository's `AGENTS.md`. In RockServer all changed public Rust
  APIs/modules/types need meaningful Rustdoc; non-obvious private logic needs a
  concise explanatory comment.
- No real network in unit tests. Use deterministic fakes/local test servers for
  importer, image, SSRF and HTTP cases. Do not use real LLMs or external providers.
- **Каждый новый или изменённый запрос к PostgreSQL обязан иметь проверку на
  disposable PostgreSQL базе.** Запускать соответствующий integration test с
  `TEST_DATABASE_URL` против одноразовой, не staging/production БД. Unit tests,
  mocks и in-memory fakes дополняют, но не заменяют эту проверку; в отчёте явно
  указывать, была ли disposable-БД доступна и результат теста.
- For RockServer changes update `docs/status.md` and chronological `docs/tasks.md`;
  update `docs/service-diagrams.html` only when current or explicitly planned
  architecture changed.
- Required RockServer handoff checks: `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
  Also run focused web/API/OpenAPI checks where applicable.
- For RockMobile, Gradle commands must run serially from its root with the
  repository's process-local `JAVA_TOOL_OPTIONS` instruction. Never start any
  build/test/dependency install while another Codex-started build is running.
- **Никогда не запускать компиляцию проектов параллельно.** Перед `cargo`, Gradle,
  npm/pnpm, Docker image build или любой проверкой, которая компилирует код,
  дождаться и проверить результат предыдущей Codex-задачи такого типа.
- Before editing, inspect dirty state; preserve unrelated user changes. Do not
  deploy, mutate staging data, run an irreversible cleanup or expose a secret
  unless the user explicitly authorizes that action.
- **Все Codex-задачи по RockServer выполняются только в основном checkout
  `C:\repos\rockserver` на ветке `master`.** Отдельные Git worktree для этого
  репозитория не создавать и не использовать. Перед началом проверить
  `git status` и `git branch --show-current`; если активный каталог или ветка не
  совпадают с этим правилом, перейти в основной checkout на `master` до любых
  правок. Не менять ветку и не создавать commit без явного требования задачи.

## Ordered execution backlog

Statuses mean actual repository state, not an instruction to blindly implement.
Each task begins by re-validating its status and dependencies.

### RS-ADMIN

| ID | Status | Scope and acceptance boundary |
|---|---|---|
| RS-ADMIN-001 | Planned | Add durable admin principal, credential/session, login-attempt and security-event migrations plus domain/persistence interfaces and deterministic fakes. Separate from user/device and machine-client credentials; test every new/changed PostgreSQL query on a disposable DB via `TEST_DATABASE_URL`. |
| RS-ADMIN-002 | Planned | Bootstrap admin from protected terminal/deployment env; Argon2id hash, one-time creation, ignored `.env` on staging, no secret logs or password reset on restart. |
| RS-ADMIN-003 | Planned | Login/logout/refresh, opaque revocable Bearer sessions, middleware/API gate, role enforcement, durable throttling, generic failures and audit events. Add OpenAPI security/error cases. |
| RS-ADMIN-004 | Planned | Minimal protected Axum+HTMX console: login, in-memory bearer hand-off, stations/read models, strict CSP/no-store/Origin protection; no token persistence. |
| RS-ADMIN-005 | Complete (local, uncommitted) | Atomic administrator-session rotation, bounded request records with a 30-day retention decision, backup/incident runbook and security acceptance review. RockCast machine credentials are explicitly out of scope: current RockCast product routes are public and its user identity uses pairing. |

### RS-ICON

> **Статус направления:** отложено владельцем 2026-09-01. План ниже сохраняется
> как утверждённое будущее направление, но ни одну `RS-ICON-*` задачу нельзя
> начинать без нового явного запроса владельца. Текущие client-side favicon
> fallback-механизмы остаются переходным поведением.

| ID | Status | Scope and acceptance boundary |
|---|---|---|
| RS-ICON-001 | Complete (documentation) | Contract is recorded in `docs/roadmap/station-icons.md` step 0. Re-check it against OpenAPI/DTOs before code; it is not live behaviour. |
| RS-ICON-002 | Planned | Add `station_icons` metadata migration and repository boundary. Schema-only migration; preserve existing station/stream IDs and counts. |
| RS-ICON-003 | Planned | Extend shared/Radio Browser catalog import with normalized `favicon_source_url`; source changes schedule refresh but retain old ready icon. |
| RS-ICON-004 | Planned | Implement storage trait/filesystem backend with safe keys, path/symlink protection, atomic writes, replacement and concurrency tests. It stores ready artifacts exclusively as WebP. |
| RS-ICON-005 | Planned | Implement SSRF-safe downloader and normalizer with fake fetcher/storage/image boundaries, MIME/signature/decode limits, retry classification and backoff. Every accepted raster source is converted to square WebP <=256x256; source bytes are not retained. |
| RS-ICON-006 | Planned | Implement resumable bounded `sync_station_icons` CLI/backfill: missing/stale/station/limit/concurrency/dry-run/retry modes, short transactions and worker claim/lease semantics. It must not become a migration or startup job. |
| RS-ICON-007 | Planned | Add read-only WebP icon endpoint plus OpenAPI/router tests for ready `200 image/webp`, conditional `304`, missing `404`, headers and malformed IDs. |
| RS-ICON-008 | Planned | Add nullable RockServer `faviconUrl` to domain/persistence/search/voice/public DTOs only for available ready files; preserve ranking/order and reverse-proxy public-base correctness. |
| RS-ICON-009 | Planned | Build/import/verify versioned offline icon bundle with checksums, licensing/denylist/removal policy; binary assets stay out of Git. |
| RS-ICON-010 | Planned | Wire a bounded bundle-import + missing/retryable sync into staging deployment with backup, persistent volume, coverage/endpoint verification and partial-success semantics. No database recreation. |
| RS-ICON-011 | Planned; depends on RS-ADMIN-003/004 | Expose protected admin import/sync controls and durable job progress/history. A UI action starts a background bounded job and polls persisted state; it must never execute the full import inside one HTTP request. |
| RS-ICON-012 | Deferred | Manual per-station override/upload/refresh/remove. It must never be overwritten by automatic source sync; define upload validation, audit and deletion semantics first. |

Recommended sequencing after the owner reactivates the icon direction:
RS-ICON-002..010 proceed as bounded server/CLI work in listed order;
RS-ICON-011 follows the admin and icon foundations. Do not start RS-ICON-012
without an explicit product/security decision.

## Start prompt for a future Codex task

```text
Work on [TASK-ID] in the RockServer/RockCast/RockMobile project set.

First read C:\repos\rockserver\docs\codex-project-context.md and the relevant
AGENTS.md files. Then read RockServer docs/status.md, docs/tasks.md and the
task's linked roadmap/design document, and inspect the actual implementation.

Treat the code and current OpenAPI as the source of truth for what is already
implemented. Do not duplicate completed work. Keep the change limited to the
task, preserve unrelated working-tree changes, and do not deploy or mutate staging.

Do not run compilation, build, compiling test, or dependency-install commands
in parallel with another Codex-started command of that type in any project.
Wait for the earlier command to finish and verify its result first.

For every new or changed PostgreSQL query, add and run a relevant integration
test against a disposable database using TEST_DATABASE_URL. Unit tests, mocks
and in-memory fakes do not replace that check. State availability and result of
the disposable-database test in the final report.

Work only in the primary C:\repos\rockserver checkout on branch master. Do not
create or use a separate Git worktree for RockServer; if you are elsewhere,
switch to that checkout before editing. Inspect and preserve unrelated
working-tree changes before editing.

Implement the smallest correct slice, update the required documentation and
contract/tests, run the required checks, then report: decision made, files
changed, verification performed, known limitations, and the next task.
```

## Reference documents

- `C:\repos\rockserver\AGENTS.md`
- `C:\repos\rockserver\src\ARCHITECTURE.md` — compact source map for cross-module navigation.
- `C:\repos\rockserver\docs\status.md`
- `C:\repos\rockserver\docs\tasks.md`
- `C:\repos\rockserver\docs\architecture.md`
- `C:\repos\rockserver\docs\admin-security-plan.md`
- `C:\repos\rockserver\docs\roadmap\station-icons.md`
- `C:\repos\rockserver\api\openapi.yaml`
- `C:\repos\rockcast\AGENTS.md`, `C:\repos\rockcast\docs\README.md`
- `C:\repos\rockmobile\AGENTS.md`, `C:\repos\rockmobile\docs\README.md`
