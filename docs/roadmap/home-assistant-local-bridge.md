# План интеграции локального Home Assistant с RockServer

Статус: исходный проектный план, 2026-11-12. Владелец согласовал замену отдельного домашнего агента на Home Assistant custom integration. Актуальные шаги запуска и проверенное состояние реализации записаны в `docs/home-assistant-bridge.md`, `docs/status.md` и `docs/ha-bridge-handoff.md`. Этот документ уточняет DC-021/DC-022 и погодную часть DC-030 для Home Assistant, который доступен только в домашней сети. Работы с ESP32 и другими клиентами сюда не входят.

## Цель и границы

Первый результат: RockServer получает от выбранного владельцем экземпляра Home Assistant актуальные показания разрешённых датчиков, текущую погоду и краткий прогноз. Данные доступны через серверную нормализованную модель и будущий read-only weather provider. Отсутствие связи и устаревшие значения видны явно. На домашнем роутере не открываются входящие порты, адрес HA и его токен не передаются в RockServer.

Первая версия работает только на чтение. Управление светом, реле, климатом и произвольные HA service calls относятся к отдельной DC-023 и не входят в эту работу. Не добавлять клиентский экран, голосовой ответ, TTS, ESP32 firmware или общий scheduler в рамках интеграции.

## Выбранная топология

```text
HA (локальный HTTP/WebSocket API)
          ↕ только домашняя сеть
домашний агент RockServer HA Bridge
          │ исходящее TLS/WSS, переподключение
          ▼
RockServer: integration ingress → нормализация → entity state / weather cache
```

Агент запускается на постоянно работающем узле в той же сети, что и HA: отдельный контейнер или системная служба. Установка как HA App не обязательна. HA остаётся доступным только локально. Агент инициирует оба соединения: к HA по локальному адресу и к публичному RockServer по WSS. Reverse proxy, проброс портов, публичный HA URL и VPN для штатной работы не нужны. Если нет доступного домашнего узла для агента, это единственная инфраструктурная предпосылка, которую нужно решить до реализации.

Прежний DC-021 описывал server-side adapter с конфигурацией HA endpoint в RockServer. Для закрытой сети provider boundary остаётся на сервере, но HA transport выполняется локальным агентом. Сервер никогда не делает исходящий запрос на частный HA URL. `GET /api/v1/devices/connect` принимает только paired native-device session; агенту нужен отдельный integration endpoint и identity, а не притворная регистрация устройства.

## Доверие и настройка

1. Владелец создаёт в HA отдельного пользователя интеграции и его токен; локальный агент хранит токен в защищённом конфигурационном секрете. HA API авторизует этого пользователя; не предполагать, что long-lived token сам по себе ограничивает доступ до отдельных entities. Ограничение публикуемых данных обеспечить allowlist в агенте и повторной проверкой на RockServer.
2. RockServer создаёт привязанную к одному `user_id` запись интеграции и отдельный отзывной credential только для ingest/health этой интеграции. Начальный bootstrap может быть операторской конфигурацией без нового UI; секрет показывается один раз и не попадает в git, логи или URL. Ротация и отзыв обязательны до реального подключения.
3. В конфигурации агента указываются локальный HA URL, ссылка на HA token, публичный WSS URL RockServer, credential интеграции и список разрешённых `entity_id`. Погодная entity (`weather.*`) выбирается явно; при нескольких кандидатах автоназначения нет.
4. RockServer хранит отдельный server-side allowlist. При первом подключении агент присылает preview метаданных; владелец/оператор утверждает конкретные entities. До утверждения значения не публикуются в общей directory. При несовпадении локального и серверного списков действует их пересечение.

Не передавать HA token, локальный URL, native attributes и сырые events в публичный device-control протокол. Внешний WSS обязан проверять сертификат и имя сервера. Локальный TLS к HA использовать, если он настроен; для HTTP соединение ограничено доверенной домашней сетью. Не отключать проверку TLS ради локального самоподписанного сертификата.

## Минимальный контракт agent ↔ RockServer

