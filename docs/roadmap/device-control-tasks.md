# Пошаговые задачи: Rockmobile, RockCast, ESP32 и Home Assistant

## Как использовать список

Каждый пункт ниже — отдельная задача для Codex с ограниченным scope и проверяемым результатом. Задачи выполняются по порядку зависимостей; следующий пункт не должен реализовывать ещё не утверждённый контракт предыдущего. Изменения RockServer, RockCast, Rockmobile и ESP32 остаются в своих репозиториях.

Общие правила для всех задач:

- существующие account/device pairing, `devices.id`, durable device secret, `/api/v1/auth/device-session`, `/api/v1/devices` и revoke являются готовой основой и не реализуются повторно;
- `DeviceCapabilities`, runtime state, entity telemetry, user intents и transport commands остаются разными типами;
- немедленный command result, долгоживущая operation, trigger event и асинхронная delivery имеют разные IDs и lifecycle;
- OpenAPI и protocol fixtures меняются до реализации нового публичного поведения;
- обычные тесты не обращаются к реальным внешним сервисам; Home Assistant, STT и сеть заменяются deterministic fakes;
- существующие search/voice endpoints и локальный каталог RockCast сохраняют совместимость;
- каждая задача обновляет status/task log соответствующего репозитория и выполняет его обязательные проверки.

## Milestone A — контракт и базовая модель

### DC-000 — подтвердить готовность Windows-first основы

**Репозитории:** RockServer и RockCast.  
**Зависимости:** текущий `windows-production-roadmap.md`.

- Проверить, что RockCast использует стабильный RockServer search/voice path, а production STT и fallback-поведение достаточно определены для следующего этапа.
- Записать остающиеся blockers; не начинать ESP32 firmware ради обхода незавершённого Windows пути.

**Приёмка:** есть зафиксированное решение go/no-go для device-control protocol; известны текущие playback/volume/Chromecast/relay entry points RockCast.

### DC-001 — зафиксировать существующий ownership и новые control scopes — выполнено (2026-09-02)

**Репозиторий:** RockServer.  
**Зависимости:** DC-000.

- Зафиксировать реализованную модель `user_id -> devices.id`, существующие native sessions, list/revoke и pairing как MVP security boundary.
- Оставить `home_id`, shared-home membership и automation principals отдельным будущим расширением, не меняющим текущий account contract неявно.
- Зафиксировать роли `controller`, `player`, `display_surface`, `voice_endpoint`, `sensor_source`, `actuator`, `integration_adapter`.
- Разделить scopes для чтения sensor state, управления media, display и actuator.
- Определить, какие intents требуют уточнения или подтверждения.

**Приёмка (выполнено):** roadmap фиксирует таблицу разрешений для Rockmobile, RockCast, ESP32, будущего automation principal и Home Assistant adapter, минимальные отдельные scopes, server-side checks и intent-safety matrix. Control plane использует текущий `user_id` ownership; cross-user доступ и hidden broadcast запрещены по умолчанию. Никакой новый runtime contract, pairing flow, machine credential, migration, DTO или endpoint не добавлен.

### DC-002 — описать protocol v1 и HTTP/WebSocket контракт — выполнено (2026-09-02)

**Репозиторий:** RockServer.  
**Зависимости:** DC-001.

- Добавить в `api/openapi.yaml` planned `GET /api/v1/device-control/directory` и `GET /api/v1/devices/connect`; сослаться на существующие pairing/device-session/device-list endpoints без их дублирования.
- Описать envelope, version negotiation, registration, heartbeat, manifests, state, telemetry, directory events, typed commands, lifecycle results и errors.
- Задать exact frame/payload/list limits, heartbeat/TTL, registration deadline, in-flight policy, command timeout, idempotency window, revision/resync и compatibility rules.
- Не менять семантику `/api/v1/voice/stream`.

**Приёмка (выполнено):** оба operation машинно помечены `planned` и принимают только
`RockserverBearer`; существующий inventory `GET /api/v1/devices` и voice WebSocket не
переопределены. OpenAPI фиксирует protocol major 1, bounded lifecycle, exact limits,
forward compatibility, typed command vocabulary, server-derived identity и owner/scope policy.
YAML, local refs, discriminator mappings и `operationId` проходят contract validation;
runtime по-прежнему отсутствует.

### DC-003 — создать общие protocol fixtures — выполнено (2026-09-02)

**Репозиторий:** RockServer; копии/генерация для клиентов только после стабилизации.  
**Зависимости:** DC-002.

- Создан канонический versioned набор в `tests/fixtures/device-control/v1/` с raw JSON messages
  и source-neutral normalized Home Assistant entity projections; клиенты должны читать те же
  файлы, а не создавать копии.
- Покрыты RockCast player, ESP32 multi-role registration/manifest/state/telemetry, directory,
  `sensor_grid`, lifecycle command success/failure и bounded semantic rejections.
- `jsonschema` 0.30 добавлен только в dev-dependencies для JSON Schema 2020-12 validation
  component schemas с локальными OpenAPI `$ref`; явные assertions покрывают revision,
  correlation, idempotency и outcomes, которые не следуют из одного JSON document.

**Приёмка (выполнено):** каждый JSON fixture явно зарегистрирован и проверяется против
указанного OpenAPI component schema; тест обнаруживает незарегистрированный/отсутствующий файл,
schema mismatch и schema-invalid example. Unknown command теперь schema-valid на envelope уровне,
но сохраняет structured `unsupported_command`; known command branches остаются строгими.
Runtime, pairing, provider adapter и client/firmware implementation не добавлены.

### DC-004 — реализовать доменные типы без транспорта (выполнено, 2026-09-02)

**Репозиторий:** RockServer.  
**Зависимости:** DC-003.

- Ввести `Device`, `Entity`, `Surface`, capabilities, manifests, device/entity state, `Presentation`, command/result и typed validation errors.
- Сделать неизвестные namespaces forward-compatible, но запретить их исполнение без зарегистрированного handler/schema.
- Нормализовать entity domain, device class, units, area/labels, freshness и quality.

**Приёмка (выполнено):** `src/device_control.rs` публикует transport-agnostic v1-типы
для device/entity/surface/capability/manifest/state/presentation/command и lifecycle результатов,
раздельные typed IDs и safe validation errors. Canonical fixtures подтверждают serde round-trip
RockCast/ESP32 manifest, sensor-grid, telemetry и known/unknown command paths. Unknown namespaced
capabilities сохраняют extra fields; unknown commands остаются opaque и возвращают
`unsupported_command` до будущего handler. Проверены bounds/uniqueness, unit/value normalization,
fixed-time freshness, revision replay/conflict/gap и terminal result invariants. HTTP/WebSocket,
persistence, pairing и provider code не добавлены.

### DC-005 — добавить persistence foundation — выполнено (2026-09-02)

**Репозиторий:** RockServer.  
**Зависимости:** DC-004.

- Добавить migrations и repository traits для `device_capabilities`, entities, surfaces, latest snapshots и bounded command audit/idempotency, используя FK на существующий `devices.id`.
- Не создавать вторую таблицу devices, pairing requests, device secrets или native sessions.
- Хранить provider-native IDs отдельно от публичных IDs; обеспечить tombstone/revocation semantics.
- Не добавлять бесконечную историю sensor telemetry в MVP.

