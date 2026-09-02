# Roadmap: управление RockCast, ESP32 и устройствами умного дома

## Статус и цель

Это **планируемая** архитектура; она не описывает реализованное поведение RockServer. Текущий публичный контракт RockServer отвечает за поиск станций и голосовой поток. Новый контур управления должен быть добавлен отдельными совместимыми этапами, не меняя существующие search и voice-маршруты.

### Уже реализованная основа аккаунтов и устройств

Этот roadmap не проектирует новую регистрацию устройств. RockServer уже имеет `users`, привязанные к аккаунту `devices`, краткоживущие `pairing_requests`, долговечный хешированный `device_secret`, короткоживущие native access sessions, `POST /v1/auth/device-session`, `GET /v1/devices` и отзыв устройства. RockCast и Rockmobile уже используют этот account/device boundary.

Control plane обязан повторно использовать существующие `devices.id`, ownership, pairing, device-session renewal и revoke semantics. Новый WebSocket аутентифицируется обычным короткоживущим native access token. Новые таблицы могут только расширять существующее устройство capabilities, connections, entities, surfaces и state. Параллельная таблица устройств, второй pairing flow или отдельный machine credential для RockCast не создаются.

Текущая модель ownership — устройство принадлежит `user_id`. `home_id`, комнаты, совместный доступ и automation principals являются будущими аддитивными расширениями и не должны блокировать первый control-plane MVP или неявно менять существующую безопасность аккаунта.

Цель — один расширяемый протокол, через который Rockmobile, голосовая точка или домашняя автоматизация обращаются к RockCast, ESP32 и сущностям умного дома. Первый сценарий — управление воспроизведением на Windows RockCast. Дальнейшие сценарии не должны требовать второго API: ESP32 сможет одновременно быть player, экраном, голосовой точкой и источником датчиков, а Home Assistant — поставщиком и исполнителем нормализованных home-entities.

```text
Rockmobile / голос / automation (controller)
        -> RockServer intent + device control plane
        -> registry, routing, state и policy
        -> RockCast / ESP32 / Home Assistant adapter
        -> player / display / sensor / actuator
```

Rockmobile не открывает прямой управляющий канал к RockCast, ESP32 или Home Assistant. RockServer аутентифицирует участников, знает доступные устройства и entities, проверяет разрешённые действия и маршрутизирует их к target. Физическое действие и подтверждение фактического состояния остаются обязанностью соответствующего устройства или integration adapter; поиск станции продолжает использовать существующий RockServer search/voice pipeline.

## Основные решения

- **Роли и surfaces, а не жёсткие типы устройств.** `controller` инициирует намерения и команды; `player`, `display_surface`, `voice_endpoint`, `sensor_source`, `actuator` и `integration_adapter` предоставляют независимые функции. Rockmobile обычно `controller`; RockCast — `player` и иногда локальный `controller`; ESP32 может сочетать `player`, `display_surface`, `voice_endpoint` и `sensor_source`; Home Assistant подключается как `integration_adapter`.
- **DeviceCapabilities — часть регистрации и контракта.** Клиент строит доступное управление по capability, а не по строке `device_type`. Тип (`rockcast`, `esp32`) — лишь диагностика и удобный default UI.
- **Capabilities, state и commands — разные сущности.** Capability отвечает «может ли устройство это делать», state — «что происходит сейчас», command — «что требуется сделать». Команда не изменяет state сама по себе: state подтверждает player.
- **Сервер авторитетен для directory, policy и маршрутизации, provider — для фактического состояния.** RockServer хранит последний принятый snapshot и online-присутствие, но источником истины о воспроизведении, датчике или реле остаётся RockCast, ESP32 либо Home Assistant adapter.
- **Расширение без пробелов в enum.** Каждое сообщение версионируется; команды и capabilities имеют стабильные строковые имена, допускающие неизвестные будущие значения. Старый controller скрывает неизвестное, старый player отвергает неизвестную команду структурированной ошибкой.

## Термины и модель данных

### Идентичность и доступ

`device_id` — существующий `devices.id`, выданный завершением текущего account pairing и уже используемый native session. RockCast/ESP32 сохраняет его вместе с существующим device secret. `entity_id` — стабильный адрес отдельной функции или ресурса внутри узла или интеграции, например `sensor.kitchen_temperature` или `light.bedroom`. `surface_id` — адрес экрана/голосовой точки, на которой должен появиться результат. `connection_id` — новый на каждое WebSocket-подключение. Сейчас `user_id` определяет владельца и изолированный namespace; будущий `home_id` добавляется только отдельной миграцией и threat-model решением. Display name и тип не являются ключами маршрутизации.

