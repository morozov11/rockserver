# RM-011-G7 — read-only диагностика staging и план исправлений

Дата исследования: 2026-08-29  
Staging: `https://alex.vault57.ru`  
Режим: **только чтение; реализация, deploy, изменение БД и тестовых аккаунтов не выполнялись**

## Результат в одном абзаце

Публичный staging доступен и отдаёт web bundle, побайтно совпадающий с текущим
`C:\repos\rockserver\web\dist`. Текущие `master` всех трёх репозиториев содержат согласованный
pairing completion contract: сервер возвращает `202 pairing_pending`, RockCast и RockMobile
повторяют только `202`, а `401/409/410` считают terminal. Однако непосредственно предыдущие
RockServer/RockMobile ревизии использовали противоположную семантику: сервер сворачивал pending в
`401 pairing_rejected`, а Mobile повторял только `401`. Обе Android-сборки имеют неразличимую
версию `0.1.0` / `versionCode=1`. Поэтому наиболее сильное объяснение наблюдаемого немедленного
mobile rejection — **version skew установленного APK и staging completion API**, а не passkey UX.
Точный участник skew нельзя доказать без версии установленного APK и safe server request log;
публичный API не сообщает revision, Mobile теряет error `code/request_id`, ADB отсутствует, а
read-only SSH был остановлен из-за непроверенного host key. В текущем коде независимо подтверждены
ещё четыре реальные проблемы: смешение connected/pairing/error states в RockCast, неидемпотентный
web approval UI с одновременно видимыми terminal error и кнопкой, двойной product prefix в именах,
а также QR без гарантированной quiet zone в RockCast и с approval secret в URL query.

## Дополнение: разрешённый disposable flow 2026-08-29

После отдельного разрешения владельца задача 0 была начата на физическом `RMX5056` через ADB.
Получено более точное доказательство, которое **отвергает первоначальную гипотезу version skew для
этой конкретной попытки**:

| Поле evidence tuple | Наблюдение |
| --- | --- |
| Client build | установлен `com.rockmobile` `0.1.0`, `versionCode=1`, APK SHA-256 `f39a016a96cfdb422eeafd28c06b072c5e85a7dc385276fd5ab6c1ab2d204c12`; DEX содержит новую `202`-совместимую ветку и новый device-limit text |
| Server build | OCI revision запущенного контейнера `27beb6c1ebb730193e6786d440c495685354a2d7` |
| Protocol | текущая неверсированная completion semantics: pending=`202 pairing_pending` |
| Native result | после создания request Mobile оставался в `Waiting` несколько polling intervals, без unavailable/expired/generic failure |
| Web status/code/request_id | **не создавались**: нажатие `Подключить` остановлено локальным guard до `fetch`, поэтому HTTP status/code/request_id отсутствуют |
| Server lifecycle после click | request оставался `approved=false`, `consumed=false`, `revoked=false`, `expired=false`; account/device/session не создавались |

Скриншот и current web source дают точную первопричину. `accountName` одновременно хранит текст
поля «Имя аккаунта» и служит условием authenticated UI. После первой буквы `accountName` становится
truthy, выражение `preview && accountName` ошибочно показывает экран «Подключить … к аккаунту
«а»?», хотя passkey registration не запускалась и `csrf` пуст. Нажатие кнопки попадает в
`if (!preview || !csrf)`, устанавливает «Сначала войдите с passkey» и возвращается без API call.

Следовательно, это не expired request, не server rejection и не дефект установленного APK.
In-place update телефона намеренно не выполнялся: текущая APK уже использует новую protocol ветку,
а ошибка находится в развернутом RockServer web bundle. До исправления безопасный временный обход
для нового disposable account — перезагрузить исходную защищённую ссылку, **не вводить имя** и
нажать `Создать Rock-аккаунт`; сервер применит default account name. Это лишь smoke workaround, не
замена исправлению раздельных `registrationName` и `authenticatedAccountName`/exclusive state.

Владелец завершил этот workaround реальным passkey и approval. Read-only DB projection после
возврата показала `approved=true`, `consumed=true`, `revoked=false`, `expired=false`: server и
RockMobile успешно закончили pairing и создали native session. При этом browser сохранил карточку
pending request и enabled кнопку `Подключить`. Причина также детерминирована current source:
успешный approve (`204`) вызывает только `setMessage("Устройство подтверждено…")`; `preview` и
ветка `preview && accountName` не очищаются и отдельного success state нет.