**Приёмка (выполнено):** migration `0021` добавляет только device-owned control projections
с FK на существующий `devices.id`: current manifest/capabilities/entities/surfaces, latest-only
device/entity state и bounded command idempotency. PostgreSQL store фильтрует active user и
non-revoked device внутри read/write transactions; полный manifest tombstones omitted entries,
а revival разрешён только для идентичной прежней public projection. Revisions возвращают
accepted/replay/stale/conflict/resync, command fingerprints защищают 24-hour replay window,
terminal result записывается один раз, а pruning batch-limited и не трогает in-flight records.
Disposable PostgreSQL integration covers an empty migration chain, populated account/device
baseline, owner isolation, revocation, manifest/state/command outcomes, constraints and retention.

## Milestone B — RockServer control plane

### DC-006 — интегрировать control plane с существующей device authentication — выполнено (2026-09-02)

**Репозиторий:** RockServer.  
**Зависимости:** DC-005.

- Повторно использовать завершённый account pairing, существующий durable device secret и `POST /api/v1/auth/device-session` для получения access token.
- Переиспользовать общий validator/extractor для будущего control ingress: короткоживущий native access token разрешается в server-derived principal с существующими `user_id`/`device_id`; client не передаёт и не заменяет эти поля.
- Сохранить текущие device list/revoke и invalid/transient-error semantics; revoked device не подключается.
- Оставить Home Assistant integration credential отдельным типом, не смешивая его с account device credential.

**Приёмка (выполнено):** `device_control_auth` разрешает только bounded native `Bearer` через
существующий session resolver и возвращает именно server-derived `(user_id, device_id)`; expiry,
unknown/revoked session, legacy/admin Bearer и любые cookie не становятся control principal.
Ошибка session store отдельна как retryable `Unavailable` и не меняет durable device binding;
renewal остаётся исключительно `POST /api/v1/auth/device-session`. HTTP guard для будущего
`/api/v1/devices/connect` готов и отдаёт typed invalid/unavailable outcome будущему handler для
controlled `401`/`503`, однако самого WebSocket upgrade, register, registry, heartbeat, TTL и
presence нет: это точная граница DC-007.

### DC-007 — реализовать connection registry и presence — выполнено (2026-09-02)

**Репозиторий:** RockServer.  
**Зависимости:** DC-006.

- Добавить WebSocket upgrade/auth, `device.register`, `device.registered`, heartbeat и graceful disconnect.
- Связать stable `device_id` с единственным active connection policy либо явно определить multi-connection behavior.
- Реализовать server-controlled TTL, offline event и reconnect с новым `connection_id`.

**Приёмка (выполнено):** runtime `GET /api/v1/devices/connect` выполняет native Bearer auth до
upgrade (401 invalid/revoked, 503 unavailable), v1 hello/welcome/register handshake и bounded
text-frame validation. Успешная регистрация создаёт server-issued `connection_id` и online presence;
register не принимает identity fields. Per-user process-local registry имеет bounded replacement
channel/history, atomic single-active policy и connection-ID guard против stale cleanup. Heartbeat
обновляет только server-observed `last_seen`; TTL, graceful close, transport loss, revoke и server
shutdown производят ровно один offline transition active generation. Transport tests покрывают
handshake/auth, reconnect, heartbeat/TTL, invalid/binary/timeout frames, identity injection,
owner isolation и shutdown cleanup. DC-008/DC-010 по-прежнему не реализованы.

### DC-008 — реализовать manifests и state hub — выполнено (2026-09-03)

**Репозиторий:** RockServer.  
**Зависимости:** DC-007.

- Принимать versioned capabilities/entities/surfaces manifest и полный snapshot после регистрации.
- Принимать ordered device/entity deltas, отбрасывать stale revisions и уметь запросить full resync.
- Публиковать авторизованным subscribers online/state/telemetry updates.
- Явно вычислять fresh/stale/unknown, не подменяя отсутствие sensor value нулём.

**Приёмка:** тесты покрывают reconnect, out-of-order delta, manifest replacement, sensor removal, stale time и backpressure медленного subscriber.

`/api/v1/devices/connect` теперь принимает typed manifest при register и последующие typed
manifest/state/entity frames, требует full state перед heartbeat/reconnect и сохраняет latest-only
projection через DC-005 store. Gaps/conflicts запрашивают `device.resync_requested`; stale/replay
не изменяют accepted state. Internal fan-out owner-scoped и bounded/lossy для slow subscribers;
публичного directory API и command router нет.

### DC-009 — реализовать command router — выполнено (2026-09-03)

**Репозиторий:** RockServer.  
**Зависимости:** DC-008.

- Проверять actor scope, home ownership, target presence, capability и payload schema до отправки.
- Реализовать `received → accepted → terminal result`, deadline, cancellation policy и correlation.
- Дедуплицировать `command_id` в bounded idempotency window; не создавать бесконечную offline queue.

**Приёмка (выполнено):** authenticated WebSocket теперь маршрутизирует только explicit
owner-scoped commands с lifecycle `received → accepted → terminal result`; target acknowledgement
не является success. Router проверяет server-derived principal/active generation, controller role,
server-derived scope, owner, presence, target role/capability/entity/surface and bounded payload.
Durable DC-005 reservation uses a SHA-256 fingerprint of the client-visible canonical command for
24-hour replay/conflict, while the server applies the default deadline separately. One exact active
target generation receives delivery; timeout, disconnect/replacement and bounded 16/8 admission
produce a terminal failure without an offline queue. v1 не содержит explicit command cancel, so
deadline/disconnect are its deterministic cancellation policy. DC-010 directory API и DC-011 intents
остаются не реализованы.

### DC-010 — реализовать directory/controller API — выполнено (2026-09-03)

**Репозиторий:** RockServer.  
**Зависимости:** DC-008, DC-009.

- Добавить чтение devices/entities/surfaces с online, capabilities и state freshness.
- Добавить live subscription events для controller UI.
- Поддержать фильтры home, area, domain и device class без раскрытия чужих сущностей.

**Приёмка:** controller получает согласованный initial snapshot и последующие deltas; reconnect не создаёт дубликаты.

Выполнено: `GET /api/v1/device-control/directory` отдаёт owner-scoped snapshot с presence,
capabilities и state freshness, а зарегистрированный controller получает `directory.upsert`
события; отстающий subscriber закрывается `directory_resync_required` и перезагружает HTTP
snapshot. Подробности — в логе задач RockServer за 2026-09-03.

### DC-011 — реализовать typed intents и presentation builder — выполнено (2026-09-03)

**Репозиторий:** RockServer.  
**Зависимости:** DC-004, DC-008, DC-009.

- Ввести `UserIntent` с `play_radio`, `show_sensors`, `query_sensor`, media navigation и ограниченными actuator intents.
- Реализовать deterministic target/entity/surface resolution по explicit IDs, текущему target, area и capabilities.
- Реализовать `Presentation` views `text`, `now_playing`, `sensor_grid`.
- LLM разрешить только построение typed intent; routing, permissions, freshness и command payload строятся обычным кодом.