Один device может предоставлять много entities и surfaces. Например, один ESP32 имеет `media_player`, `sensor.temperature`, `sensor.humidity`, `display.main` и `voice.main`. Home Assistant adapter может публиковать множество entities, не регистрируя каждую как отдельное физическое устройство.

Минимальные серверные записи:

| Сущность | Назначение | Сохраняемые данные |
| --- | --- | --- |
| существующая `devices` | уже реализованная account-owned identity | существующие `id`, `user_id`, display name, device type, app version, `device_secret_hash`, activity/revocation; schema не дублируется |
| `device_capabilities` | versioned расширение существующего устройства | `device_id` FK, protocol/manifest revision, capabilities snapshot, updated_at |
| `entities` | нормализованные функции устройства или интеграции | `entity_id`, provider/device, domain, class, unit, readable/controllable flags, metadata, availability |
| `surfaces` | доступные точки вывода/ввода | `surface_id`, device, kind (`display`, `voice`, `mobile`), supported presentation formats |
| `device_connections` | эфемерное присутствие | `connection_id`, device, connected/last_seen, server instance; удаляется при disconnect/expiry |
| `device_state_snapshots` | последнее подтверждённое состояние device | `device_id`, monotonically increasing `state_revision`, state JSON, observed_at |
| `entity_state_snapshots` | последнее значение entity | `entity_id`, value, unit, quality, observed_at, received_at, stale_after |
| `device_command_log` | короткоживущий аудит и идемпотентность | `command_id`, target, controller, type, result, accepted/finished timestamps, bounded retention |
| `integration_connections` | настройки внешнего adapter | integration type, endpoint reference, encrypted credential reference, sync cursor, status; секреты не лежат в JSON state |
| `operations` | долгоживущая server-side работа | `operation_id`, owner, kind, input, status, scheduled/started/finished timestamps, cancellation/version data |
| `operation_actions` | результаты одного срабатывания | operation/trigger ID, action type, target/surface, status, attempts, terminal result |
| `delivery_outbox` | надёжная асинхронная доставка | event ID, destination policy, payload reference, priority, not-before/deadline, attempts, acknowledgement state |
| `audio_assets` | временный TTS-результат | content type/codec, bounded size/duration, encrypted storage reference, expires_at; raw audio не хранится в command log |

Не хранить stream URL, токены Chromecast или секреты в логе команд. Их могут передавать только в нужном command payload по защищённому соединению и они не должны попадать в обычные журналы.

### DeviceCapabilities

Capabilities — сравнительно стабильная декларация прошивки/приложения или integration adapter. Provider присылает полный набор при `device.register`/`integration.sync` и повторно при обновлении. Каждая capability имеет namespace, version и параметры, чтобы UI и intent router не гадали о границах.

```json
{
  "protocol_version": 1,
  "device_type": "esp32",
  "roles": ["player", "display_surface", "voice_endpoint", "sensor_source"],
  "capabilities": {
    "playback": {"supported": true, "actions": ["play", "pause", "stop", "next", "previous"]},
    "volume": {"supported": true, "min": 0, "max": 100, "step": 1, "mute": true},
    "station_selection": {"supported": true, "sources": ["rockserver_catalog", "direct_stream"]},
    "display": {"supported": true, "surfaces": ["main"], "views": ["text", "sensor_grid", "now_playing"]},
    "sensors": {"supported": true, "entity_classes": ["temperature", "humidity"]},
    "voice_input": {"supported": true, "audio": ["pcm16_mono_16000"], "wake_word": false},
    "speech_output": {"supported": true, "delivery": ["audio_url", "audio_stream"], "codecs": ["opus", "mp3"], "local_tts": false},
    "relay": {"supported": false},
    "home_assistant": {"supported": false}
  }
}
```

Для несинхронных сценариев тот же versioned envelope переносит `operation.created`, `operation.updated`, `operation.triggered`, `delivery.available` и `delivery.updated`. Controller подписывается на доступные ему operations/events, но доставка на speech/display surface не зависит от активной controller-сессии. Для получения временного audio asset используется отдельный авторизованный HTTP endpoint или бинарная audio session; большие audio chunks не смешиваются с JSON control frames.

Первый RockCast MVP объявляет только реально работающие playback/volume/Chromecast/relay возможности. ESP32 объявляет только физически доступные функции конкретной прошивки и подключённых модулей. После подключения или отключения датчика ESP32 повторно публикует capabilities/entities snapshot с новой revision. Capability `supported: false` можно не передавать; она показана выше только для ясности примера.