Целевое post-success поведение обязательно:

1. Сразу после `204` страница необратимо переходит в exclusive `approved` state; QR, expiry,
   passkey/create controls и кнопка approve исчезают.
2. Крупный итог: `RockMobile — RMX5056 подключён к аккаунту «Rock account»` и пояснение
   `RockMobile завершает вход автоматически`.
3. Primary CTA на мобильном browser — `Вернуться в RockMobile` через проверенный Android App Link
   или custom scheme; fallback — явная инструкция вернуться жестом/переключателем приложений.
4. Secondary CTA — `Открыть аккаунт и устройства`, ведущая на `/` с текущей browser session.
5. Повторный tap/back/reload не должен снова показывать enabled approve: same-session повторное
   подтверждение трактуется идемпотентно или восстанавливает success/account-centre state.

## Безопасные границы исследования

- Не создавались pairing requests, WebAuthn challenges, browser sessions, users, devices или
  native sessions.
- Не выполнялись approve, complete, cancel, revoke, logout, rename, registration, authentication,
  migration, cleanup, deploy, commit или push.
- Не читались cookies, credential stores, browser history, passkeys, localStorage или БД.
- Bearer/refresh tokens, pairing proofs, approval secrets, private credentials и PII не
  запрашивались, не печатались и не сохранялись.
- QR реального пользовательского request не был получен: безопасной активной ссылки в контексте не
  было, а создание новой изменило бы staging DB. Проверены точная форма payload в исходниках и
  renderer implementation; никакое значение proof не извлекалось.
- Единственное изменение workspace этого исследования — данный новый диагностический файл.

## Проверенные revisions и состояние checkout

| Репозиторий | Branch / revision | Начальное состояние |
| --- | --- | --- |
| RockServer `C:\repos\rockserver` | `master` / `6ad37bd3c8312d27a51f93b289cba82460ad0ce0` | clean, совпадает с `origin/master` |
| RockCast `C:\repos\rockcast` | `master` / `0bd9639` | clean, совпадает с `origin/master` |
| RockMobile `C:\repos\rockmobile` | `master` / `fd1693d` | clean, совпадает с `origin/master` |

Pairing-state fixes находятся в RockServer `99c2783`, RockCast `0bd9639` и RockMobile `fd1693d`.
RockServer deployment log в репозитории утверждает успешный rollout образа
`sha256:bf80ed529b0a7c1ffc14c2333730851fdaddfadf401a90a1c03a6052e190cd3f`, но staging не имеет
публичного revision endpoint/header, поэтому это release evidence, а не независимое доказательство
исполняемого server binary.

## Факты, подтверждённые на staging

### Публичный browser flow

1. Чистая in-app browser session открыла `https://alex.vault57.ru/` с HTTP `200`.
2. Видимый экран: `Rock-аккаунт` → `Вы не вошли`, инструкция войти для просмотра устройств или
   подтверждения защищённой ссылки, одна кнопка `Войти с passkey`.
3. При viewport `390 × 844` страница не имеет горизонтального overflow (`scrollWidth=clientWidth=390`).
4. Browser console не содержала warning/error.
5. Passkey не запускался: это создало бы WebAuthn challenge/server state и потребовало бы реального
   пользовательского выбора credential.
6. Pairing-specific mobile page на staging не открывалась: без уже активного безопасного request
   её нельзя получить read-only. Подставлять или перебирать code/secret запрещено.

### Safe HTTP observations

| Request | Status | Безопасно наблюдаемый результат |
| --- | ---: | --- |
| `GET /` | 200 | `text/html`, title `RockServer` |
| `GET /health/live` | 200 | JSON `status=ok` |
| `GET /health/ready` | 200 | JSON `status=ok` |
| `GET /v1/devices` без credentials | 401 | structured error `authentication_required`, присутствует `request_id` |
| `GET /v1/auth/browser/session` | 404 | ожидаемо: актуальный route — POST `/v1/auth/browser-session` |

На публичных ответах подтверждены CSP `default-src 'self'`, `frame-ancestors 'none'`,
`form-action 'self'`, `object-src 'none'`, `img-src 'self' data:`, а также
`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer` и
`Permissions-Policy: camera=(), microphone=(), geolocation=()`.

