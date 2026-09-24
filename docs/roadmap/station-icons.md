# Поддержка иконок радиостанций

## Цель

RockServer должен стать основной точкой выдачи иконок. Клиенты получают `Station.faviconUrl` / `Station.favicon_url` как URL RockServer и не скачивают favicon напрямую у станций.

```text
Каталог / homepage станции / ручная загрузка администратора
                         |
                         v
          protected admin import job + metadata + file storage
                         |
                         v
          GET /api/v1/stations/{id}/icon -> клиент
```

Основные решения:

- Публичный `faviconUrl` — URL RockServer, а не внешний адрес.
- Внешний URL хранится только как `source_url`; файлы хранятся вне PostgreSQL.
- `faviconUrl` равен `null`, пока готового файла нет; placeholder отображает клиент.
- SQL migration только меняет схему: в ней запрещены сетевые скачивания.
- HTTP endpoint только выдаёт готовый cache: никакой загрузки по запросу пользователя.
- Импорт запускается только явной кнопкой авторизованного администратора в `https://rockplatform.win/admin`; CLI, startup- и deploy-side sync не используются.
- Кнопка создаёт отдельную durable background job; HTTP-запрос не ждёт скачивания, а админка показывает её сохранённый прогресс.
- Админка показывает готовые иконки только через URL RockServer и локальный placeholder; она не запрашивает внешние favicon напрямую.

## Выбор модели Codex

Обычная изолированная реализация — **Terra medium**. Для изменений схемы, обработки недоверенных URL/изображений и production rollout — **Terra high**. Если доступна, **Terra xhigh** применять для SSRF/threat-model review и финального deployment review: в этих задачах цена архитектурной ошибки выше, чем выигрыш от скорости.

Каждый шаг ниже является отдельной задачей для Codex.

## Шаг 0. Зафиксировать контракт

**Цель:** принять совместимые решения до кода.

**Изменения:** описать канонический `GET /api/v1/stations/{id}/icon`; семантику `faviconUrl`; source priority: явный URL каталога → favicon homepage → отсутствует; формат v1 (рекомендуется WebP, квадрат до 256×256); допустимые размеры, `200`/`304`/`404`, cache headers и rollback. Placeholder остаётся на клиенте.

**Acceptance criteria:** документация однозначно описывает, когда поле `null`, и исключает network I/O из DB migration и request path.

**Тесты/проверки:** review против `AGENTS.md`, `api/openapi.yaml`, существующих DTO и router.

**Зависимости:** нет.

**Рекомендуемая модель:** **Terra high** или **Terra xhigh** — публичный контракт, безопасность и эксплуатация.

### Планируемый контракт v1 (решение шага 0; ещё не фактическое поведение)

Этот раздел фиксирует реализованный контракт v1 и remaining operational work.
`api/openapi.yaml` объявляет `GET /api/v1/stations/{id}/icon` и nullable
`favicon_url`; только готовая server-owned metadata публикует path к этому endpoint.