### Runtime state

State изменчив, имеет `state_revision`, timestamp и partial-update семантику. Его нельзя использовать для вывода о поддержке функции. Пример:

```json
{
  "type": "device.state",
  "device_id": "9b0e…",
  "state_revision": 42,
  "observed_at": "2026-09-02T12:00:00Z",
  "state": {
    "online": true,
    "playback": {"status": "playing", "station_id": "station-jazz-001", "position_ms": null},
    "volume": {"level": 35, "muted": false},
    "output": {"mode": "chromecast", "receiver_name": "Living room TV"},
    "display": {"surface_id": "display.main", "view": "sensor_grid"},
    "entities": {
      "sensor.kitchen_temperature": {"value": 23.4, "unit": "°C", "quality": "ok"},
      "sensor.kitchen_humidity": {"value": 41, "unit": "%", "quality": "ok"}
    }
  }
}
```

Обязательный серверный presence-state ограничен `online`, `last_seen` и причиной disconnect. Остальной state — best effort: controller показывает `unknown` или «обновляется», если snapshot/telemetry устарели. Provider отправляет полный snapshot после регистрации/reconnect и delta после каждого значимого фактического изменения. Для sensor value отдельно хранятся `observed_at`, `received_at`, `quality` и `stale_after`; отсутствие свежего значения нельзя превращать в ноль.

### Commands

Команда содержит `command_id` (UUID, идемпотентный), `target_device_id`, `type`, типизированный `payload`, actor и deadline. Сервер проверяет ownership, online, capability и schema, затем отправляет её player. Player отвечает `command.accepted`, потом ровно одним terminal `command.result`; параллельно рассылает подтверждающий state.

Начальный набор:

| Группа | Команды |
| --- | --- |
| Playback | `play`, `pause`, `stop`, `next`, `previous` |
| Station | `play_station` (stable `station_id`; player при необходимости получает поток через серверный API), `play_stream` (строго валидируемый URL, только если capability разрешает) |
| Volume | `set_volume`, `change_volume`, `set_mute` |
| Chromecast | `chromecast.discover`, `chromecast.connect`, `chromecast.disconnect` |
| Relay | `relay.start`, `relay.stop`, `relay.set_mode` |
| Display | `display.show_view`, `display.show_text`, `display.dismiss` |
| Actuator | `entity.turn_on`, `entity.turn_off`, `entity.set_value` — только для entity с соответствующим command schema |

`chromecast.discover` возвращает данные через `command.result`; это действие, а не capability. Capability говорит только, что discovery/handoff поддерживаются. Чтение температуры также не является командой устройству: RockServer читает последний свежий entity state или запрашивает refresh у provider. Новые действия добавляются namespaced-командами без перегрузки playback-команд.

### Entities, telemetry и presentation model

Device — это узел подключения и доверия. Entity — адресуемая функция или значение. Surface — место, где показывается/произносится результат. Такое разделение нужно, чтобы фраза «покажи датчики» не зависела от того, пришла температура непосредственно с ESP32 или через Home Assistant.

Нормализованная entity содержит:

- стабильный `entity_id`, `device_id` или `integration_id`, domain (`sensor`, `switch`, `light`, `climate`, `media_player`) и device class (`temperature`, `humidity`, `co2`);
- readable state schema, unit, availability, freshness/quality и необязательный controllable command schema;
- human-readable name, room/area и labels для безопасного intent resolution;
- provider-native ID только как внутреннюю привязку adapter, не как публичный контракт.

RockServer хранит только последнее оперативное значение в основном state store. Долговременная история измерений — отдельная будущая подсистема или ответственность Home Assistant; её не следует неявно строить внутри первого device-control MVP.

Presentation — типизированная модель результата, а не готовая bitmap-картинка. Для protocol v1 достаточно `text`, `now_playing` и `sensor_grid`. Команда `display.show_view` передаёт `view`, `surface_id`, заголовок и нормализованные карточки. ESP32 сам рисует подходящий layout под свой экран; Rockmobile использует собственные widgets. Это позволяет одной фразе породить эквивалентный результат на разных surfaces.

Пример команды для ESP32:

```json
{
  "command_id": "uuid",
  "target_device_id": "kitchen-esp32",
  "type": "display.show_view",
  "payload": {
    "surface_id": "display.main",
    "view": "sensor_grid",
    "title": "Датчики кухни",
    "items": [
      {"entity_id": "sensor.kitchen_temperature", "label": "Температура", "value": 23.4, "unit": "°C", "quality": "ok"},
      {"entity_id": "sensor.kitchen_humidity", "label": "Влажность", "value": 41, "unit": "%", "quality": "ok"}
    ]
  }
}
```