### Доказательство фактического web deploy

Staging загрузил:

| Asset | Staging SHA-256 | Размер | Сверка |
| --- | --- | ---: | --- |
| `/assets/index-karVpDs9.js` | `8083efc774099c64de87e55852e6fe19a2d9eaa50b4ac74c2c5b58b3666b9fd6` | 26,579 bytes | точно совпадает с текущим `web/dist` |
| `/assets/index-sT0AFEmX.css` | `71a110621641e2ecb9f2d2f812d16d5440c62334162ae835835b184161f4675e` | 1,499 bytes | точно совпадает с текущим `web/dist` |

Это исключает старый **web bundle** как объяснение текущего landing screen. Оно не доказывает
revision server binary и тем более не доказывает revision установленного RockCast/RockMobile.

## Логи и наблюдаемая граница

- Staging server JSON logs документированно находятся в `/home/rockserver/logs`. Попытка
  read-only SSH с `BatchMode=yes` и `StrictHostKeyChecking=yes` остановилась до соединения: для
  IP отсутствовал доверенный ED25519 host key. Host key не принимался и `known_hosts` не менялся.
- На машине не запущен RockCast, `%LOCALAPPDATA%\RockCast\rockcast.log` отсутствует. Найденные в
  checkout `.rockserver-local.*.log` датированы 2026-08-17 и не относятся к account/pairing flow.
- `adb` отсутствует, Android device/emulator не подключён; logcat и установленный package metadata
  недоступны.
- Текущий RockCast/Mobile код не пишет безопасный pairing result diagnostic. Mobile exception
  сохраняет только HTTP status; server `code` и `request_id` отбрасываются. RockCast также сводит
  ответ к локальному enum без request ID.

Безопасный ручной шаг владельца для server logs: сверить SSH fingerprint с out-of-band записью VPS,
добавить ключ стандартным операторским способом и повторить только проекцию JSON полей
`timestamp`, `level`, `fields.message`, `fields.request_id`, `fields.endpoint` за узкое время
воспроизведения. Не выводить URI/query, headers, bodies, cookies или идентификаторы account/device.

## Статическая сверка contract и реализации

### Текущий согласованный completion contract

`POST /v1/pairing-requests/{request_id}/complete` с native desktop proof:

| State | Текущий RockServer | Текущий RockCast/RockMobile |
| --- | --- | --- |
| browser approval ещё не выполнен | `202 pairing_pending` | повторять до deadline |
| proof неверен | `401 pairing_rejected` | terminal |
| device limit | `409 device_limit_reached` | terminal, отдельный текст |
| expired/consumed/revoked | `410 pairing_expired` | terminal |
| approved active request | `200`, одноразовые native credentials | сохранить в OS-protected storage |

SQL completion выполняется в транзакции с `FOR UPDATE`, проверяет request state до device insert,
сериализует cap аккаунта, создаёт device/session/refresh row и ставит `consumed_at` атомарно.

### Детерминированная несовместимость соседних releases

- RockServer до `99c2783`: pending, invalid, expired, consumed и limit могли сходиться в
  `401 pairing_rejected` через `Ok(None)`.
- RockMobile до `fd1693d`: `shouldContinuePairing` повторял **только HTTP 401**.
- Текущий RockMobile `fd1693d`: повторяет **только HTTP 202**.
- Текущий RockCast `0bd9639`: повторяет `202`, прекращает `401/409/410`.
- Android `versionName=0.1.0`, `versionCode=1`; RockCast crate version `0.1.0`. По UI/файлу нельзя
  понять, до или после protocol fix собрана установленная программа.

Следствия:

1. Старый Mobile + новый server получает `202`, не считает его pending и немедленно завершает flow.
2. Новый Mobile + старый server получает `401`, считает его terminal rejection.
3. Только согласованные пары old/old или new/new способны дождаться approval; old/old при этом
   небезопасно повторяет также настоящий invalid proof.

Staging web bundle и release record указывают на новый server release, поэтому основной рабочий
диагноз — **на телефоне остался APK до `fd1693d`**, несмотря на одинаковое отображаемое `0.1.0`.
Это остаётся обоснованным диагнозом с явно указанной границей, а не доказанным фактом конкретного
устройства. Для окончательного установления нужны package build identity и один safe
`status/code/request_id` из той же попытки.