| Область | Планируемое решение v1 |
| --- | --- |
| Публичный URL | Канонический URL готовой иконки — same-origin path `GET /api/v1/stations/{id}/icon`. `id` — стабильный идентификатор RockServer; клиент разрешает path относительно уже выбранного API origin, поэтому внутренний bind address никогда не раскрывается. |
| `faviconUrl` | Nullable-поле результатов станции содержит только этот server-owned same-origin path. Оно равно `null`, если для станции нет metadata со статусом `ready`. Клиент при `null` показывает свой placeholder; RockServer не возвращает внешний URL как fallback. |
| Внешний источник | Внутренний `source_url` хранится отдельно от публичного DTO и никогда не раскрывается endpoint-ом. Приоритет выбора: (1) явный валидный HTTP(S) URL иконки из каталога, (2) favicon, извлечённый из валидной homepage станции, (3) источник отсутствует. Источник с более низким приоритетом не заменяет доступный источник с более высоким приоритетом без явного решения sync-службы. |
| Сеть и данные | SQL migration меняет только схему и не выполняет сетевой I/O. HTTP endpoint читает только готовые metadata и storage; он не скачивает source URL, не парсит homepage, не запускает sync и не удерживает DB lock во время сети. Загрузка и нормализация выполняются отдельным контролируемым sync/backfill job. |
| Формат и лимиты | v1 принимает только raster-источники после проверки сигнатуры и MIME: PNG, JPEG, WebP или ICO. SVG не принимается в v1. До декодирования лимит тела — 2 MiB; после декодирования — не более 1 024×1 024 px и 1 048 576 пикселей. Готовый артефакт — квадратный WebP, не более 256×256 px; прозрачные области сохраняются. Любой лимит, тип или декодирование, не прошедшие проверку, не публикуют новый файл. |
| Ответы | При готовом проверенном файле endpoint отвечает `200` с `Content-Type: image/webp`, `Content-Length`, strong `ETag` из content hash и `Last-Modified`. Совпавший `If-None-Match` возвращает `304` без тела с теми же cache validators. Неизвестная station, неготовая/отсутствующая иконка либо несоответствие metadata и файла возвращает `404`; source/storage details не раскрываются. |
| Кеширование | `200` и `304` используют `Cache-Control: public, max-age=86400, must-revalidate`. Это ограничивает устаревание по стабильному station URL, который может получить заменённый файл. `404` использует `Cache-Control: no-store`, чтобы отсутствие не кешировалось, пока фоновый job может подготовить иконку. |
| Rollout и rollback | Rollout: совместимая migration без сети → persistent storage → endpoint → публикация `faviconUrl` только для `ready` файлов → явный import из админки. На каждом этапе частичный успех допустим: отсутствующие иконки остаются `null`/`404`. Rollback приложения не удаляет metadata или файлы и не требует rollback migration; отключённая выдача снова даёт `faviconUrl: null` для новых API-ответов, а клиентский placeholder остаётся рабочим. Удаление файлов/metadata — отдельная явно подтверждённая операция, не rollback. |

Security and operational boundaries for the later downloader remain deliberately separate from this
HTTP contract: it must validate every redirect and resolved address against SSRF policy, use bounded
timeouts and retries, and retain an older ready artifact until a replacement is verified. These are
requirements for steps 3–5, not behavior of migrations or request handling.

## Шаг 1. Добавить metadata schema

**Статус:** реализовано локально 2026-09-22; финальная проверка всего изменения пройдена 2026-09-22 (закрыта в RS-ICON-010).

**Цель:** подготовить заполненную staging-БД без recreate.

**Изменения:** добавить новую SQLx migration, не меняя уже применённые. Создать `station_icons` с `station_id` как PK/FK на `stations(id) ON DELETE CASCADE`; полями `source_url`, `storage_key`, `content_type`, `byte_size`, dimensions, `content_hash`, `source_etag`, `source_last_modified`, `status`, attempts/retry timestamps, `last_error_code`, audit timestamps. Ограничить статусы `pending`, `ready`, `missing`, `retryable_error`, `permanent_error`; добавить индексы для missing/due retries. Не создавать и не загружать файлы в migration.

**Acceptance criteria:** migration применяется поверх непустого каталога без изменения station/stream IDs и counts; отсутствие строки metadata означает отсутствие иконки; startup не вызывает сеть.