### Intent layer

Voice/mobile intent не совпадает с transport command. `show_sensors`, `query_sensor`, `play_radio` или `set_light` сначала проходят детерминированную validation/policy фазу, затем intent router выбирает entities и output surface, строит одну или несколько конкретных commands и ждёт подтверждённый результат.

```text
«Покажи датчики на кухне» на kitchen-esp32
  -> intent: show_sensors(area=kitchen, surface=current)
  -> resolve readable sensor entities
  -> reject missing/stale values or mark them stale
  -> render Presentation(view=sensor_grid)
  -> command display.show_view -> kitchen-esp32/display.main
  -> state confirms that sensor_grid is visible
```

LLM может переводить естественный язык в ограниченный typed intent, но не получает unrestricted tool access и не отправляет raw commands. Selection, permissions, capability checks, freshness checks и command construction выполняются обычным кодом. Для неоднозначной опасной команды система запрашивает уточнение; для read-only «покажи датчики» может использовать текущий/последний выбранный display surface.

### Server-side executors и долгоживущие operations

Не каждое намерение адресовано внешнему устройству. RockServer может сам исполнять ограниченные server-side intents через зарегистрированные executors:

- `timer.create`, `timer.list`, `timer.cancel` — durable scheduler с явной timezone/clock semantics;
- `weather.current`, `weather.forecast` — read-only provider с bounded timeout/cache/fallback;
- в будущем — reminders и безопасные automation workflows, но не произвольный запуск кода.

Синхронная команда возвращает окончательный результат в рамках текущего interaction. Долгая или отложенная команда возвращает `operation.created` с `operation_id`, текущим status и временем следующего события. Это подтверждает принятие таймера, но не его срабатывание. Позже scheduler создаёт отдельный `operation.triggered` event, а action orchestrator выполняет настроенные действия и записывает результат каждого из них.

```text
«Поставь таймер на 10 минут и скажи на кухонном пульте»
  -> typed intent timer.create(duration=10m, output=speech, surface=kitchen-remote)
  -> validate owner, timezone, surface capability и delivery policy
  -> persist operation; immediate response: operation.created
  ... controller может отключиться ...
  -> durable scheduler emits operation.triggered
  -> build speech presentation «Таймер завершён»
  -> TTS provider creates bounded temporary audio
  -> delivery outbox sends speech to kitchen-remote
  -> remote acknowledges received/playing/completed
  -> operation/action reaches terminal status
```

Executor registry должен быть allowlist: intent kind сопоставляется с типизированным handler и schema. LLM не выбирает сетевой URL, provider method или произвольное устройство. При срабатывании operation RockServer повторно проверяет ownership, scopes, актуальные capabilities и availability; разрешение, существовавшее при создании таймера, не даёт вечного права управлять отозванным устройством.

### Асинхронные ответы и voice delivery

Голосовой ответ является `Presentation` для surface с capability `speech_output`, а не продолжением входящей voice WebSocket-сессии. Поэтому источник запроса может уже отключиться, когда появляется ответ. Delivery destination задаётся явно либо выбирается policy из разрешённых surfaces: исходная поверхность → предпочитаемая поверхность комнаты/home → разрешённый fallback. Скрытый broadcast запрещён.

Для speech output capability объявляет поддерживаемые способы:

- `audio_url` — предпочтительный MVP: RockServer/TTS создаёт временный bounded audio asset, а пульт скачивает его по короткоживущей авторизованной ссылке;
- `audio_stream` — отдельная бинарная delivery session для низкой задержки или длинного ответа; control WebSocket хранит только метаданные и управление;
- `local_tts` — необязательный режим, когда RockServer передаёт нормализованный текст, а устройство синтезирует речь локально.

Контракт доставки использует независимые `event_id` и `delivery_id`, состояния `queued`, `dispatched`, `received`, `playing`, `completed`, `failed`, `expired`, deadline и bounded retry. `received` не означает, что звук был воспроизведён. Устройство сообщает `playing/completed`; при reconnect сервер дедуплицирует delivery. Если surface offline, политика явно выбирает: ждать до deadline, доставить на fallback surface, отправить мобильное уведомление либо завершить `expired`. По умолчанию старый голосовой ответ после deadline не воспроизводится.