## Подтверждённые UX/root-cause дефекты текущего кода

### RockCast mixed state

`draw_account_window` не имеет единого state machine:

- блок `Connect this PC` показывается всегда, когда нет текущего in-memory pairing, даже если
  `account_profile` уже загружен;
- `Refresh account & devices` показывается одновременно с connect/pairing/connected content;
- при refresh failure старые `account_profile/account_devices` не очищаются, а generic сообщение
  `Account is unavailable or the session has expired` добавляется рядом с визуально подключённым PC;
- connect, pairing, status, QR, refresh, device list и error способны сосуществовать.

Это точно объясняет наблюдаемый экран RockCast и не требует серверной ошибки.

### Двойной product prefix

RockCast default отправляет `device_display_name = "RockCast — <hostname>"`, RockMobile —
`"RockMobile — <model>"`. Затем RockCast, RockMobile и web account centre снова рендерят
`productName(device_type) + " — " + device_display_name`. Поэтому текущая модель закономерно даёт
`RockCast — RockCast — DESKTOP-…` и `RockMobile — RockMobile — RMX5056`.

Первопричина — не данные пользователя, а отсутствующий invariant контракта: OpenAPI ограничивает
длину `device_display_name`, но не определяет, включает ли он product name.

### Web terminal error рядом с активной кнопкой

Web хранит независимые `preview`, `message`, `accountName` и `busy`. Если `approve` возвращает
`pairing_not_approvable`, код устанавливает terminal-looking message, но не очищает `preview` и не
переводит экран в terminal state. Условие `preview && accountName` продолжает рисовать кнопку
`Подключить`. Серверный `409 pairing_not_approvable` в свою очередь не различает already-approved,
expired, consumed, revoked, stale reauthentication и wrong proof для UI.

Это точно объясняет противоречивый мобильный browser screen после повторного/гоняющегося approval.

### QR payload и rendering

Фактическая форма в обоих native clients:

```text
https://alex.vault57.ru/?code=<8-hex-short-code>&secret=<opaque-approval-secret>
```

В QR нет bearer или refresh token. В нём есть короткоживущий approval secret; поэтому QR и полная
ссылка являются credential material до expiry. Реальное значение не извлекалось.

- RockCast рисует матрицу в `180 × 180` logical px, начиная с края матрицы; явной quiet zone нет,
  размер module дробный, добавляется `+0.5` overlap. Это ухудшает сканирование и может искажать края.
- RockMobile просит ZXing построить `360 × 360`, затем показывает raster как `180dp`. ZXing margin
  не закреплён hint/test, а последующее масштабирование зависит от density и может размывать modules.
- `Referrer-Policy: no-referrer` подтверждён и защищает последующие referrer, но не скрывает query
  исходного navigation от browser history и возможного edge access log. Approval secret не должен
  находиться в query.
- Тексты не дают короткой нумерованной инструкции «1. Сканируйте; 2. войдите; 3. сравните фразу;
  4. нажмите Подключить; 5. вернитесь в приложение».

## Таблица проблем и приоритетов

| Priority | Проблема | Владелец | Первопричина | Доказательство |
| --- | --- | --- | --- | --- |
| **P0** | RockMobile не завершает pairing при release skew | RockServer + RockMobile + release | pending contract сменился `401 → 202`, но нет protocol negotiation и уникальной build identity | diff соседних commits; обе APK версии `0.1.0/1`; current/new contract совместим статически |
| **P1** | Нельзя точно диагностировать live rejection | RockServer + RockCast + RockMobile | staging не публикует revision; clients теряют error `code/request_id`; pairing logs отсутствуют | public health не содержит revision; client transport source |
| **P1** | Web показывает terminal error и доступный approve одновременно | RockServer web + API | независимые state fields; approve failure не инвалидирует preview; `pairing_not_approvable` сворачивает разные причины | current `web/src/app.tsx`, approve handler/API outcome |
| **P1** | RockCast одновременно показывает connect/pairing/connected/error/refresh | RockCast | UI строится условными блоками без exclusive state; stale profile остаётся после failure | current `src/app/ui/account.rs` |
| **P1** | Двойные `RockCast/RockMobile` prefixes | все три UI, contract owner RockServer | product включён и в `device_display_name`, и в presentation | current defaults + render helpers; пользовательские строки воспроизводятся напрямую |
| **P1** | QR secret находится в query | RockServer web + оба клиента | deep link contract использует `?secret=` | current `deep_link/browserLink`; security headers защищают только referrer |
| **P1** | RockCast QR мал и без quiet zone | RockCast | fixed 180px, matrix-to-edge, fractional modules | current `draw_qr` |
| **P2** | RockMobile QR raster scaling не закреплён | RockMobile | 360px bitmap показывается как 180dp, margin/error correction не специфицированы | current `QrCode` |
| **P2** | Инструкции и terminal states не образуют один понятный flow | все UI | тексты описывают детали, но не следующий обязательный шаг и recovery | фактические UI strings и browser flow |
| **P2** | Cancel native не отзывает server request | RockServer + clients | client прекращает polling, request остаётся pending до expiry | current cancel paths не вызывают API |