**Приёмка (выполнено):** `src/device_control_intent.rs` вводит versioned schema-valid
`UserIntent`, server-derived actor/directory/context input и typed plan/clarification/
confirmation/error result. Resolver использует только owner-scoped directory projection,
явные scopes, presence, roles, capabilities, manifests и state; он выбирает explicit ID,
request-local current target, supplied canonical area mapping или единственный candidate и
никогда не broadcast/fallback. `show_sensors` строит bounded `sensor_grid`; fresh values
показываются как current, stale сохраняют value с `stale`, unavailable/missing становятся
явными `null`/unavailable or unknown. Actuator proposal требует explicit entity target и
возвращает confirmation без command dispatch. LLM/voice integration остаются DC-025.

## Milestone C — RockCast и Rockmobile

### DC-012 — подключить RockCast как зарегистрированный player — выполнено (2026-09-04, локально)

**Репозиторий:** RockCast.  
**Зависимости:** DC-007, DC-003.

- Использовать уже сохранённые после pairing `device_id`/device secret и добавить `DeviceControlClient` с access-token renewal и WSS reconnect/backoff.
- Публиковать truthful playback/volume capabilities и полный state snapshot.
- Не менять существующий локальный playback fallback.

**Приёмка:** RockCast появляется online, переживает restart/server loss и после reconnect отправляет корректный snapshot.

Выполнено локально: `DeviceControlClient` на блокирующем tungstenite-транспорте с registration/
state resync покрыт wire/lifecycle-тестами; лог — в репозитории RockCast за 2026-09-04.

### DC-013 — связать RockCast playback и volume commands — выполнено (2026-09-04, локально)

**Репозиторий:** RockCast.  
**Зависимости:** DC-012, DC-009.

- Адаптировать `play_station`, play/pause/stop/next/previous и volume/mute к существующему PlaybackController.
- Возвращать один terminal result и публиковать фактический state после выполнения.
- Обработать invalid station, playback failure, interruption и duplicate command.

**Приёмка:** fake Rockmobile управляет RockCast через RockServer; команда не считается успешной до результата RockCast.

Выполнено локально: `play_station`, media и volume/mute команды адаптированы к `PlaybackController`
с ровно одним terminal result и публикацией фактического state; лог — в репозитории RockCast за
2026-09-04.

### DC-014 — добавить Chromecast и relay adapters — выполнено (2026-09-06, локально)

**Репозиторий:** RockCast.  
**Зависимости:** DC-013.

- Добавить capabilities, discovery result schemas, connect/disconnect и relay mode transitions.
- Определить freshness receiver discovery и поведение при network loss.
- Не регистрировать receiver отдельным target без самостоятельной identity модели.

**Приёмка:** controller видит только поддерживаемые действия; state соответствует реальному output mode; timeout/failure не выглядит как success.

Выполнено локально: `media.chromecast`/`media.relay` capabilities, opaque receiver-хендлы и
взаимоисключающий output state покрыты тестами без живого LAN-ресивера; live Chromecast smoke не
проводился и остаётся отдельной проверкой. Лог — в репозитории RockCast за 2026-09-06.

### DC-015 — встроить target selector в существующий Rockmobile account flow — выполнено (2026-09-06, локально)

**Репозиторий:** Rockmobile.  
**Зависимости:** DC-006, DC-010.

- Повторно использовать существующие login/pairing/device list, добавить расширенный directory snapshot/subscription и last selected target.
- Показывать online/offline/stale и требовать явный выбор при отсутствии подходящего target.
- Не отправлять single-device command широковещательно.

**Приёмка:** пользователь может выбрать RockCast; offline/revoked target обрабатывается без зависшего UI.

Выполнено локально: revision-safe selector поверх directory REST/WSS с явной персистентностью
выбора и видимой обработкой недоступных целей; лог — в репозитории RockMobile за 2026-09-06.

### DC-016 — реализовать capability-driven controls — выполнено (2026-09-07, live E2E)

**Репозиторий:** Rockmobile.  
**Зависимости:** DC-015, DC-013.

- Строить playback/volume/Chromecast/relay controls только по capabilities.
- Показывать lifecycle pending/accepted/success/error и подтверждённый state.
- Игнорировать неизвестные capabilities, сохраняя работоспособность известных.

**Приёмка:** Rockmobile управляет RockCast end-to-end; double tap не дублирует действие; unsupported controls отсутствуют.

Выполнено: controls строятся только по свежим capabilities выбранного target, lifecycle
pending/received/accepted/terminal отображается, дубликаты подавляются. Физический E2E
RockMobile → RockServer → RockCast подтверждён 2026-09-07 (команда `Stop`, terminal `succeeded`
менее чем за секунду через deployed staging). Логи — в репозиториях RockMobile и RockCast за
2026-09-07.

## Milestone D — ESP32 display и sensors

### DC-017 — реализовать ESP32 provisioning и transport core — выполнено (2026-09-09)

**Репозиторий:** ESP32 firmware (rock-esp32).  
**Зависимости:** DC-003, DC-006, DC-007; предусловия bring-up из rock-esp32.