Speech action содержит priority, interruption policy (`queue`, `duck`, `interrupt`) и quiet-hours policy. Таймер/будильник может иметь более высокий приоритет, чем прогноз погоды, но правила задаются продуктом и пользователем; RockServer не должен бесконтрольно перебивать текущее воспроизведение. TTS provider находится за trait, получает только разрешённый bounded текст и не определяет target/actions. Audio assets имеют короткий TTL, размер/длительность ограничены, доступ проверяется при скачивании, а содержимое и signed URLs не журналируются.

### Составные server actions

Одно событие может породить несколько типизированных actions: озвучить результат, показать карточку, включить реле или вызвать разрешённую Home Assistant entity. Action orchestrator возвращает aggregate result по каждому target; частичный успех видим и не маскируется общим `success`. Для actuator action обязательны актуальные permissions, capability/schema validation и отдельный audit. Retry разрешён только для идемпотентной action либо с устойчивым idempotency key.

## Протокол и WebSocket

Добавить отдельный canonical endpoint, например `GET /api/v1/devices/connect`; существующий `/api/v1/voice/stream` остаётся только голосовым протоколом. Все control-сообщения — JSON text frames в envelope:

```json
{
  "protocol_version": 1,
  "message_id": "uuid",
  "type": "device.command",
  "sent_at": "2026-09-02T12:00:00Z",
  "payload": {}
}
```

### Жизненный цикл подключённого device/provider

1. Уже paired RockCast/ESP32 получает короткоживущий access token через существующий `POST /v1/auth/device-session`, затем открывает защищённый WebSocket с этим token. Pairing secret и device secret через control WebSocket не передаются. Server-side Home Assistant adapter использует отдельную integration credential и не имитирует account-device pairing.
2. Provider отправляет `device.register`: stable ID, roles, type/name, app/firmware version, полный `DeviceCapabilities`, entities/surfaces manifest и полный `DeviceState`.
3. RockServer создаёт/обновляет directory entry, связывает с живым connection и отвечает `device.registered` с server time, heartbeat interval и актуальными policy limits.
4. Provider отправляет `heartbeat`, `device.state` и `entity.state`; сервер обновляет presence и публикует изменения авторизованным controllers.
5. При чистом disconnect отправляется `device.offline`; при потере сети сервер переводит устройство offline по TTL. Reconnect повторяет регистрацию и даёт новый `connection_id`.

### Жизненный цикл controller и target device

1. Rockmobile подключается как `controller` или использует HTTP для чтения snapshot; для live UI предпочтителен тот же WebSocket.
2. Сервер отправляет `device.list`/`device.upsert` только для устройств данного owner/home: id, name, type, online, capabilities, свежесть state.
3. Rockmobile хранит явный `selected_target_device_id` локально и показывает selector. На старте выбирает последний online target; если его нет — просит выбор, а не отправляет команду «всем».
4. Controller посылает `device.command`. Сервер отвечает немедленным `command.received` или структурированной ошибкой (`target_offline`, `capability_not_supported`, `forbidden`, `invalid_payload`).
5. Player подтверждает принятие и результат. Сервер ретранслирует lifecycle и state controller-у. UI различает «отправляется», «выполняется», «выполнено» и «не выполнено».

Для server-side operation controller получает `operation.created` и может читать/list/cancel operation по `operation_id`. Последующее срабатывание приходит как новое событие, а не как запоздалый `command.result` старой сессии.

Команда с тем же `command_id` не должна воспроизводиться повторно после reconnect; сервер и player могут вернуть сохранённый terminal result в течение согласованного idempotency window. Команды не кладутся в бесконечную offline-очередь: для MVP `target_offline` — окончательная ошибка. Явное отложенное действие — будущая отдельная feature с TTL и UX-подтверждением.

### Голос с ESP32

1. ESP32 voice endpoint открывает существующую либо совместимую bounded audio session и передаёт `source_device_id`, `source_surface_id`, locale и audio format.
2. RockServer выполняет STT и получает transcript через provider-neutral recognizer.
3. `CommandInterpreter` расширяется до общего typed `UserIntent`; прежние radio intents остаются совместимым подмножеством.
4. Intent router резолвит target/entities/surface с учётом home, area, permissions, capabilities и freshness. Неоднозначная или опасная команда требует уточнения/подтверждения.
5. RockServer выполняет конкретные commands и возвращает presentation/result на исходный ESP32: экран, короткий голосовой ответ или оба варианта согласно capabilities.

### Home Assistant adapter

Home Assistant подключается через отдельный provider adapter с минимально необходимым token scope. Adapter импортирует allowlisted entities и их state/events в общую модель, нормализует units/device classes/areas и преобразует разрешённые RockServer commands в Home Assistant service calls. Неизвестные domains и attributes сохраняются как unsupported metadata, а не автоматически становятся исполняемыми tools.