## Целевой пользовательский сценарий

### Один аккаунт и первое устройство

1. Пользователь в RockCast или RockMobile нажимает одну primary action: `Подключить это устройство`.
2. Экран переходит из `Disconnected` в единственное состояние `Waiting for browser approval`.
3. Показываются только имя целевого устройства, срок/таймер, QR, fallback actions и фраза проверки.
4. Browser по защищённой ссылке показывает exact target. Если account ещё нет, отдельно предлагает
   `Создать аккаунт`; если есть — `Войти с passkey`.
5. После успешной ceremony browser показывает account + target и требует явный approve.
6. Native client получает success, сохраняет credentials, закрывает pairing UI и показывает один
   connected summary. Connect CTA и старые errors исчезают.

### Добавление desktop

1. В уже существующем account RockCast создаёт новый request с raw label `DESKTOP-685GRAQ`, а
   product передаёт отдельно как `rockcast_windows`.
2. QR сканируется телефоном; browser говорит `Подключить RockCast — DESKTOP-685GRAQ к «…»?`.
3. Фраза в browser и RockCast совпадает; один approve; desktop получает credentials.
4. Account centre показывает desktop ровно один раз, помечает `Этот ПК`, refresh не открывает CTA.

### Добавление mobile

1. RockMobile raw label — `RMX5056`, type — `rockmobile_android`, protocol/build version явны.
2. Ссылка может открыться на том же телефоне или быть отсканирована другим устройством.
3. После approve Mobile продолжает polling только для documented pending state, получает `200`,
   сохраняет credentials и показывает `Этот телефон`.
4. Desktop, mobile и browser видят один account и два различных named devices.

### Отмена, истечение и повтор

- **Cancel до approve:** native отправляет idempotent cancel с native request proof; browser при
  refresh получает terminal `cancelled`; ни одна session/device не создаётся.
- **Expiry:** native таймер и server clock приводят к `410 pairing_expired`; approve скрыт; primary
  recovery — `Создать новую ссылку`.
- **Повторный approve той же browser session:** idempotent success/`already_approved`, без красной
  ошибки и без второй device row.
- **Approve другим account или wrong/stale proof:** neutral terminal error без раскрытия owner/state.
- **Completion уже consumed:** `410`, credentials повторно не выдаются.
- **Устройство уже connected:** connected UI не предлагает connect. Повторное pairing после
  явного local credential reset создаёт новую device identity и требует отдельного подтверждения;
  account centre предлагает отозвать старую запись.

## План работ маленькими задачами и зависимости

### 0. Сначала снять однозначное evidence текущего rejection (без исправлений)

**Зависимостей нет.**

1. Владелец сверяет VPS SSH fingerprint out-of-band.
2. На физическом телефоне записывает только package `versionName`, `versionCode` и SHA-256 APK;
   не делает backup app data и не извлекает Keystore.
3. Устанавливает узкое окно времени, создаёт один новый disposable pairing request и записывает
   только `HTTP status`, server `code`, `request_id`, client build ID и переход UI state.
4. Из server JSON logs выбирается тот же `request_id`, только safe event name/status.
5. Матрица решения: `202` + старый client = stale APK; `401 pairing_rejected` до approval = old
   server или proof mismatch; `409` = cap; `410` = lifecycle; `5xx` = service/storage.

Эта задача требует отдельного разрешения на staging mutation и реального устройства; в рамках
данного исследования она не выполнялась.

### 1. Release/protocol observability (P0 prerequisite)

**RockServer + оба клиента.**