До runtime зафиксировать endpoint, auth и схемы в `api/openapi.yaml`; добавить положительные и отрицательные fixtures. Сохранить стиль существующего protocol v1 envelope, но выделить новый endpoint, например `/api/v1/integrations/home-assistant/connect`. Имена ниже проектные и утверждаются в OpenAPI, а не считаются уже реализованными:

- `integration.hello`/`integration.welcome`: версия, integration ID из credential, server time, лимиты и heartbeat. Клиент не может заявить чужой `user_id` или integration ID.
- `integration.snapshot`: полный список только allowlisted metadata и их state после auth, reconnect или смены allowlist; revision/epoch для идемпотентного resync. Отсутствующие ранее опубликованные entities переводятся в unavailable/tombstone после полного snapshot.
- `integration.state`: ограниченный batch изменений с монотонной revision на соединение, HA `last_changed`/`last_updated` как observed time и server received time. Дубликаты и старые revisions игнорируются.
- `integration.weather`: текущие условия и отдельно прогноз с типом периода, временем получения, сроком свежести и единицами; размер и число периодов ограничены. Отсутствующие поля остаются отсутствующими, а не заполняются нулями.
- `integration.heartbeat` и `integration.error`: health HA/API/forecast, без секретов и сырых response bodies. Сервер подтверждает принятый revision; после неопределённого результата агент шлёт полный snapshot.

Применить существующие ограничения на JSON UTF-8, размер кадра и rate limits либо задать более узкие. Проверять schema, owner, integration status и allowlist на каждом сообщении. Не принимать service-call команды с этого канала в первой версии. Сервер не должен доверять присланным `freshness` и `received_at`: вычисляет их сам. Нужны deterministic duplicate/reorder/reconnect tests.

## Получение данных из HA

- На старте агент проверяет auth, получает текущие states и список выбранных entities через HA API. Для изменений подписывается на `state_changed` через локальный `/api/websocket`; после разрыва снова читает полный snapshot перед отправкой дельт. Не считать подписку журналом с гарантированным replay.
- Переносить только разрешённые `sensor.*` и выбранную `weather.*`. Нормализовать числовые значения, единицы, device class, quality, observed time и availability. `unknown`, `unavailable`, нечисловой sensor value, неподдерживаемая единица и удалённая entity не превращаются в правдоподобное число.
- Текущие условия брать из выбранной `weather.*`. Прогноз получать через HA action `weather.get_forecasts` с выбранным типом (`daily` для MVP), поскольку прогноз не обязан быть атрибутом состояния weather entity. Агент запрашивает его при старте и затем с ограниченной частотой, например раз в 15 минут; конкретный интервал и допустимый TTL утверждаются вместе с выбранным HA weather provider. Ошибка обновления не стирает последний прогноз немедленно, но делает его stale после TTL.
- Использовать явную timezone/location настройки владельца или HA weather entity, не угадывать местоположение по IP сервера. На сервер передаются готовые величины и временные метки, а не HA credential или произвольная геолокация. Температура, осадки и ветер сохраняют исходные единицы с явной конвертацией в нормализованную модель.

## Серверная модель и чтение

Повторно использовать доменные `EntityDirectoryEntry`/`EntityStateSnapshot`, owner boundary, state store и проверки freshness, где они уже реализованы. HA native `entity_id` хранить только в integration mapping; публичный ID должен быть стабилен и привязан к integration ID, чтобы одинаковые имена разных HA не сливались. Удалённые entities tombstone, а не назначаются другой native entity. Состояние HA не создаёт paired device и не получает `actuator` capability.

Weather provider для DC-030 получает нормализованные current/forecast из последнего принятого snapshot конкретного владельца и выбранной weather entity. Сначала реализовать серверный read API/доменный query с явным fresh/stale/unavailable; text/display/speech presentation оставить для соответствующих задач DC-030 и клиентских этапов. Не подменять недоступный HA данными другого пользователя или внешнего weather API. Если свежих данных нет, вернуть структурированную unavailable ошибку либо явно помеченный stale snapshot по принятому контракту.

## Сбои и эксплуатация