Для первой версии интеграция односторонне настраивается в RockServer: discover → review/allowlist → sync. Удалённая entity становится unavailable/tombstoned, а не молча переиспользуется под другой native ID. ESP32 может показывать как собственные датчики, так и разрешённые entities из Home Assistant одинаковым `sensor_grid` presentation.

### Надёжность, безопасность и наблюдаемость

- Использовать TLS/WSS вне localhost и существующий короткоживущий native access token. Pairing, durable device secret, token renewal и revoke остаются в уже реализованном account/device контуре; второй pairing flow запрещён.
- Изолировать directory, подписки и команды по owner/home; проверять роль и право controller управлять target на сервере.
- Ограничить frame size, частоту state/command, число соединений на device и число in-flight команд; выдавать стандартную error shape (`code`, `message`, `request_id`, `details`).
- Heartbeat/TTL должны быть server-controlled; online не означает доступность Chromecast receiver.
- Пропускать state revisions назад, но уметь принять полный resync после reconnect. Не считать `command.result` успешным без player-originated terminal response.
- Для датчиков учитывать freshness, unit и quality; stale/unknown данные нельзя озвучивать или рисовать как актуальные без явной маркировки.
- Разделить read-only intents и действия с побочным эффектом. Реле, замки, климат и другие actuator-команды требуют более узких scopes, audit и при необходимости подтверждения.
- Scheduler, operation store и delivery outbox должны переживать restart RockServer; повторный запуск не создаёт второе срабатывание или повторное actuator action.
- Ограничить TTS text/audio size, asset TTL, retry count и delivery deadline; соблюдать quiet hours, priority и interruption policy.
- Логировать correlation IDs, device IDs в безопасной форме, latency и outcome; не логировать access/device/pairing secrets, stream URLs и пользовательские голосовые данные.

## Изменения контракта и границы репозиториев

- `api/openapi.yaml` — источник истины для HTTP endpoints и WebSocket event schemas: endpoint, auth, envelope, регистрации, directory/state, command/result/error и compatibility policy.
- RockServer повторно использует реализованные account/device pairing, sessions, list/revoke и реализует только control-plane domain DTO/validation, связанные с существующим `devices.id` capabilities/entities/surfaces, connection registry, state/telemetry normalization, intent routing, presentation building, server-side executor registry, durable scheduler/operations, TTS/audio delivery, command routing, idempotency/audit и contract/integration tests.
- RockCast реализует persistent device ID, reconnect/backoff, capabilities/state adapters и исполнение поддерживаемых команд через существующий playback/Chromecast/relay код. Он сохраняет локальный каталог как offline fallback.
- Rockmobile повторно использует существующие login/pairing/device-list/session flows и добавляет selector target device, capability-driven controls, optimistic-but-pending command UI, список/cancel долгоживущих operations и отображение online/stale/error state.
- ESP32 реализует совместимый multi-role device после стабилизации protocol v1: сначала player/display, затем sensors, voice input и `speech_output`. Firmware не создаёт второй API или прямой мобильный протокол.
- Home Assistant adapter остаётся изолированным provider boundary: credentials, native entity IDs, websocket/event subscriptions и service calls не протекают в публичный device protocol.

## Этапы реализации

### Phase 0 — зафиксировать контракт и product decisions

- [ ] Зафиксировать существующий `user_id -> devices.id` ownership как MVP boundary; `home_id`/shared-home оформить отдельным будущим расширением и зафиксировать permission matrix для новых control scopes.
- [ ] Выбрать canonical `/api/v1/devices/connect` и HTTP read endpoints; не менять voice WebSocket.
- [ ] Описать OpenAPI/AsyncAPI-style schemas для devices, entities, surfaces, capabilities, state, telemetry, presentation и commands; зафиксировать protocol v1, error codes, limits, heartbeat/TTL, compatibility и deprecation rules.
- [ ] Утвердить initial command set и exact semantics `play_station`/`play_stream`; явно задокументировать, какой сервис резолвит stream URL.
- [ ] Зафиксировать typed intent vocabulary (`play_radio`, `show_sensors`, `query_sensor`, actuator intents) и правило «LLM интерпретирует, обычный код разрешает и исполняет».
- [ ] Добавить protocol fixtures и negative examples для unknown capability/command, stale sensor, duplicate command, missing surface и offline target.

**Готово, когда:** schema и примеры reviewable без кода; Rockmobile и RockCast могут начать независимую реализацию против fixtures.