- Ввести `pairing_protocol_version` в create response/request contract и явную policy совместимости.
- Публиковать safe server build revision/API contract version в health response или response header.
- Увеличивать RockMobile `versionCode` на каждую distributable build; обновить `versionName` минимум
  до `0.1.1`. RockCast также получает display build/version distinct от старого `0.1.0`.
- `app_version` включает semantic version + short build revision, но не hostname/user data.
- При unsupported protocol возвращать `426 client_upgrade_required`, не generic rejection.
- На ограниченный rollout window либо поддержать legacy pending semantics по negotiated version,
  либо явно блокировать legacy client до создания request. Не угадывать version по User-Agent.

### 2. Сначала server/API lifecycle, затем web и native clients

**Зависит от 1.**

- Зафиксировать canonical states: `pending`, `approved`, `consumed`, `cancelled`, `expired`.
- Сохранить current completion `202/200/409/410/401`, добавить stable JSON error `code` во все ветки.
- Сделать повторный approve той же account/browser ceremony идемпотентным; terminal outcomes
  различать достаточно для UX, не раскрывая owner постороннему proof.
- Добавить native-proof idempotent cancel endpoint, например
  `POST /v1/pairing-requests/{id}/cancel` с `desktop_token` в JSON body и `204` для уже cancelled.
- Обновить `api/openapi.yaml` и contract tests до реализации клиентов.

### 3. Исправить текущий rejected Mobile flow

**Зависит от 1–2 и server-first deploy.**

- Transport parses structured `ApiError(code, request_id, status)` вместо исключения только со status.
- Polling повторяет исключительно `pairing_pending`; status без ожидаемого code считается protocol
  error и показывает `Обновите RockMobile`, а не `запрос отвергнут`.
- `401 pairing_rejected`, `409 device_limit_reached`, `410 pairing_expired`, `426` и `5xx` имеют
  отдельные сообщения/recovery.
- Сохранение credentials остаётся после проверки pending identity/cancel flag.
- Выпустить новый APK с новым versionCode; выполнять in-place upgrade (`adb install -r`/обычный
  update), не удалять app data существующего пользователя без отдельного решения.
- Повторить один flow на staging и сопоставить request ID. Если согласованные builds всё ещё дают
  `401`, расследовать сериализацию `desktop_token/request_id` в памяти, не печатая значения.

### 4. Exclusive RockCast account state machine

**Может идти параллельно с 3 после contract freeze.**

- Один enum/view model: `Disconnected`, `CreatingRequest`, `Waiting`, `Connected`,
  `RecoverableError`, `TerminalError`.
- В `Connected` нет connect form/QR; есть account summary, device list, refresh/logout.
- Refresh failure не утверждает expiry без `401`; network/5xx сохраняет connected profile с
  non-blocking banner. `401` очищает stale profile после подтверждённого refresh failure.
- `Waiting` содержит один cancel, таймер и QR; закрытие окна отменяет local job и вызывает server
  cancel best-effort.
- Разделить profile и device-list failures, чтобы успешная session не выглядела отключённой.

### 5. Web pairing state machine и тексты

**Зависит от 2.**

- Вместо четырёх независимых state fields использовать discriminated state:
  `loading`, `pending-anonymous`, `pending-authenticated`, `approving`, `approved`, `terminal`,
  `unavailable`.
- При terminal approve error preview/button скрываются. При success экран фиксируется на
  `Подключено; вернитесь в приложение`; повторный click невозможен.
- Не показывать общий текст «истёк/завершён/уже подключено» при доступном approve.
- Показать один numbered next-step list и один primary action на каждом экране.
- Fresh passkey requirement объяснять перед ceremony; stale reauth предлагает повторный вход, а не
  новую pairing ссылку.

### 6. Нормализовать device display name без destructive migration

**Contract уточняется в 2; клиенты меняются вместе с 3–4.**

- OpenAPI invariant: `device_display_name` — пользовательская/raw machine label без product prefix;
  product выводится только из `device_type`.
- Новые defaults: `DESKTOP-685GRAQ` и `RMX5056`.
- Общий presentation helper добавляет product ровно один раз. Для legacy rows, уже начинающихся с
  canonical `RockCast — ` / `RockMobile — `, helper не добавляет второй prefix.
- Не выполнять массовую staging migration. Пользователь может переименовать legacy device через
  account centre; отдельная migration возможна только после preview и явного разрешения.