**Тесты/проверки:** применить старые migrations и fixture import, затем новую migration; сравнить IDs/counts; `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

**Зависимости:** шаг 0.

**Рекомендуемая модель:** **Terra high** — production schema и upgrade path заполненной БД.

## Шаг 2. Расширить catalog import источником иконки

**Статус:** реализовано локально 2026-09-22; финальная проверка всего изменения пройдена 2026-09-22 (закрыта в RS-ICON-010).

**Цель:** отделить внешний URL от публичного `faviconUrl`.

**Изменения:** добавить nullable `favicon_source_url` в `ImportedStation` или отдельную metadata-команду; расширить Radio Browser DTO upstream favicon; применить нормализацию только HTTP(S); обновить PostgreSQL upsert. При смене URL помечать запись на refresh, но сохранять старую готовую иконку до успешной замены.

**Acceptance criteria:** валидный URL сохраняется, плохой/пустой не ломает import; source URL change не создаёт окно без иконки.

**Тесты/проверки:** DTO normalization, upsert, повторный import с неизменным/пустым/сменившимся URL.

**Зависимости:** шаг 1.

**Рекомендуемая модель:** **Terra medium** — существующий import pipeline уже имеет подходящие границы.

## Шаг 3. Реализовать cache/storage abstraction

**Статус:** реализовано локально 2026-09-22; финальная проверка всего изменения пройдена 2026-09-22 (закрыта в RS-ICON-010).

**Цель:** отделить файлы от DB, HTTP и будущего S3/MinIO.

**Изменения:** ввести trait для `get`, `exists`, `put_atomic` и безопасного удаления; filesystem backend с persistent root из `ROCKSERVER_STATION_ICON_DIR`. Storage key строится только из валидированного station ID/content hash; запретить path traversal и symlink escape. Писать временный файл в том же filesystem и атомарно переименовывать; `ready` ставить только после commit файла. Старый файл удалять только после успешной замены.

**Acceptance criteria:** падение/частичная запись не создаёт `ready` metadata без файла; concurrent writer не публикует частичный файл.

**Тесты/проверки:** temp-dir tests для atomic write, invalid key, missing file, replacement и concurrency.

**Зависимости:** шаг 1.

**Рекомендуемая модель:** **Terra high** — атомарность, конкурентность и path-security.

## Шаг 4. Реализовать безопасный downloader и normalisation service

**Статус:** реализовано локально 2026-09-22; финальная проверка всего изменения пройдена 2026-09-22 (закрыта в RS-ICON-010). Приоритет-2 источника — favicon с homepage станции (bounded SSRF-проверенный fetch, `<link rel="icon">`/`apple-touch-icon`, fallback `/favicon.ico`, `source_priority = 1`) — добавлен 2026-09-22 в RS-ICON-011 после того, как первый production-импорт показал 16825 missing из-за отсутствия явных URL в pinned-каталоге.

**Цель:** безопасно превратить внешнюю favicon в готовый артефакт.

**Изменения:** создать `StationIconService` с тестируемыми fetcher/repository/storage/image-processor boundaries. Ограничить connect/request timeout, redirects, body bytes, decoded dimensions/pixels; проверять MIME и file signature. Разрешить HTTP(S) только с SSRF-защитой: localhost, private/link-local/reserved IP, DNS rebinding и каждый redirect. Нормализовать raster formats; явно определить SVG/ICO. Добавить ETag/Last-Modified, content hash, retry classification, exponential backoff+jitter и безопасные логи.

**Acceptance criteria:** internal/invalid/oversized URL не становится `ready`; временная ошибка сохраняет старый файл и планирует retry; sync идемпотентен.

**Тесты/проверки:** deterministic fake fetcher/server для redirect, SSRF, timeout, oversize, MIME mismatch, malformed/decompression-bomb image, `304`, retry; unit tests не используют внешний интернет.

**Зависимости:** шаги 1 и 3.

**Рекомендуемая модель:** **Terra xhigh**, иначе **Terra high** — недоверенный контент и SSRF.

## Шаг 5. Добавить durable import job, запускаемую из админки

**Статус:** реализовано локально 2026-09-22; protected HTTP launch and UI are implemented locally in step 8.

**Цель:** позволить оператору в `https://rockplatform.win/admin` начать bounded импорт недостающих и retryable иконок без CLI, миграционного, startup- или deploy-side запуска.

**Изменения:** добавить metadata для import job и её сохранённых счётчиков: `selected`, `processed`, `ready`, `missing`, `retryable_error`, `permanent_error`, `skipped`, `started_at`, `finished_at`, безопасный status и последний безопасный error code. `POST` администратора создаёт только одну активную job и сразу отвечает её идентификатором; background worker выполняет bounded concurrency и короткие DB transactions, никогда не держит DB lock во время сети. `GET` администратора читает status и прогресс для polling. После restart незавершённая job становится `interrupted`; оператор явно запускает новую/resume job кнопкой, а сервис не начинает сеть сам. Нет binary `sync_station_icons`, cron или скрытого deploy hook.

**Acceptance criteria:** один администраторский запрос быстро возвращает job ID; одновременно активна не более одна job; один upstream не останавливает batch; retryable записи можно явно повторить; прогресс переживает reload админки и не раскрывает source URL, stream URL или секреты.

**Тесты/проверки:** непустая DB + fake fetcher/storage: polling progress, single-active-job guard, restart interruption, resume, concurrency cap, partial failure и отсутствие network I/O до явного admin start. Каждый новый PostgreSQL query проверить на disposable DB.

