# Поддержка иконок радиостанций

## Цель

RockServer должен стать основной точкой выдачи иконок. Клиенты получают `Station.faviconUrl` / `Station.favicon_url` как URL RockServer и не скачивают favicon напрямую у станций.

```text
Каталог / homepage станции / подготовленный bundle
                         |
                         v
          CLI sync/backfill + metadata + file storage
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
- Staging-БД обновляется in-place: migration → bundle import (если есть) → отдельный sync job.
- Первый deploy использует предварительно подготовленный bundle; deploy-side sync дозаполняет missing и retryable записи.

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

Этот раздел фиксирует решение для следующих шагов roadmap. Он **не** добавляет маршрут,
поле DTO или OpenAPI-операцию: в текущем `api/openapi.yaml` нет ни
`GET /api/v1/stations/{id}/icon`, ни `faviconUrl`. До реализации клиенты не должны
считать этот контракт доступным.

| Область | Планируемое решение v1 |
| --- | --- |
| Публичный URL | Канонический URL готовой иконки — `GET /api/v1/stations/{id}/icon`. `id` — стабильный идентификатор RockServer; публичный base URL берётся из deployment-конфигурации, а не из внутреннего bind address. |
| `faviconUrl` | Будущее nullable-поле результатов станции содержит только абсолютный URL RockServer к этому endpoint. Оно равно `null`, если для станции нет metadata со статусом `ready` **или** готовый файл недоступен/не прошёл проверку. Клиент при `null` показывает свой placeholder; RockServer не возвращает внешний URL как fallback. |
| Внешний источник | Внутренний `source_url` хранится отдельно от публичного DTO и никогда не раскрывается endpoint-ом. Приоритет выбора: (1) явный валидный HTTP(S) URL иконки из каталога, (2) favicon, извлечённый из валидной homepage станции, (3) источник отсутствует. Источник с более низким приоритетом не заменяет доступный источник с более высоким приоритетом без явного решения sync-службы. |
| Сеть и данные | SQL migration меняет только схему и не выполняет сетевой I/O. HTTP endpoint читает только готовые metadata и storage; он не скачивает source URL, не парсит homepage, не запускает sync и не удерживает DB lock во время сети. Загрузка и нормализация выполняются отдельным контролируемым sync/backfill job. |
| Формат и лимиты | v1 принимает только raster-источники после проверки сигнатуры и MIME: PNG, JPEG, WebP или ICO. SVG не принимается в v1. До декодирования лимит тела — 2 MiB; после декодирования — не более 1 024×1 024 px и 1 048 576 пикселей. Готовый артефакт — квадратный WebP, не более 256×256 px; прозрачные области сохраняются. Любой лимит, тип или декодирование, не прошедшие проверку, не публикуют новый файл. |
| Ответы | При готовом проверенном файле endpoint отвечает `200` с `Content-Type: image/webp`, `Content-Length`, strong `ETag` из content hash и `Last-Modified`. Совпавший `If-None-Match` возвращает `304` без тела с теми же cache validators. Неизвестная station, неготовая/отсутствующая иконка либо несоответствие metadata и файла возвращает `404`; source/storage details не раскрываются. |
| Кеширование | `200` и `304` используют `Cache-Control: public, max-age=86400, must-revalidate`. Это ограничивает устаревание по стабильному station URL, который может получить заменённый файл. `404` использует `Cache-Control: no-store`, чтобы отсутствие не кешировалось, пока фоновый job может подготовить иконку. |
| Rollout и rollback | Rollout: совместимая migration без сети → persistent storage → bundle import/verify → отдельный bounded sync → endpoint → публикация `faviconUrl` только для `ready` файлов. На каждом этапе частичный успех допустим: отсутствующие иконки остаются `null`/`404`. Rollback приложения не удаляет metadata или файлы и не требует rollback migration; отключённая выдача снова даёт `faviconUrl: null` для новых API-ответов, а клиентский placeholder остаётся рабочим. Удаление файлов/metadata — отдельная явно подтверждённая операция, не rollback. |

Security and operational boundaries for the later downloader remain deliberately separate from this
HTTP contract: it must validate every redirect and resolved address against SSRF policy, use bounded
timeouts and retries, and retain an older ready artifact until a replacement is verified. These are
requirements for steps 3–5, not behavior of migrations or request handling.

## Шаг 1. Добавить metadata schema

**Цель:** подготовить заполненную staging-БД без recreate.

**Изменения:** добавить новую SQLx migration, не меняя уже применённые. Создать `station_icons` с `station_id` как PK/FK на `stations(id) ON DELETE CASCADE`; полями `source_url`, `storage_key`, `content_type`, `byte_size`, dimensions, `content_hash`, `source_etag`, `source_last_modified`, `status`, attempts/retry timestamps, `last_error_code`, audit timestamps. Ограничить статусы `pending`, `ready`, `missing`, `retryable_error`, `permanent_error`; добавить индексы для missing/due retries. Не создавать и не загружать файлы в migration.

**Acceptance criteria:** migration применяется поверх непустого каталога без изменения station/stream IDs и counts; отсутствие строки metadata означает отсутствие иконки; startup не вызывает сеть.

**Тесты/проверки:** применить старые migrations и fixture import, затем новую migration; сравнить IDs/counts; `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