### 7. Безопасный QR/link contract и UI

**Зависит от 2; server web должен быть развернут раньше клиентов с новым link format.**

- Перенести approval secret из query в fragment, например
  `https://alex.vault57.ru/?code=<code>#secret=<proof>`; fragment не отправляется edge/server.
- Web валидирует fragment в памяти и немедленно делает `history.replaceState` на URL без proof.
- Запретить analytics/third-party assets на pairing page; сохранить CSP/no-referrer.
- QR остаётся одноразовым credential до expiry; не логировать/capture его в telemetry/screenshots.
- Старый query format поддерживать только короткое rollout window и никогда не логировать query.

### 8. Release order

1. Backward-compatible RockServer observability/lifecycle + OpenAPI.
2. Deploy server и подтвердить revision/readiness.
3. Deploy web state machine и fragment parser с old/new link compatibility.
4. Выпустить RockMobile/RockCast с distinct versions, protocol version, raw names и новым QR.
5. Провести один disposable staging E2E.
6. После adoption удалить legacy pending/query compatibility отдельной задачей.

## Безопасное предложение QR UI

- Target rendered side: **320 logical px/dp** на обычном экране, адаптивно не меньше **256**.
- Quiet zone: **ровно минимум 4 светлых modules со всех сторон**, включённая в итоговый размер.
- Масштаб: вычислять integer pixels/module по physical scale; не растягивать готовый raster
  нецелым коэффициентом. Minimum 6 physical pixels/module для тестовых устройств.
- Цвета: непрозрачный `#000000` на `#FFFFFF`, без градиента, rounded modules или overlay;
  фактический contrast 21:1.
- Error correction: явно задать не ниже `M` и покрыть тестами ожидаемый matrix/quiet-zone dimension.
- Над QR: `Подключить RockCast — DESKTOP-…` / `RockMobile — RMX…`.
- Под QR: `1. Откройте камеру; 2. войдите с passkey; 3. сравните фразу; 4. нажмите Подключить`.
- Отдельно видны verification phrase, short code и countdown `Ссылка действует ещё 04:32`.
- Fallback actions: `Открыть защищённую ссылку` и `Копировать защищённую ссылку`; после copy —
  `Ссылка скопирована. Не пересылайте её: она даёт право подтвердить это устройство до …`.
- Accessibility: meaningful accessible name с target и expiry, keyboard focus, screen-reader
  инструкция; QR image не является единственным способом продолжить.
- Ни UI, ни clipboard confirmation не показывают approval secret отдельно. Short code — только для
  визуального сопоставления, не замена proof.

## API/contract changes

Минимальный предлагаемый набор:

1. Уточнить schema description/invariant `device_display_name`.
2. Добавить `pairing_protocol_version` в create request/response и документировать version policy.
3. Добавить `426 client_upgrade_required` для unsupported protocol.
4. Стабилизировать completion codes `pairing_pending`, `pairing_rejected`,
   `device_limit_reached`, `pairing_expired`.
5. Добавить idempotent cancel endpoint с native proof.
6. Сделать approve outcome идемпотентным для того же account/request либо вернуть отдельный safe
   `already_approved` success-like outcome.
7. Добавить safe build/API revision в health/header без environment, hostname или credential data.
8. Описать fragment-based browser handoff вне server query schema и redaction requirements.

## Тестовый план

### RockServer unit/persistence

- State table для pending/approved/consumed/cancelled/expired/wrong proof.
- Concurrent double completion выдаёт credentials ровно один раз.
- Concurrent approve одного account идемпотентен; другой account/proof не получает state leak.
- Device cap возвращает только `409 device_limit_reached`.
- Cancel до/после approve, повторный cancel, cancel после consume.
- Protocol v1/v2 transition и `426` после удаления compatibility.
- Ни error/audit log, ни request span не содержат URL query/proofs/tokens.

### OpenAPI/contract

- Все statuses и `ErrorResponse(code,message,request_id,details)` структурно проверяются.
- `device_display_name` invariant и protocol field обязательны/совместимы согласно rollout phase.
- Public revision field безопасен и соответствует OCI/build metadata.

### RockCast unit/UI