**Зависимости:** шаги 2–4 и существующая admin Bearer/Origin boundary.

**Рекомендуемая модель:** **Terra high** — durable orchestration, authorization и resume semantics.

## Шаг 6. Добавить HTTP endpoint

**Статус:** реализовано локально 2026-09-22; `GET /api/v1/stations/{station_id}/icon` выдаёт только ready WebP, с ETag/304 и non-cacheable 404.

**Цель:** дать клиентам стабильный кешируемый URL.

**Изменения:** добавить `GET /api/v1/stations/{id}/icon` в Axum router и OpenAPI. Endpoint читает только `ready` metadata/storage; отдаёт `Content-Type`, `Content-Length`, strong ETag из hash, `Last-Modified`, `Cache-Control`, поддерживает `If-None-Match`/`304`. Не раскрывает storage/source URL и не запускает sync. Unknown/missing icon — `404`; metadata/file mismatch безопасно фиксируется telemetry.

**Acceptance criteria:** ready → `200`, conditional request → `304` без body, missing не вызывает сеть, path traversal невозможен.

**Тесты/проверки:** router tests для `200/304/404`, headers, unknown/malformed ID, missing file; OpenAPI validation.

**Зависимости:** шаги 1, 3, 4.

**Рекомендуемая модель:** **Terra medium** — read-only endpoint над подготовленными данными.

## Шаг 7. Вернуть `faviconUrl` в Station/API

**Статус:** реализовано локально 2026-09-22: PostgreSQL left-join выдаёт nullable same-origin `favicon_url` только для ready metadata; offline catalog остаётся `null`.

**Цель:** публиковать URL только когда файл действительно доступен.

**Изменения:** добавить `favicon_url: Option<String>` в domain/persistence DTO и публичное поле согласованного casing; сделать search SQL `LEFT JOIN` только к `status='ready'`. URL строится как same-origin path, а не из внутреннего bind address. Обновить search/voice responses, примеры и `StationResult` в OpenAPI. При отсутствии ready file возвращать `null`; не менять ranking/order.

**Acceptance criteria:** ready station получает RockServer same-origin path, остальные `null`; path корректен за reverse proxy; старые клиенты совместимы.

**Тесты/проверки:** DTO/SQL integration, contract snapshots, base URL/reverse-proxy cases, ranking regression.

**Зависимости:** шаг 6.

**Рекомендуемая модель:** **Terra medium** — последовательное расширение SQL/domain/DTO.

## Шаг 8. Добавить управление импортом и иконки в админку

**Статус:** реализовано локально 2026-09-22 для automatic import: кнопка запуска, durable polling/progress и thumbnail/placeholder готовы. `favicon_url` в catalog DTO и ручной override остаются следующими отдельными этапами.

**Цель:** администратор на `https://rockplatform.win/admin` видит покрытие, запускает импорт кнопкой и видит готовые иконки в списке станций.

**Изменения:** добавить в текущую вкладку «Станции» кнопку «Импортировать иконки», disabled при active job, и progress panel, который poll-ит только защищённый same-origin job read model. Панель показывает безопасный status, счётчики и время начала/окончания; она не показывает source URL или stream URL. Admin station DTO получает стабильный `station_id` только для защищённых действий и nullable `favicon_url` только для ready-файла. Рендерить thumbnail с `src=favicon_url`; при `null`, `404` или ошибке декодирования — локальный placeholder. CSP сохраняет `img-src 'self'`, поэтому админка никогда не обращается к origin станции. Операции должны быть описаны в OpenAPI, требовать текущий AdminBearer и, для POST, existing Origin/proxy protection.

**Acceptance criteria:** кнопка возвращает UI в usable state сразу после создания job; reload продолжает показывать сохранённый прогресс; повторный click не создаёт вторую job; готовая иконка отображается из RockServer, missing — placeholder; неаутентифицированный caller не видит progress и не может запустить import.

**Тесты/проверки:** HTTP/contract tests для authorization, Origin rejection, create/status/single-active job; deterministic UI regression для start/poll/complete/error и thumbnail/placeholder; OpenAPI validation.

**Зависимости:** шаги 5–7.

**Рекомендуемая модель:** **Terra high** — state-changing admin API и безопасная UI state machine.

## Шаг 9. Добавить ручную загрузку и override в админке