Целевое железо зафиксировано: плата JC4880P443C_I_W (ESP32-P4 v1.3 без радио, Wi-Fi через
ESP32-C6 по ESP-Hosted/SDIO, 4.3" дисплей, 16 MB flash), ESP-IDF 6.1 и Rust через vendored
`esp-idf-sys`.

- Предусловия bring-up (до любого control-трафика): поднять Wi-Fi через ESP32-C6 (ESP-Hosted по
  SDIO), синхронизировать время по SNTP и выполнить один HTTPS-запрос; без корректного времени не
  работают TLS-валидация, `sent_at` и экспирация access-токенов. Перед тестами
  производительности/надёжности flash включить dedicated Boya flash driver.
- Реализовать provisioning поверх существующих RockServer pairing endpoints с минимальным
  экраном pairing на дисплее: short code, verification phrase и QR (матрица через pure-Rust crate
  `qrcode`, рендер в фреймбуфер — локальная задача). Безопасно сохранить выданные
  `device_id`/device secret (NVS), получать access token через `/api/v1/auth/device-session`,
  затем использовать WSS, bounded messages, heartbeat и reconnect jitter.
- Выбрать WSS-транспорт с учётом ESP-IDF 6.1 и vendored `esp-idf-sys` (`esp_websocket_client`
  либо собственный слой на Rust TLS) и зафиксировать решение вместе с memory/task/watchdog limits
  и safe firmware version reporting.
- Реализовать generic capability/manifest/state/command dispatch core; контрактные тесты прошивки
  читают канонические `tests/fixtures/device-control/v1/` RockServer без создания копий.
- Стратегия переиспользования кода RockCast: transport-agnostic часть `device_control/protocol.rs`
  и pairing state machine RockCast — кандидаты на общий crate после стабилизации протокольного
  слоя на обеих платформах; до этого ESP32 реализует минимальный сабсет против канонических
  fixtures. Общий crate живёт в отдельном репозитории (правила rockserver запрещают комбинировать
  серверный и клиентский код). egui-код RockCast на ESP32 не переносится: переносится только
  визуальная тема как дизайн-референс.

**Приёмка (выполнено):** на JC4880P443C_I_W подтверждены ESP32-P4 v1.3, ESP-IDF 6.1,
ESP-Hosted 3.0.7 на P4 и C6, SDIO 4-bit и живое сканирование Wi-Fi на ST7701/LVGL-дисплее.
Прошивка реализует pairing, обычное NVS-хранилище `rock_auth` для `device_id`/`device_secret`,
renewal device session, bounded WSS transport, heartbeat/reconnect и generic
manifest/state/command dispatch. Контрактные тесты против канонических RockServer fixtures
прошли 6/6; P4-образ собран и прошит без стирания NVS. Детали и ограничения — в
`rock-esp32/docs/dc-017-progress.md` и `rock-esp32/docs/device-control.md`.

### DC-018 — реализовать ESP32 display surface

**Репозиторий:** ESP32 firmware.  
**Зависимости:** DC-017, DC-011.

- GUI-стек выбран и подтверждён владельцем (2026-09-08, сравнение через agy + независимая
  проверка фактов): **LVGL v9 через C-компонент `esp_lvgl_port`**. Обоснование: лицензия MIT
  (бесплатно для проприетарного продукта), аппаратное ускорение ESP32-P4 (PPA), кинетические
  списки/скроллы и theming из коробки, совместимость с ESP-IDF 6.1 через Component Registry.
  Rust-сторона владеет протоколом, состоянием и маппингом presentation → view; тонкий слой
  биндингов к LVGL пишется в rock-esp32 самостоятельно, без зависимости от незрелых сторонних
  v9-биндингов (`oxivgl`, `lightvgl-sys` — только как референс). Slint исключён: royalty-free
  лицензия не покрывает embedded, проприетарный продукт требует платного плана, рендер
  программный; остаётся fallback-вариантом, если стоимость собственного FFI-слоя превысит
  стоимость лицензии. embedded-graphics — только provisioning-экран DC-017, для полноценных
  view исключён. Минимальный provisioning-экран DC-017 от финального стека не зависит.
- Зарегистрировать `display.main` и поддержать `text`, `now_playing`, `sensor_grid` в пределах
  hardware profile. Визуальная тема — RockCast-подобная (палитра, шрифты, композиция now_playing)
  как дизайн-референс; egui-код RockCast не переносится и не переиспользуется.
- Рендерить presentation локально; не принимать произвольный HTML/script.
- Инициализировать и проверить GT911; touch допускается для локального подтверждения,
  повторной попытки и навигации внутри уже показанного view, но не создаёт browse/search
  protocol и не расширяет v1-контракт.
- Публиковать текущий view и terminal command result.
- Граница v1-скоупа: интерактивный browse каталога и аудиовыход на самом устройстве не входят в
  эту задачу — они за явными продуктовыми решениями DC-039/DC-040.

**Приёмка:** golden presentations стабильно рисуются на целевом дисплее; touch корректно
обрабатывается после power cycle; неизвестный view даёт `capability_not_supported`; повторная
`display.show_view` не создаёт новый побочный эффект.

### DC-019 — реализовать ESP32 sensor modules

**Репозиторий:** ESP32 firmware.  
**Зависимости:** DC-017, DC-008.

**Статус:** отложена владельцем 2026-09-09: физические датчики пока отсутствуют. Возобновить
после появления hardware; задача не отменена.

- Добавить драйверы первых temperature/humidity sensors за внутренним provider interface.
- Публиковать entity manifest и telemetry с unit, observed time, quality и configurable stale interval.
- Обрабатывать sensor missing/read error/reconnect без ложного значения.

**Приёмка:** RockServer видит свежие и stale состояния; reboot и отключение датчика дают корректный manifest/state transition.

### DC-020 — завершить sensor-display end-to-end

**Репозитории:** RockServer и ESP32 firmware.  
**Зависимости:** DC-011, DC-018, DC-019.

**Статус:** отложена владельцем 2026-09-09 вместе с DC-019: без доступных физических датчиков
невозможно подтвердить end-to-end приёмку. Возобновить после DC-019 и появления hardware.

- Провести typed intent `show_sensors` через entity resolution и presentation builder.
- Отправить `display.show_view(sensor_grid)` на нужную surface.
- Отобразить stale/unavailable явно и подтвердить показанный view state.

**Приёмка:** команда «покажи датчики» рисует температуру/влажность, а не список радио; тест покрывает no sensors, stale value и ambiguous area.

## Milestone D2 — интерактивный GUI и аудио на ESP32 (продуктовые расширения)

Обе задачи строятся на проверенном transport/display core из Milestone D, но не входят в
protocol v1 MVP. Каждая начинается отдельным решением владельца продукта; при выборе расширения
контракта изменения `api/openapi.yaml` и fixtures идут до реализации. Ни одна из них не блокирует
Milestone E–G.

### DC-039 — спроектировать и реализовать интерактивный browse GUI на ESP32

**Репозитории:** RockServer (только при контрактном расширении) и ESP32 firmware.  
**Зависимости:** DC-018; явное продуктовое решение о скоупе on-device GUI.

**Статус:** выбрана владельцем для выполнения 2026-09-09; продуктовое решение — реализовать
интерактивный выбор станции на устройстве. Любое требуемое контрактное расширение должно быть
зафиксировано в OpenAPI и fixtures до реализации.

- Зафиксировать продуктовое решение: интерактивный выбор станции на самом устройстве
  (browse/search/play с touch) против управления только с controller.
- При положительном решении определить контрактный путь: новые bounded presentation types
  (например, `station_list`/`menu`) с подтверждаемым view state либо ESP32-роль `controller` с
  локальным каталогом и явными scopes; обновить OpenAPI и канонические fixtures до реализации.
- Реализовать UI в выбранном на DC-018 стеке: список/поиск станций, now_playing, подтверждение
  команды; touch-ввод остаётся локальным и не создаёт второй API или прямой мобильный протокол.
- Синхронизировать визуальную тему с RockCast (палитра, шрифты, композиция) как дизайн-референс.

**Приёмка:** выбор станции на устройстве проходит через RockServer intent/command путь с явным
target; неизвестные/oversized presentations отвергаются; при offline-сервере UI показывает явный
offline state, а не устаревший или фиктивный список.

### DC-040 — решить и при необходимости реализовать аудиовыход на ESP32

**Репозитории:** ESP32 firmware; RockServer только при контрактных изменениях.  
**Зависимости:** DC-017; явное продуктовое решение о необходимости локального звука.

- Зафиксировать продуктовое решение: является ли ESP32 самостоятельным player (I2S-выход,
  декодер MP3/AAC, буферизация сетевого стрима) или остаётся display/sensor/voice-устройством при
  воспроизведении на RockCast/Chromecast.
- При положительном решении: определить hardware profile (I2S-кодек/ЦАП, RAM-ограничения
  декодера), truthful `media.playback`/`media.volume` capabilities в пределах железа и поэтапный
  план (stream client → декодер → вывод → soak); playback state подтверждается только фактом.
- При отрицательном решении: зафиксировать в документации, что реальная прошивка не публикует
  `media.playback`/`media.volume` (контрактный fixture multi-role ESP32 остаётся примером
  протокола, а не описанием этой платы); скоуп Milestone D при этом полный без аудио.

**Приёмка:** манифест реальной прошивки декларирует только физически доступные функции;
реализованный звук проходит power cycle/reconnect soak, а нереализованный — явно исключён без
молчаливого обещания capability.

### RS-задачи — контракты и серверная реализация плана RockCast-радио (2026-09-09)

Выполняются по плану `rock-esp32/docs/rockcast-device-plan.md` (шаги 2–3), координация —
`rock-esp32/docs/plan-control.md`. Порядок жёсткий: контракты (RS-1) закрываются до любой
реализации (RS-2/RS-3/RS-4). Находки аудита 2026-09-09: F1 (play_stream только planned),
F2 (catalog не в OpenAPI, анонимный, отдаёт stream_url), F3 (voice без device-auth/cancel,
дрейф лимитов), F4 (Rockmobile жёстко валидирует media.volume), F10 (ranked search
не курсоруется).

#### RS-1 — зафиксировать контракты до реализации — выполнено (2026-09-09)

**Репозиторий:** RockServer (только контракты, fixtures, документация и контрактные тесты).

Сделано:

- `api/openapi.yaml` 0.5.0: два `x-rockserver-status: planned` device-facing пути каталога с
  аутентификацией RockserverBearer — `GET /api/v1/device-control/catalog/stations`
  (browse: явный курсор = последний стабильный station id, ≤512 ASCII, ≤20 на страницу) и
  `GET /api/v1/device-control/catalog/search` (query ≤128, ≤20 результатов, одна страница,
  без курсора — решение F10). Оба возвращают новый `DeviceStationDto` БЕЗ `stream_url`
  (и без score/reason/provider-полей); `PublicStationDto` не переиспользуется.
- `station.play_stream` переведён из свободного enum в структурно-исполнимую схему: вариант
  `source=rockserver_catalog` (только серверный, обязателен `station_id` — эхо разрешённой
  станции) и вариант `source=direct_stream` (контроллерский, только при объявленной
  capability). Переход `play_station → play_stream` сохраняет `command_id` и lifecycle
  accepted/result ≤30 с; SSRF-ограничения (http/https, ≤2048, запрет приватных диапазонов,
  ≤5 редиректов, never logged) сохранены в общей схеме `StationStreamUri`.
- Voice: устранён дрейф `/api/v1/voice/stream` с рантаймом (ровно 16000 Гц mono pcm_s16le,
  чанк ≤32 КиБ, сессия ≤2 МиБ/60 с, idle 10 с, wall 75 с, provider 15 с — машиночитаемо в
  `x-voice-stream-limits`; лимит результатов потока ≤10); задокументировано planned-расширение
  RS-4: device-session аутентификация, `surface_id=voice.main`, серверный `source_device_id`,
  явный cancel-фрейм `VoiceStreamCancel` (marked planned).
- Зафиксировано решение: `station_list`, `search`, `loading`, `offline`, `playback_error` —
  локальные UI-состояния устройства; remote presentation types остаются ровно
  `text`/`now_playing`/`sensor_grid` (описано в `DisplayCapability`).
- Fixtures: `esp32-register-client.json` переведён в финальную правдивую форму радио
  (roles player/controller/display_surface/voice_endpoint; `media.station` только
  `rockserver_catalog`; volume 0/100/step 1/mute; surfaces `display.main` + `voice.main`;
  без сенсоров — DC-019 отложен); `esp32-manifest-client.json` (rev 2) и
  `directory-snapshot-server.json` синхронизированы (rev 2 добавляет sensor-часть как пример
  возобновления DC-019); добавлен `device-catalog-response.json` (`DeviceCatalogPage` без
  stream_url). В README fixtures зафиксированы семантика «accepted немедленно, result
  асинхронно ≤30 с (10 с expiry в Rockmobile — норма)» и правило Rockmobile-совместимости
  media.volume (0..100, step 1..100, нарушение валидно отвергается схемой).
- Контрактные тесты `tests/openapi_contract.rs` расширены: структурные проверки обоих путей
  каталога, положительная и отрицательная валидация `DeviceStationDto` (в т.ч. с stream_url —
  invalid), страницы >20 станций, кривого курсора; play_stream-варианты (отрицательные:
  rockserver_catalog без station_id, direct_stream с station_id, ftp://, >2048); volume
  (negative: min/max вне 0..100, step 0/101, level вне 0..100); display views (station_list
  и др. — schema-invalid); voice (значения `x-voice-stream-limits`, 8000/24000/44100 Гц и
  limit 50 — invalid, cancel-фрейм); runtime-проверка, что planned-пути каталога не
  зарегистрированы роутером (404).

**Приёмка (выполнено):** OpenAPI и все канонические fixtures валидируются
(`cargo test --test openapi_contract`, 8/8); planned-маркеры не сняты ни с одной операции
(снимаются только в RS-2/RS-3/RS-4 по мере реализации); выбор станции всегда несёт явный
player `device_id` (target обязателен в схеме команды); ни один контракт не раскрывает
секреты, провайдерные идентификаторы или невалидированные stream URL; runtime-код не изменён.

#### RS-2 — реализовать device-facing каталог — выполнено (2026-09-09)

**Репозиторий:** RockServer (runtime + контракты + тесты + документация).

Сделано:

- Оба пути каталога из контракта RS-1 зарегистрированы роутером и реализованы в
  `src/http/device_catalog.rs` поверх существующих сервисов: `browse`
  (`GET /api/v1/device-control/catalog/stations`, курсор = стабильный id последней станции,
  `^[A-Za-z0-9-]+$` ≤512, лимит 1..=20, дефолт 20) через `SearchService::public_catalog` и
  `search` (`GET /api/v1/device-control/catalog/search`, GET с query-параметрами: `q` ≤128
  символов и не из одних пробелов, `locale` BCP 47-like c дефолтом `en-US`, лимит 1..=20)
  через `SearchService::interpret_and_search` под тем же 5-секундным таймаут-бюджетом, что у
  публичного `/api/v1/search` (таймаут маппится в 503 — у контракта нет 504).
- Аутентификация — существующий механизм device-session: Bearer короткоживущей нативной
  сессии (`POST /api/v1/auth/device-session`) проверяется через
  `authenticate_control_ingress` → `NativeSessionResolver` (тот же путь, что у
  `/device-control/directory`; отсутствие/просрочка/ревокация/не-нативный токен → 401,
  недоступность store → 503 c Retry-After). Новый формат токенов и scope не вводились:
  v1 достаточно валидной неотозванной сессии (решение RS-1).
- Ответы — `DeviceCatalogPage`/`DeviceSearchPage` из нового `DeviceStationDto`: маппинг из
  доменной `Station`/`RankedStation` вырезает `stream_url` и поля score/reason/provider
  на сервере конструктивно (в DTO эти поля отсутствуют); поиск не курсоруется, browse
  отдаёт `next_cursor` = id последней станции полной страницы; 200-е ответы несут
  `Cache-Control: no-store`.
- Rate limits: browse 60/20 и search 30/10 (числа как у публичных каталога/поиска), но
  бакеты per-device (`AppState::device_request_allowed`, ключ `endpoint:device_id`), чтобы
  одно разговорчивое устройство не съедало квоту флота; 429 c `Retry-After` и
  `details.limit_scope="device"`. Отклонённые до аутентификации запросы бакет не расходуют.
- Порядок проверки: request_id → аутентификация → per-device quota → валидация параметров
  (400 `malformed_request`, включая неизвестные query-параметры через
  `deny_unknown_fields`) → сервис. Детерминированные 4xx подтверждены тестами.
- Попутно исправлен латентный баг курсорной пагинации in-memory репозитория:
  `InMemoryStationRepository::list_public`/`list_admin` теперь сортируют по стабильному id
  (как `ORDER BY s.id` в Postgres); ранее фильтр `id > after` по несортированному пинну
  мог повторять станции на странице — это нарушало бы «страница строго после курсора».
- `x-rockserver-status: planned` снят ровно с двух операций каталога (browse/search →
  `implemented`) и обновлено описание info; planned-маркеры `station.play_stream` (RS-3) и
  voice-расширения (RS-4) не тронуты.
- Тесты: новый `tests/device_catalog_api.rs` (12 интеграционных тестов: happy path обоих
  путей, постраничный обход 41 станции без повторов строго после курсора, границы
  limit/cursor/q (0/21/пустой/пробельный/512/513/129 символов/не-ASCII/unknown-параметры),
  401 без токена и с неизвестным/просроченным, 503 при недоступном auth-store, таймаут
  поиска → 503 с `timeout_ms`, отсутствие `stream_url`/`score`/`reason`/`provider_id` в
  ответах, включая станции с невалидным `stream_url` (фикстурный репозиторий),
  per-device 429 с изоляцией квот между устройствами). Контрактные тесты RS-1 обновлены:
  статус `implemented` и рантайм-проверка «пути зарегистрированы и требуют нативную
  device-сессию (401)» вместо прежней «404». Публичные `/catalog` и `/search` не изменены
  (кроме сортировки страниц по id в in-memory режиме — см. выше) и покрыты прежними тестами.

**Приёмка (выполнено):** оба контракта RS-2 реализованы и покрыты runtime-тестами
(`cargo test --test device_catalog_api` 12/12, `--test openapi_contract` 8/8);
`stream_url`, провайдерные идентификаторы и секреты не возвращаются и не логируются;
planned-статусы других операций не тронуты; firmware-границы соблюдены (≤20 на страницу,
одна страница поиска, валидный курсор, детерминированные отказы).

Открытый вопрос для RS-3: 403 «revoked or forbidden» из контракта каталога остаётся
зарезервированным — ревокация устройства/сессии видна как 401 (сессия перестаёт
резолвиться), отдельного 403-пути v1 не требует.

#### RS-3 — реализовать резолюцию play_station → play_stream — выполнено (2026-09-09)

**Репозиторий:** RockServer (runtime + контракты + тесты + документация).

Сделано:

- `CommandBody` получил типизированный вариант `PlayStream` (`station.play_stream`): вариант
  `source=rockserver_catalog` (обязательное эхо `station_id` + `stream_uri`) и вариант
  `source=direct_stream` (только `stream_uri`, без station_id). Десериализация строго
  воспроизводит замороженную схему `StationCommand`: каталог-вариант без `station_id`,
  direct-вариант с `station_id`, неизвестный/отсутствующий `source` — невалидны.
- В `CommandRouter::submit` команда `station.play_station` резолвится на сервере: после
  `validate_target` (роль `player` + `media.station` с source `rockserver_catalog`) и после
  вычисления fingerprint/резерва (дедуп 86 400 с работает по ОРИГИНАЛЬНОЙ команде
  контроллера) станция ищется в каталоге через новый трейт `StationCatalog` (реализация —
  `SearchService::public_station`, тот же каталог, что и публичные/device-пути; бюджет 5 с),
  `stream_url` проверяется SSRF-валидатором формы (http/https, хост, без userinfo/fragment,
  порт 1..=65535 или дефолт схемы, ≤2048, литеральные IPv4/IPv6 и всегда-локальные имена —
  только публичные назначения), и тело заменяется на `station.play_stream` под тем же
  `command_id` ДО диспатча таргету. `stream_uri` не попадает ни в persistence (резерв
  хранит исходную `play_station`-команду), ни в lifecycle-кадры контроллера, ни в логи,
  ни в тексты ошибок.
- Контроллерский `station.play_stream` с `source=rockserver_catalog` отвергается как
  `invalid_payload` (по контракту вариант серверный); `source=direct_stream` допускается
  только таргету, объявившему этот source (`capability_not_supported` иначе), и проходит
  тот же SSRF-валидатор формы при вводе.
- Детерминированные отказы резолюции дают terminal failed result с кодом из замороженного
  enum: неизвестный station_id, станция без stream_url, невалидный/запрещённый URI →
  `invalid_payload` с фиксированными сообщениями («Unknown station identifier.» / «Station
  has no playable stream.» / «Stream address is not allowed.»); транзиентный отказ/таймаут
  каталога → `persistence_unavailable`. Отказ воспроизводится идемпотентно: повтор той же
  команды в окне дедупа возвращает сохранённый result без повторной резолюции и диспатча.
  Отсутствие подключённого каталога у роутера — синхронный `persistence_unavailable` до
  создания lifecycle-записи.
- Семантика lifecycle не изменилась: accepted — от таргета немедленно, result — асинхронно
  ≤30 с; поздний result после клиентского expiry — норма. Поведение прямых
  `playback.*`/`volume.*`/display/entity команд не тронуто.
- SSRF DNS-уровень (резолв имени в приватный адрес, проверка каждого из ≤5 редиректов)
  сервером НЕ выполняется: общего исходящего egress-слоя в репозитории нет. Валидатор
  проверяет форму URI и литеральные адреса (+ `localhost`, чисто-цифровые хосты); DNS-разрыв
  задокументирован в Rustdoc `validate_stream_uri` и в status.md как ограничение; проверку
  редиректов по контракту выполняет стрим-клиент таргета (RE-5).
- `x-rockserver-status: planned` снят со схемы `StationStreamUri` (→ `implemented`),
  обновлено описание info; planned-маркеры voice (RS-4) не тронуты.
- `CommandRouter` подключается к каталогу во всех продакшн-билдерах роутера (включая
  Postgres) через `with_station_catalog`; каталог возвращает наружу только stream URL.
- Тесты: юнит `CommandBody::PlayStream` (round-trip обоих вариантов, строгие формы,
  validate_at) и батарея `validate_stream_uri` (5 публичных ok; 25 запрещённых назначений,
  включая IPv4-mapped/compatible IPv6 и `localhost`; 12 структурных отказов — порт 0/65536/
  нечисловой, userinfo, fragment, ftp/верхний регистр, >2048); роутер-тесты (6): happy path
  (таргет получил `station.play_stream` под тем же command_id с эхом station_id; контроллер
  — обычный received→accepted→result; replay дублята без повторного диспатча), 5
  детерминированных отказов (unknown/empty/malformed/private/loopback) — terminal
  `invalid_payload` без диспатча, без URI в сериализациях кадров, идемпотентный replay
  отказа; транзиентный отказ каталога → terminal `persistence_unavailable`; гейтинг
  контроллерских play_stream (спуфинг rockserver_catalog, forbidden direct URI,
  легальный direct_stream на объявивший таргет); target без rockserver_catalog source и
  роутер без каталога. Контрактные тесты: статус `StationStreamUri` = implemented,
  voice-маркер остался planned.

**Приёмка (выполнено):** резолюция и SSRF-валидация живут в серверном командном роутере;
тот же `command_id`, fingerprint/идемпотентность по исходной команде контроллера;
`stream_uri` отсутствует в persistence, lifecycle-кадрах контроллера, логах, текстах
ошибок, фикстурах и документации; контрактные тесты RS-1/RS-2 остаются зелёными
(`cargo test --test openapi_contract` 8/8, `--test device_catalog_api` 12/12);
`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` и полный
`cargo test` зелёные; поведение прямых playback/volume команд не изменилось.

#### RS-4 — реализовать voice device-session, cancel и UserIntent-роутинг — ожидает

Аутентифицировать device-сессии на `/voice/stream`, прокинуть `source_device_id`,
`voice.main` и locale через сессию, реализовать `VoiceStreamCancel`; маршрут распознанного
транскрипта через типизированный `UserIntent` в существующий командный роутер (радио-intents);
снять planned-маркеры с voice-расширения.

Примечание (Rockmobile, план шаг 2.8): изменения протокола Rockmobile не требуются —
«Играть на устройстве» (RM-1) строит `station.play_station` через существующую модель
lifecycle; 10-секундный клиентский deadline с пометкой Expired остаётся ожидаемым
поведением, серверный bound 30 с покрывает асинхронный result.

## Milestone E — Home Assistant

### DC-021 — создать Home Assistant connection adapter

**Репозиторий:** RockServer.  
**Зависимости:** DC-005, DC-008.

- Добавить конфигурацию endpoint/credential reference, connection health и bounded reconnect.
- Реализовать discovery preview и явный allowlist до импорта.
- Изолировать Home Assistant DTOs/WebSocket protocol за provider trait.

**Приёмка:** fake HA server покрывает auth failure, reconnect, oversized event и secret-safe logs; без конфигурации startup не ломается.

### DC-022 — синхронизировать read-only HA entities

**Репозиторий:** RockServer.  
**Зависимости:** DC-021.

- Нормализовать разрешённые sensor domains, device classes, units, areas и availability.
- Подписаться на state changes и сохранять provider-native cursor/identity mapping.
- Tombstone удалённые entities; не переиспользовать public ID молча.

**Приёмка:** ESP32/Rockmobile одинаково отображают собственные и HA sensors; duplicates, unavailable и stale покрыты тестами.

### DC-023 — добавить ограниченные HA actuator actions

**Репозиторий:** RockServer.  
**Зависимости:** DC-001, DC-009, DC-021, DC-022.

- Начать с allowlisted `switch`/`light`; сопоставить command schema с точными service calls.
- Проверять scope, entity allowlist, payload bounds и confirmation policy.
- Завершать command только после service result и/или подтверждённого state transition.

**Приёмка:** нельзя вызвать произвольный service/domain; cross-home и non-allowlisted entity запрещены; audit не содержит secrets.

## Milestone F — голос на ESP32

### DC-024 — добавить ESP32 voice transport

**Репозитории:** ESP32 firmware и RockServer.  
**Зависимости:** DC-017 и production-ready STT из Windows roadmap.

- Зарегистрировать `voice.main` с точными audio capabilities.
- Передавать bounded audio с source device/surface, locale, cancellation и timeout.
- Вернуть transcript/result без дублирования существующего provider-neutral STT слоя.

**Приёмка:** silence, cancel, network loss, unsupported format и STT failure безопасны; аудио/транскрипт не попадают в обычные логи.

### DC-025 — расширить CommandInterpreter до общего UserIntent

**Репозиторий:** RockServer.  
**Зависимости:** DC-011, DC-024.

- Сохранить существующие radio intents и добавить `show_sensors`, `query_sensor`, display и разрешённые actuator intents.
- Добавить schema/semantic validation для target, area, entity class и response mode.
- Реализовать clarification/confirmation flow без прямого LLM tool execution.

**Приёмка:** deterministic fake и provider conformance tests дают одинаковые typed intents; malformed provider output не исполняет команду.

### DC-026 — голосовой multi-domain E2E

**Репозитории:** RockServer и ESP32 firmware.  
**Зависимости:** DC-020, DC-023, DC-024, DC-025.

- Проверить «включи рок», «какая температура на кухне», «покажи датчики» и одну разрешённую actuator-команду.
- Проверить выбор исходной display/voice surface, explicit target override, ambiguity и stale state.
- Подтвердить, что sensor intent не попадает в radio search response.

**Приёмка:** четыре сценария проходят на fake providers и ручном hardware smoke; ошибки имеют понятный экранный/голосовой ответ.

## Milestone G — server-side operations и асинхронные ответы

### DC-027 — определить operation, event и delivery contract

**Репозиторий:** RockServer.  
**Зависимости:** DC-002, DC-011, DC-026.

- Добавить typed `Operation`, `OperationTrigger`, `OperationAction`, `Delivery` и lifecycle events.
- Отделить immediate `operation.created` от будущих `triggered/completed/failed/cancelled`.
- Задать `operation_id`, `trigger_id`, `event_id`, `delivery_id`, revisions, deadlines и idempotency semantics.
- Описать подписку controller, list/get/cancel API и событие после отключения исходной сессии.

**Приёмка:** контракт однозначно описывает таймер, который принят сейчас, срабатывает позже и не использует старый `command.result`; fixtures проходят schema validation.

### DC-028 — добавить durable operation store и scheduler

**Репозиторий:** RockServer.  
**Зависимости:** DC-005, DC-027.

- Добавить migrations/repositories для operations, triggers и action results.
- Реализовать scheduler с UTC storage, явной timezone presentation, monotonic/restart-safe calculations и bounded polling/wakeup.
- Добавить cancellation, restart recovery, claim/lease и exactly-once-effect через idempotent action keys.
- Не считать «ровно один запуск worker» гарантией ровно одного внешнего эффекта.

**Приёмка:** таймер переживает server restart, cancel не срабатывает, два worker не создают два action effects, просроченный trigger имеет явный status.

### DC-029 — реализовать typed server executor registry и таймер

**Репозиторий:** RockServer.  
**Зависимости:** DC-011, DC-028.

- Ввести allowlisted `ServerExecutor` handlers со schema, permission class и sync/async result type.
- Реализовать `timer.create/list/cancel` без произвольного кода, URL или provider method из LLM output.
- На trigger создавать типизированную presentation/action plan, например «Таймер завершён».

**Приёмка:** deterministic clock tests покрывают create/list/cancel/trigger, restart, duplicate claim и invalid duration/timezone; LLM не может выбрать незарегистрированный executor.

### DC-030 — реализовать weather read-only provider

**Репозиторий:** RockServer.  
**Зависимости:** DC-011, DC-029.

- Добавить provider-neutral weather trait и нормализованные current/forecast DTO.
- Определить разрешение location из explicit user/home settings; не передавать provider произвольные device данные.
- Добавить timeout, bounded response, cache/freshness, safe error и deterministic fake.
- Формировать text/display/speech presentation независимо от конкретного weather API.

**Приёмка:** текущая погода и краткий прогноз работают через fake provider; stale cache/provider failure явно отражены; обычные тесты не используют сеть.

### DC-031 — определить speech output capability и delivery protocol

**Репозитории:** RockServer и ESP32 firmware contract fixtures.  
**Зависимости:** DC-018, DC-027.

- Описать `speech_output` с modes `audio_url`, `audio_stream`, optional `local_tts`, codecs, max duration/size и interruption support.
- Добавить delivery states `queued/dispatched/received/playing/completed/failed/expired`.
- Добавить priority, `queue/duck/interrupt`, quiet-hours и destination/fallback policy.
- Разделить JSON control frames и binary/audio delivery path.

**Приёмка:** старое устройство без `speech_output` остаётся совместимым; `received` и `completed` различаются; скрытый broadcast исключён.

### DC-032 — добавить TTS provider и временные audio assets

**Репозиторий:** RockServer.  
**Зависимости:** DC-030, DC-031.

- Добавить provider-neutral TTS trait, deterministic fake audio и bounded text/language/voice request.
- Создавать временный audio asset с codec, size, duration, hash/reference и коротким TTL.
- Реализовать авторизованную одноразовую/короткоживущую загрузку или отдельную stream session.
- Не логировать текст, audio bytes, storage path или signed URL; добавить cleanup expired assets.

**Приёмка:** fake TTS delivery проходит без внешней сети; expired/unauthorized asset недоступен; oversized/unsupported audio отвергается.

### DC-033 — реализовать durable delivery outbox

**Репозиторий:** RockServer.  
**Зависимости:** DC-028, DC-031, DC-032.

- Сохранять асинхронную доставку до отправки, отдельно от active controller connection.
- Реализовать target resolution: explicit surface → preferred home/area surface → разрешённый fallback → expire.
- Добавить bounded retry/backoff, deadline, reconnect deduplication и acknowledgement processing.
- Повторно проверять ownership/scope/capability перед dispatch.

**Приёмка:** offline пульт получает событие после reconnect в пределах deadline либо delivery завершается `expired`; один `delivery_id` не воспроизводится дважды.

### DC-034 — реализовать speech output на пульте/ESP32

**Репозиторий:** ESP32 firmware; при необходимости отдельный репозиторий пульта.  
**Зависимости:** DC-017, DC-018, DC-031, DC-033.

- Объявлять реальные codecs/modes/buffer limits и audio output surface.
- Получать authorized audio asset либо stream, проверять метаданные и воспроизводить с bounded buffer.
- Реализовать queue/duck/interrupt в рамках capability и отправлять `received/playing/completed/failed`.
- Хранить короткий deduplication window для `delivery_id` через reconnect/reboot согласно hardware limits.

**Приёмка:** синхронный и асинхронный голос воспроизводятся; таймер не теряется при отсутствии controller; unsupported codec и playback failure видны серверу.

### DC-035 — реализовать action orchestrator

**Репозиторий:** RockServer.  
**Зависимости:** DC-009, DC-023, DC-029, DC-033.

- Преобразовывать trigger/result в набор typed actions: speech, display, mobile notification и allowlisted entity/device command.
- Возвращать aggregate status с отдельным result каждого target/action.
- Повторно проверять permission/capability/current availability при срабатывании.
- Retry только идемпотентных actions или actions с устойчивым idempotency key; partial failure не маскировать.

**Приёмка:** один таймер может озвучить сообщение и включить разрешённое устройство; отозванное право блокирует actuator, но сохраняет понятный partial result.

### DC-036 — асинхронные timer/weather E2E

**Репозитории:** RockServer, ESP32/pult и controller client.  
**Зависимости:** DC-030, DC-034, DC-035.

- Проверить создание таймера голосом, immediate acknowledgement, disconnect controller и последующее speech/display срабатывание.
- Проверить синхронный и асинхронный прогноз погоды с выбранной surface.
- Проверить restart, cancel, offline surface, fallback, quiet hours, expiry, duplicate event и TTS/provider failure.
- Проверить составной action: голос плюс разрешённая entity activation.

**Приёмка:** все lifecycle состояния видимы и детерминированы; устаревший голос не проигрывается после deadline; внешнее действие не дублируется.

## Milestone H — эксплуатация и расширение

### DC-037 — production hardening и ограниченный rollout

**Репозитории:** все затронутые.  
**Зависимости:** DC-016, DC-020, DC-022, DC-036; DC-023 только для actuator feature flag.

- Добавить rate/concurrency limits, metrics, traces, alerts, audit retention, credential rotation, backup/restore и revocation runbooks.
- Добавить operation backlog/lag, scheduler drift, TTS latency, delivery retry/expiry и audio storage metrics.
- Провести reconnect/soak, slow consumer, command storm, duplicate scheduler claim, compromised device и Home Assistant/TTS outage tests.
- Включать RockCast control, ESP32 sensors/display, HA read-only, HA actuators, ESP32 voice и async speech отдельными feature flags/rollout gates.

**Приёмка:** измерены command/delivery latency, scheduler drift, reconnect rate, stale telemetry и failure rate; rollback отключает новые paths без поломки search/voice/local playback.

### DC-038 — группы, scenes и automation API

**Репозиторий:** RockServer, затем соответствующие controllers.  
**Зависимости:** DC-035, DC-037.

- Ввести automation-specific credentials/scopes и explicit trigger/action model поверх operation/action foundation.
- Реализовать group command как aggregate с отдельным result для каждого target.
- Добавить scenes только после определения partial failure, rollback и idempotency semantics.

**Приёмка:** automation не использует user session token; single-device command никогда не превращается в скрытый broadcast; частичный результат видим вызывающей стороне.

## Контрольные точки

1. После DC-003 контракт можно независимо реализовывать в клиентах.
2. После DC-011 готов RockServer control-plane simulator.
3. После DC-016 готов первый полезный продукт: Rockmobile → RockServer → RockCast.
4. После DC-020 готов ESP32 с датчиками и экраном.
5. После DC-022 один экран показывает ESP32 и Home Assistant sensors.
6. После DC-026 готов голосовой multi-domain сценарий.
7. После DC-029 RockServer умеет долговечный таймер без активного controller.
8. После DC-034 пульт принимает синхронную и асинхронную речь.
9. После DC-036 готовы timer/weather и составные server actions end-to-end.
10. DC-037 обязателен перед широким использованием actuator, async speech и automation функций.