**Зависимости:** шаг 0.

**Рекомендуемая модель:** **Terra high** — production schema и upgrade path заполненной БД.

## Шаг 2. Расширить catalog import источником иконки

**Цель:** отделить внешний URL от публичного `faviconUrl`.

**Изменения:** добавить nullable `favicon_source_url` в `ImportedStation` или отдельную metadata-команду; расширить Radio Browser DTO upstream favicon; применить нормализацию только HTTP(S); обновить PostgreSQL upsert. При смене URL помечать запись на refresh, но сохранять старую готовую иконку до успешной замены.

**Acceptance criteria:** валидный URL сохраняется, плохой/пустой не ломает import; source URL change не создаёт окно без иконки.

**Тесты/проверки:** DTO normalization, upsert, повторный import с неизменным/пустым/сменившимся URL.

**Зависимости:** шаг 1.

**Рекомендуемая модель:** **Terra medium** — существующий import pipeline уже имеет подходящие границы.

## Шаг 3. Реализовать cache/storage abstraction

**Цель:** отделить файлы от DB, HTTP и будущего S3/MinIO.

**Изменения:** ввести trait для `get`, `exists`, `put_atomic` и безопасного удаления; filesystem backend с persistent root из `ROCKSERVER_STATION_ICON_DIR`. Storage key строится только из валидированного station ID/content hash; запретить path traversal и symlink escape. Писать временный файл в том же filesystem и атомарно переименовывать; `ready` ставить только после commit файла. Старый файл удалять только после успешной замены.

**Acceptance criteria:** падение/частичная запись не создаёт `ready` metadata без файла; concurrent writer не публикует частичный файл.

**Тесты/проверки:** temp-dir tests для atomic write, invalid key, missing file, replacement и concurrency.

**Зависимости:** шаг 1.

**Рекомендуемая модель:** **Terra high** — атомарность, конкурентность и path-security.

## Шаг 4. Реализовать безопасный downloader и normalisation service

**Цель:** безопасно превратить внешнюю favicon в готовый артефакт.

**Изменения:** создать `StationIconService` с тестируемыми fetcher/repository/storage/image-processor boundaries. Ограничить connect/request timeout, redirects, body bytes, decoded dimensions/pixels; проверять MIME и file signature. Разрешить HTTP(S) только с SSRF-защитой: localhost, private/link-local/reserved IP, DNS rebinding и каждый redirect. Нормализовать raster formats; явно определить SVG/ICO. Добавить ETag/Last-Modified, content hash, retry classification, exponential backoff+jitter и безопасные логи.

**Acceptance criteria:** internal/invalid/oversized URL не становится `ready`; временная ошибка сохраняет старый файл и планирует retry; sync идемпотентен.

**Тесты/проверки:** deterministic fake fetcher/server для redirect, SSRF, timeout, oversize, MIME mismatch, malformed/decompression-bomb image, `304`, retry; unit tests не используют внешний интернет.

**Зависимости:** шаги 1 и 3.

**Рекомендуемая модель:** **Terra xhigh**, иначе **Terra high** — недоверенный контент и SSRF.

## Шаг 5. Добавить CLI sync/backfill

**Цель:** обновить существующий staging-каталог отдельно от migrations.

**Изменения:** добавить binary в стиле `src/bin/*`, например `sync_station_icons`, с `--missing`, `--refresh-stale`, `--station-id`, `--limit`, `--concurrency`, `--dry-run`, `--retry-errors`. Использовать bounded concurrency, небольшие страницы и короткие DB transactions: сеть не выполняется под DB lock. Для нескольких workers применить lease/claim или `FOR UPDATE SKIP LOCKED`. Итог: selected/succeeded/skipped/retryable/permanent failures и документированный exit status.

**Acceptance criteria:** job безопасно остановить и resume-ить; один upstream не останавливает batch; dry-run не меняет DB/storage; ready/fresh entries пропускаются.

**Тесты/проверки:** непустая DB + fake fetcher/storage: resume after failure, parallel workers, dry-run, concurrency cap.

**Зависимости:** шаги 2–4.

**Рекомендуемая модель:** **Terra high** — orchestration и concurrent resume.

## Шаг 6. Добавить HTTP endpoint

**Цель:** дать клиентам стабильный кешируемый URL.

**Изменения:** добавить `GET /api/v1/stations/{id}/icon` в Axum router и OpenAPI. Endpoint читает только `ready` metadata/storage; отдаёт `Content-Type`, `Content-Length`, strong ETag из hash, `Last-Modified`, `Cache-Control`, поддерживает `If-None-Match`/`304`. Не раскрывает storage/source URL и не запускает sync. Unknown/missing icon — `404`; metadata/file mismatch безопасно фиксируется telemetry.

**Acceptance criteria:** ready → `200`, conditional request → `304` без body, missing не вызывает сеть, path traversal невозможен.