**Статус:** реализовано локально 2026-09-22: raw bounded upload и подтверждённое removal используют общий WebP normalizer; automatic source сохраняется, а content-addressed artifact не удаляется inline.

**Цель:** администратор может вручную загрузить или заменить иконку конкретной станции, если автоматический источник отсутствует или некачественный.

**Изменения:** защищённый raw-body upload для выбранного `station_id` принимает только тот же bounded raster-набор (PNG/JPEG/WebP/ICO), проверяет signature/размеры, нормализует в WebP и атомарно публикует файл через шаги 3–4. Ручной `override` имеет приоритет над automatic source sync и не перезаписывается import job. Для replace/remove определить явное подтверждение, audit action/outcome без byte/source details и семантику: delete возвращает `faviconUrl: null`/`404`, не удаляет station и не запускает автоматический импорт. Админская строка показывает upload/replace/remove только после появления безопасной stable ID в шаге 8.

**Acceptance criteria:** bad/oversized/decompression-bomb input не публикует новый файл; partial upload не оставляет ready metadata; successful override сразу виден через server icon URL; automatic job его пропускает; remove требует confirmation и сохраняет audit trail без содержимого файла.

**Тесты/проверки:** deterministic upload tests для type/signature/size/decode/atomic replacement, authorization/Origin/audit tests, UI tests для upload, error, replace и confirmed removal; disposable PostgreSQL coverage для новых queries.

**Зависимости:** шаги 3, 4, 7 и 8.

**Рекомендуемая модель:** **Terra high** — upload boundary, atomicity и audit semantics.

## Шаг 10. Deployment, наблюдаемость и финальная проверка

**Статус:** deployment-часть реализована и проверена локально 2026-09-22 (RS-ICON-010: persistent volume, env wiring, deploy regression, dry-run); production deploy коммита `179c9a8` выполнен 2026-09-22 (`readiness=passed`). Метрики, admin-диагностика и подтверждённый orphan cleanup остаются будущими отдельными этапами. Первый импорт иконок — явное действие оператора в админке, не часть deployment.

**Цель:** поддерживать функцию после первого admin-started import и безопасно включить клиентов.

**Изменения:** deploy добавляет только совместимую migration, persistent icon volume и endpoint smoke check; он не создаёт import job и не выполняет сеть. Metrics/logging: coverage, job outcome/latency/bytes/retries, stale count, endpoint hit/miss/304, metadata-file mismatch. Админка показывает coverage, terminal job result и read-only диагностику missing/corrupt metadata/files. Удаление orphan-файлов — отдельная явно подтверждённая операция с grace period, не CLI. После фактической реализации обновить `docs/status.md`, `docs/tasks.md` и diagrams. Добавить E2E: существующая station → admin-created job или manual upload → storage → `faviconUrl` → admin thumbnail → `GET`/`304`; проверить ready/missing/retryable/deleted/source-changed.

**Acceptance criteria:** оператор видит coverage, сохранённый прогресс и причины ошибок; deployment не запускает сеть; read-only admin diagnostics находит missing/corrupt/orphaned data; cleanup по умолчанию ничего не удаляет; новый клиент и админка показывают иконку либо placeholder, старый клиент не ломается.

**Тесты/проверки:** metric/log assertions, admin diagnostics integration, confirmed-cleanup tests, полный `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, OpenAPI validation, staging smoke/E2E.

**Зависимости:** шаги 8–9.

**Рекомендуемая модель:** **Terra medium** для реализации, **Terra high** для final operational review.

## Последовательность первого production rollout

1. Сделать backup и зафиксировать station counts/IDs.
2. Развернуть совместимую версию и применить schema migration.
3. Проверить существующие API: `faviconUrl` может быть `null`.
4. Подключить persistent storage и проверить права/свободное место.
5. Проверить admin login, Origin/proxy boundary, storage rights и свободное место.
6. Оператор запускает import из вкладки «Станции» на `https://rockplatform.win/admin` и наблюдает прогресс; deploy не выполняет import.
7. Проверить coverage, `200/304/404`, admin thumbnail/placeholder и отсутствие изменения station counts/IDs.
8. Оставить retryable failures следующей явно запущенной job, не откатывая успешный deploy.
9. После наблюдения включить отображение в клиентах с placeholder при `null`/`404`.
