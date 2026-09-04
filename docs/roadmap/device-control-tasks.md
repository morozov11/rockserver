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

### DC-010 — реализовать directory/controller API

**Репозиторий:** RockServer.  
**Зависимости:** DC-008, DC-009.

- Добавить чтение devices/entities/surfaces с online, capabilities и state freshness.
- Добавить live subscription events для controller UI.
- Поддержать фильтры home, area, domain и device class без раскрытия чужих сущностей.

**Приёмка:** controller получает согласованный initial snapshot и последующие deltas; reconnect не создаёт дубликаты.

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

### DC-012 — подключить RockCast как зарегистрированный player

**Репозиторий:** RockCast.  
**Зависимости:** DC-007, DC-003.

- Использовать уже сохранённые после pairing `device_id`/device secret и добавить `DeviceControlClient` с access-token renewal и WSS reconnect/backoff.
- Публиковать truthful playback/volume capabilities и полный state snapshot.
- Не менять существующий локальный playback fallback.

**Приёмка:** RockCast появляется online, переживает restart/server loss и после reconnect отправляет корректный snapshot.

### DC-013 — связать RockCast playback и volume commands

**Репозиторий:** RockCast.  
**Зависимости:** DC-012, DC-009.

- Адаптировать `play_station`, play/pause/stop/next/previous и volume/mute к существующему PlaybackController.
- Возвращать один terminal result и публиковать фактический state после выполнения.
- Обработать invalid station, playback failure, interruption и duplicate command.

**Приёмка:** fake Rockmobile управляет RockCast через RockServer; команда не считается успешной до результата RockCast.

### DC-014 — добавить Chromecast и relay adapters

**Репозиторий:** RockCast.  
**Зависимости:** DC-013.

- Добавить capabilities, discovery result schemas, connect/disconnect и relay mode transitions.
- Определить freshness receiver discovery и поведение при network loss.
- Не регистрировать receiver отдельным target без самостоятельной identity модели.

**Приёмка:** controller видит только поддерживаемые действия; state соответствует реальному output mode; timeout/failure не выглядит как success.

### DC-015 — встроить target selector в существующий Rockmobile account flow

**Репозиторий:** Rockmobile.  
**Зависимости:** DC-006, DC-010.

- Повторно использовать существующие login/pairing/device list, добавить расширенный directory snapshot/subscription и last selected target.
- Показывать online/offline/stale и требовать явный выбор при отсутствии подходящего target.
- Не отправлять single-device command широковещательно.

**Приёмка:** пользователь может выбрать RockCast; offline/revoked target обрабатывается без зависшего UI.

### DC-016 — реализовать capability-driven controls

**Репозиторий:** Rockmobile.  
**Зависимости:** DC-015, DC-013.

- Строить playback/volume/Chromecast/relay controls только по capabilities.
- Показывать lifecycle pending/accepted/success/error и подтверждённый state.
- Игнорировать неизвестные capabilities, сохраняя работоспособность известных.

**Приёмка:** Rockmobile управляет RockCast end-to-end; double tap не дублирует действие; unsupported controls отсутствуют.

## Milestone D — ESP32 display и sensors

### DC-017 — реализовать ESP32 provisioning и transport core

**Репозиторий:** ESP32 firmware.  
**Зависимости:** DC-003, DC-006, DC-007.

- Реализовать ESP32 UI/provisioning поверх существующих RockServer pairing endpoints, безопасно сохранить выданные `device_id`/device secret, получать access token через `/api/v1/auth/device-session`, затем использовать WSS, bounded messages, heartbeat и reconnect jitter.
- Задать memory/task/watchdog limits и safe firmware version reporting.
- Реализовать generic capability/manifest/state/command dispatch core.

**Приёмка:** power cycle, Wi-Fi loss и server restart не теряют identity; malformed/oversized message не перезапускает устройство.

### DC-018 — реализовать ESP32 display surface

**Репозиторий:** ESP32 firmware.  
**Зависимости:** DC-017, DC-011.

- Зарегистрировать `display.main` и поддержать `text`, `now_playing`, `sensor_grid` в пределах hardware profile.
- Рендерить presentation локально; не принимать произвольный HTML/script.
- Публиковать текущий view и terminal command result.

**Приёмка:** golden presentations стабильно рисуются на целевом дисплее; неизвестный view даёт `capability_not_supported`.

### DC-019 — реализовать ESP32 sensor modules

**Репозиторий:** ESP32 firmware.  
**Зависимости:** DC-017, DC-008.

- Добавить драйверы первых temperature/humidity sensors за внутренним provider interface.
- Публиковать entity manifest и telemetry с unit, observed time, quality и configurable stale interval.
- Обрабатывать sensor missing/read error/reconnect без ложного значения.

**Приёмка:** RockServer видит свежие и stale состояния; reboot и отключение датчика дают корректный manifest/state transition.

### DC-020 — завершить sensor-display end-to-end

**Репозитории:** RockServer и ESP32 firmware.  
**Зависимости:** DC-011, DC-018, DC-019.

- Провести typed intent `show_sensors` через entity resolution и presentation builder.
- Отправить `display.show_view(sensor_grid)` на нужную surface.
- Отобразить stale/unavailable явно и подтвердить показанный view state.

**Приёмка:** команда «покажи датчики» рисует температуру/влажность, а не список радио; тест покрывает no sensors, stale value и ambiguous area.

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