**Тесты/проверки:** router tests для `200/304/404`, headers, unknown/malformed ID, missing file; OpenAPI validation.

**Зависимости:** шаги 1, 3, 4.

**Рекомендуемая модель:** **Terra medium** — read-only endpoint над подготовленными данными.

## Шаг 7. Вернуть `faviconUrl` в Station/API

**Цель:** публиковать URL только когда файл действительно доступен.

**Изменения:** добавить `favicon_url: Option<String>` в domain/persistence DTO и публичное поле согласованного casing; сделать search SQL `LEFT JOIN` только к `status='ready'`. URL строить из public base URL, а не внутреннего bind address. Обновить search/voice responses, примеры и `StationResult` в OpenAPI. При отсутствии ready file возвращать `null`; не менять ranking/order.

**Acceptance criteria:** ready station получает RockServer URL, остальные `null`; URL корректен за reverse proxy; старые клиенты совместимы.

**Тесты/проверки:** DTO/SQL integration, contract snapshots, base URL/reverse-proxy cases, ranking regression.

**Зависимости:** шаг 6.

**Рекомендуемая модель:** **Terra medium** — последовательное расширение SQL/domain/DTO.

## Шаг 8. Подготовить offline bundle

**Цель:** обеспечить высокий initial coverage без зависимости deploy от сайтов станций.

**Изменения:** создать workflow тем же service/CLI, формирующий bundle: нормализованные файлы и manifest с `station_id`, storage key, hash, MIME, dimensions, source fingerprint, timestamp. Добавить import/verify: проверка hash, atomic placement и idempotent metadata upsert. Не коммитить binary assets; хранить bundle как versioned deployment artifact с checksum/retention. Зафиксировать licensing, denylist и removal procedure.

**Acceptance criteria:** corrupt bundle отклоняется до публикации; повторный import не создаёт дубликаты и не ухудшает свежие данные; bundle не содержит секретов.

**Тесты/проверки:** export/import round-trip, tampered manifest/file, duplicate/partial import recovery, coverage report.

**Зависимости:** шаг 5.

**Рекомендуемая модель:** **Terra high** — целостность artifact и licensing.

## Шаг 9. Встроить flow в staging deployment

**Цель:** обновить заполненный staging без recreate и дозаполнять иконки.

**Изменения:** обновить deploy runbook/CI: backup + health check → compatible server/schema → persistent icon volume → bundle import → one-shot `sync_station_icons --missing --retry-errors` → coverage/endpoint verification → включение клиента. Sync — отдельный bounded job, не migration и не blocking server startup. Добавить deploy lease, time budget, partial-success semantics, capacity/permission/backup alerts. Rollback приложения не удаляет table/files; stale refresh — отдельное расписание.

**Acceptance criteria:** rehearsal на копии staging DB проходит без drop/recreate; service доступен при частичных upstream errors; повторные deploy/job только дозаполняют missing.

**Тесты/проверки:** backup/restore point, interrupt/resume, repeat deploy, unavailable upstream, full/read-only volume, endpoint smoke checks.

**Зависимости:** шаги 5–8.

**Рекомендуемая модель:** **Terra xhigh**, иначе **Terra high** — production rollout и частичные отказы.

## Шаг 10. Наблюдаемость и финальная проверка

**Цель:** поддерживать функцию после первого backfill и безопасно включить клиентов.

**Изменения:** metrics/logging: coverage, sync outcome/latency/bytes/retries, stale count, endpoint hit/miss/304, metadata-file mismatch. Добавить `verify`/`repair-missing`; orphan cleanup сначала report/dry-run, удаление — отдельным явным режимом с grace period. После фактической реализации обновить `docs/status.md`, `docs/tasks.md` и diagrams. Добавить E2E: существующая station → source → sync/bundle → storage → `faviconUrl` → `GET`/`304`; проверить ready/missing/retryable/deleted/source-changed.

**Acceptance criteria:** оператор видит coverage и причины ошибок; verify находит missing/corrupt/orphaned data; cleanup по умолчанию ничего не удаляет; новый клиент показывает иконку либо placeholder, старый не ломается.

**Тесты/проверки:** metric/log assertions, verify/repair integration, dry-run cleanup, полный `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, OpenAPI validation, staging smoke/E2E.

**Зависимости:** шаг 9.

**Рекомендуемая модель:** **Terra medium** для реализации, **Terra high** для final operational review.

## Последовательность первого staging rollout

1. Сделать backup и зафиксировать station counts/IDs.
2. Развернуть совместимую версию и применить schema migration.
3. Проверить существующие API: `faviconUrl` может быть `null`.
4. Подключить persistent storage и проверить права/свободное место.
5. Импортировать проверенный bundle.
6. Запустить bounded `sync_station_icons --missing --retry-errors`.
7. Проверить coverage, `200/304/404` и отсутствие изменения station counts/IDs.
8. Оставить retryable failures следующему job, не откатывая успешный deploy.
9. После наблюдения включить отображение в клиентах с placeholder при `null`/`404`.