### Phase 1 — RockServer device control foundation

- [ ] Ввести доменные типы `Device`, `Entity`, `Surface`, `DeviceCapabilities`, `DeviceState`, `EntityState`, `Presentation`, `DeviceCommand`, `CommandResult`, role/presence/permission types и строгую validation layer.
- [ ] Добавить migrations/repositories только для связанных с существующим `devices.id` capabilities, entities, surfaces, snapshots и bounded command log; не дублировать `devices`, pairing/sessions, не смешивать со station catalog persistence и не строить sensor history в MVP.
- [ ] Интегрировать control WebSocket с существующей native access-session validation и расширить существующий device directory новыми projections online/capabilities/state; текущие list/revoke semantics сохранить.
- [ ] Реализовать WebSocket registration, entity/surface manifest, heartbeat/TTL, per-user connection registry, state/telemetry fan-out и graceful disconnect handling.
- [ ] Реализовать command router: capability and authorization check, command correlation, idempotency, timeout and terminal result handling.
- [ ] Реализовать deterministic intent resolver и presentation builder независимо от LLM/provider.
- [ ] Добавить metrics/logging and readiness policy без раскрытия secrets.

**Тесты:** domain unit tests; Axum/real WebSocket tests с fake players; migration/repository tests; OpenAPI contract tests; cross-user isolation; TTL/reconnect; duplicate and out-of-order events; payload/frame/rate limits.

### Phase 2 — RockCast как первый player

- [ ] Создать `DeviceControlClient` с persistent ID, WSS reconnect/backoff и registration/state resync.
- [ ] Собрать truthful RockCast capabilities из реально доступных playback/volume/Chromecast/relay функций.
- [ ] Адаптировать команды к существующему `PlaybackController`; один terminal result на command и state update только после реального изменения.
- [ ] Добавить поддержку `play_station` через текущий RockServer search/catalog path и сохранить offline local-catalog fallback.
- [ ] Реализовать Chromecast/relay только после определения result/state transitions и ошибок receiver discovery.

**Готово, когда:** один RockCast переживает restart/reconnect, виден как online/offline, а fake/real Rockmobile может управлять playback и volume без прямого connection к Windows.

### Phase 3 — Rockmobile controller UX

- [ ] Повторно использовать существующие pairing/login/device list и добавить selector с ясной online/offline/stale индикацией и last selected target.
- [ ] Строить controls только из capabilities; не показывать Chromecast/relay/voice/display для неподдерживающего player.
- [ ] Подписываться на state/command lifecycle, показывать pending/error, предотвращать destructive double taps через `command_id`.
- [ ] Реализовать recovery: target went offline, capability changed, outdated state, reconnect and reselect.
- [ ] Добавить integration/E2E сценарии с RockCast fake и настоящим Windows smoke test.

### Phase 4 — Chromecast и relay hardening

- [ ] Уточнить ownership/session model Chromecast receiver, discovery freshness и disconnect semantics.
- [ ] Добавить receiver list/result schemas, errors and timeouts; receiver никогда не становится самостоятельным target device без регистрации.
- [ ] Проверить relay modes, interruption, network loss, multiple controller conflicts и actual state confirmation.
- [ ] Добавить telemetry/operational dashboards for command latency, failures, reconnects and stale state.

### Phase 5 — ESP32 player и display surface

- [ ] Выбрать hardware/firmware limits и capability profile; не обещать unsupported streaming/Chromecast.
- [ ] Подключить ESP32 к существующему pairing/device-session flow, безопасно хранить выданные `device_id`/device secret и реализовать WSS reconnect/heartbeat/state resync с bounded memory.
- [ ] Поддержать минимальные `playback`/`volume` и `display.show_view` commands, включая `now_playing`, `text` и `sensor_grid`; command handler должен быть идемпотентным.
- [ ] Провести soak tests для Wi-Fi loss, power cycle, server restart, firmware update и malformed/unknown messages.
- [ ] Добавить ESP32 profile to contract fixtures so Rockmobile works unchanged.

### Phase 6 — ESP32 sensors и entity telemetry

- [ ] Реализовать firmware manifest для подключённых temperature/humidity/CO₂ и будущих sensor modules.
- [ ] Публиковать entity metadata и state с unit, quality, observed time и freshness; hot-plug/reboot меняют manifest revision безопасно.
- [ ] Добавить RockServer state normalization и read API/subscription для sensor entities.
- [ ] Провести end-to-end «покажи датчики»: typed intent → свежие entities → `sensor_grid` → подтверждённый display state.