- Домашний интернет/HA недоступны: агент переподключается с bounded exponential backoff и jitter, RockServer переводит integration offline по heartbeat TTL, значения становятся stale по собственным TTL. После восстановления полный snapshot, затем дельты.
- Недействительный HA token: health `auth_failed`, без бесконечного быстрого retry и без утечки ответа HA. Недействительный RockServer credential: соединение отвергается до upgrade; состояние интеграции видно оператору; ротация через отдельный путь.
- Перезапуск/дубликат агента: одна активная сессия на integration ID, новый валидный connection заменяет старый. Epoch и revisions предотвращают перезапись новых значений запоздавшими кадрами.
- Рост очереди при потере сети: сохранять только последнее состояние на entity и последний прогноз в ограниченной памяти/диске, затем resync. Не копить бесконечный backlog всех HA events.
- Метрики: last HA success, last RockServer ingest, last forecast refresh, число stale/unavailable entities, reconnects, rejected frames и lag. Логи без токенов, локальных адресов, полного HA payload и персональных значений.

## Этапы реализации и приёмка

1. **Контракт и identity (RockServer).** Уточнить DC-021: новый integration endpoint, owner-bound credential, auth/revoke/rotation, OpenAPI/fixtures, state size/rate limits. Проверить, что native device credential не проходит на integration endpoint и наоборот; отключённая интеграция не влияет на startup и существующие device/voice маршруты.
2. **Локальный агент (отдельный модуль/пакет в RockServer, без HA custom integration).** Конфигурация secret references, локальный HA API client, snapshot + `state_changed`, outbound WSS, reconnect и безопасная диагностика. Fake HA и fake RockServer покрывают auth failure, большие events, reconnect и повторную доставку; тесты не выходят в сеть.
3. **Read-only entities (DC-022).** Preview/allowlist, owner mapping, нормализация и ingestion, full resync, revisions, tombstones, freshness. Проверить две интеграции с одинаковыми native IDs у разных владельцев, unavailable, stale, удаление и восстановление.
4. **Погода (HA-источник DC-030).** Явный выбор одной `weather.*`, current и `weather.get_forecasts`, нормализованные DTO, TTL/cache и read query. Проверить daily forecast, пустой/частичный ответ, разные единицы, ошибку action и смену выбранной entity. Клиентскую презентацию и голос отложить до их собственных этапов.
5. **Пилот в домашней сети.** На реальном HA выбрать разрешённые entities, запустить агент без входящих портов, проверить cold start, полный resync, обновление state, прогноз, перезапуск HA/агента/RockServer, отключение интернета, отзыв обоих credential и отсутствие секретов в логах. Деплой RockServer — отдельное действие после обычных проверок и решения владельца.

Для каждой кодовой задачи обновлять `docs/status.md` и `docs/tasks.md`, последовательно запускать обязательные проверки из `AGENTS.md`. Этот документ фиксирует план, а не объявляет какой-либо этап реализованным.

## Зависимости и открытые решения перед пилотом

- Где физически будет работать домашний агент и как будут храниться его секреты (контейнер, служба, HA App позже).
- Какие именно HA `entity_id` разрешить и какая `weather.*` является источником погоды; выбрать тип прогноза и TTL с учётом частоты обновления конкретной HA интеграции.
- Нужен ли в MVP ручной операторский provisioning либо сразу пользовательский UI. Без безопасной выдачи/отзыва credential подключать реальный HA нельзя.
- Согласовать контракт weather DTO с DC-030 и существующими entity schemas до написания agent/server runtime.

## Источники

- Локальные: `docs/roadmap/device-control-rockcast-esp32.md` (Home Assistant adapter, Phase 7 и Phase 9), `docs/roadmap/device-control-tasks.md` (DC-021, DC-022, DC-030), `api/openapi.yaml` (native device channel и нормализованные entity schemas).
- Home Assistant: [WebSocket API](https://contributors.home-assistant.io/core/internals/websocket-api/), [REST API](https://contributors.home-assistant.io/core/internals/rest-api/), [weather.get_forecasts](https://www.home-assistant.io/actions/weather.get_forecasts/).