- Exclusive state snapshot tests: connected screen не содержит connect/QR/error-expired.
- Refresh 401, 5xx, network failure и partial device-list failure различаются.
- Raw и legacy-prefixed names рендерятся с одним product prefix.
- QR golden tests: 4-module quiet zone, integer modules, 256–320 size, exact fragment payload shape
  с синтетическим proof; output/log snapshot не содержит proof.
- Cancel/close race не сохраняет credentials после cancellation.

### RockMobile unit/instrumentation

- `202 pairing_pending` повторяется; `401/409/410/426/5xx` дают разные terminal/recovery states.
- Parser сохраняет `code/request_id`, но не body/proof/token в log.
- Lifecycle resume/cancel и late completion не сохраняют credentials после cancel.
- QR size/margin/accessibility semantics проверяются Compose tests.
- Raw/legacy names не дублируют product.
- Package versionCode monotonic; build ID видим на diagnostic screen.

### Web unit/browser E2E

- Для каждого discriminated state есть отдельный snapshot.
- Terminal message никогда не сосуществует с enabled approve.
- Double click/retry approve даёт один success state.
- Fragment proof считывается один раз, удаляется из address bar/history state и не появляется в
  network requests, console, DOM text или referrer.
- Mobile viewport 360/390/430px, keyboard navigation, screen reader names, expired countdown.
- Synthetic deterministic harness покрывает pending → login → approve → success и все terminal states.

### Manual staging E2E

Только после отдельного разрешения на disposable account/request:

1. Записать server/web/RockCast/RockMobile build identities и clock.
2. Создать один account/passkey или использовать явно утверждённый disposable account.
3. Подключить первый device, затем desktop, затем physical Android.
4. Проверить один prefix, current-device markers, restart/session restore.
5. Отдельными requests проверить cancel, expiry, повторный approve, повторный complete, network
   outage и device cap.
6. Для каждой попытки хранить только timestamp, build IDs, HTTP status/error code/request_id и
   результат; удалить/не сохранять QR/link/proofs/tokens.

## Критерии приёмки

- Fresh supported RockMobile и RockCast завершают один staging pairing каждый; pending не выглядит
  rejection и success достигается до expiry.
- Unsupported/stale client получает явное `Обновите приложение`, а не generic rejected.
- На каждом экране ровно один lifecycle state и одна primary action.
- Connected RockCast не показывает `Connect this PC`, QR или `session expired` без подтверждённого
  terminal auth response.
- Browser не показывает enabled approve вместе с expired/already-finished error.
- Имена во всех трёх surfaces имеют ровно один product prefix.
- QR сканируется минимум двумя физическими камерами при 100% и 125/150% Windows scale и на Android
  low/high density; quiet zone и contrast проходят automated assertions.
- Approval secret отсутствует в request URL, referrer, access/app logs, console и address bar после
  initial fragment parse.
- Cancel/expiry/retry/already-consumed/device-limit имеют разные понятные outcomes и не создают
  лишних devices/sessions.
- Safe diagnostics однозначно связывают client build, server build, protocol version, status,
  error code и request ID без credentials/PII.
- Все repository quality gates проходят; public behavior changes сопровождаются OpenAPI и tests.

## Что нельзя проверить без пользователя/реального passkey/устройства

- Revision и package hash фактически установленного RockMobile/RockCast.
- Реальный WebAuthn/passkey selection, UV prompt, cancellation и двухминутная fresh-auth граница.
- Содержимое конкретного пользовательского QR и его camera scan. Проверена только contract shape;
  реальный proof намеренно не получался.
- Фактический HTTP status/code/request_id отклонённой попытки и соответствующий server event.
- Protected staging logs до out-of-band проверки SSH host fingerprint.
- Android Keystore/Windows credential store persistence после restart.
- Физическая читаемость QR различными камерами, display scaling и освещением.
- Полный account/device list пользователя и наличие device cap; это PII/account state.
- Реальные cancel/expiry/replay/race transitions на PostgreSQL staging без разрешённого disposable flow.

## Рекомендуемый следующий шаг

Не начинать UX refactor вслепую. Сначала выполнить задачу 0 в отдельном разрешённом disposable
окне и доказать одну отклонённую попытку через `(client build, server build, protocol, status,
code, request_id)`. Если основной диагноз подтвердится, немедленный recovery — in-place update
RockMobile до distinct build на базе `fd1693d` или новее и повтор flow; постоянное исправление —
задачи 1–8 в указанном server-first порядке.