### Phase 7 — Home Assistant integration

- [ ] Реализовать server-side adapter с connection health, allowlisted discovery, entity normalization и event subscription.
- [ ] Сначала включить read-only sensor entities; проверить duplicate names, areas, unavailable/stale и entity removal.
- [ ] Затем добавить узко разрешённые actuator service calls с scopes, audit, timeout и подтверждённым state.
- [ ] Проверить единый sensor view, смешивающий локальные ESP32 entities и разрешённые Home Assistant entities.

### Phase 8 — голос на ESP32

- [ ] Добавить voice endpoint capability и bounded audio session с source device/surface identity.
- [ ] Добавить `speech_output` capability, временный audio asset/download и подтверждения `received/playing/completed`.
- [ ] Расширить существующий `CommandInterpreter` до typed `UserIntent`, сохранив совместимость radio intents.
- [ ] Реализовать intent routing, clarification/confirmation policy и возврат presentation/voice response на исходное устройство.
- [ ] Проверить `включи радио`, `какая температура`, `покажи датчики` и разрешённую actuator-команду при success, ambiguity, stale state и provider failure.

### Phase 9 — server-side operations, таймеры и погода

- [ ] Зафиксировать operation/event/delivery contract и отделить `operation.created` от будущего terminal trigger/action result.
- [ ] Реализовать durable scheduler, operation store и delivery outbox с restart recovery, cancellation, deadline и idempotency.
- [ ] Добавить allowlisted executors: timer как локальный server executor и weather как read-only provider behind trait.
- [ ] Добавить TTS provider boundary, bounded temporary audio assets и маршрутизацию на `speech_output`/display/mobile surfaces.
- [ ] Реализовать delivery priority, queue/duck/interrupt, quiet hours, retry/fallback/expiry и acknowledgements.
- [ ] Проверить асинхронный сценарий таймера при отключённом controller и сценарий погоды с голосовым/экранным ответом.
- [ ] Разрешить operation trigger активировать allowlisted device/entity actions только после повторной проверки прав и capability.

### Phase 10 — automation, группы и production hardening

- [ ] Добавить отдельные controller scopes для home panels/automations; не переиспользовать пользовательские session tokens.
- [ ] Добавлять scenes/groups только как отдельный aggregate с результатом по каждому target; не делать скрытый broadcast single-device command.
- [ ] Зафиксировать retention, audit, revocation, backup/restore, rate limits, metrics и alerts.
- [ ] Провести долгие reconnect/soak и security tests для RockCast, ESP32 и Home Assistant одновременно.

## Порядок выполнения

Сначала завершить текущий Windows-first voice/search roadmap. Затем выполнять Phase 0 → 1 → 2 → 3; после устойчивого RockCast end-to-end — Phase 4 → 5 → 6. Home Assistant read-only integration (Phase 7) может идти после общей entity/state модели, но actuator-команды — только после permission/audit foundation. Голос ESP32 (Phase 8) опирается на готовые sensors, display, intents и существующий STT путь. Durable operations и асинхронная speech delivery (Phase 9) опираются на готовые surfaces, permissions и command routing, но не требуют активного controller connection. Исполнимый порядок отдельных работ записан в [`device-control-tasks.md`](device-control-tasks.md).

## Definition of Done для protocol v1

- Устройство регистрируется, reconnects и корректно становится online/offline без ручного обновления Rockmobile.
- Rockmobile или голосовой endpoint выбирает однозначный target/surface и управляет им исключительно через RockServer.
- UI определяется capabilities; factual UI и sensor values — подтверждённым runtime state с freshness/quality.
- Каждая команда коррелирована, авторизована, capability-validated, идемпотентна и завершена success/error result.
- `покажи датчики` отображает `sensor_grid`, а не radio result, независимо от того, пришли entities с ESP32 или Home Assistant.
- LLM не вызывает устройства напрямую: typed intent проходит deterministic resolution, permissions и validation.
- Таймер переживает restart RockServer, срабатывает без активного controller и доставляет голос/экран либо завершается явным `expired/failed` по policy.
- Пульт с `speech_output` воспроизводит синхронный или асинхронный TTS-ответ с отдельными delivery acknowledgements; `received` не подменяет `completed`.
- Server-side executors ограничены зарегистрированными typed handlers; погода read-only, а device actions повторно проходят authorization при срабатывании.
- Current search, voice и local RockCast fallback contracts не сломаны.
- Контракт, migrations, deterministic tests, WebSocket/integration tests и security/observability documentation обновлены; неподдерживаемые функции явно скрыты, а не имитируются.
