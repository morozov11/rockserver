# RM-011 — план исправления bugs и UX/UI RockMobile, RockCast и web-кабинета

Дата: 2026-08-29  
Основание: read-only staging-исследование и один разрешённый disposable physical-device flow  
Статус: **план; реализация, commit, deploy и изменение account/device data не выполнялись**

## 1. Цель

Пользователь должен за один очевидный flow:

1. понять, подключено ли текущее устройство;
2. при необходимости начать pairing одной кнопкой;
3. пройти passkey/browser approval без ложных переходов;
4. увидеть однозначный success в browser и native app;
5. открыть список устройств или вернуться в приложение;
6. получить конкретное действие при expiry, cancellation, offline, invalid session и device limit.

RockMobile, RockCast и web используют одинаковые продуктовые состояния и тексты, но реализуют их
в существующем Compose/egui/Preact коде. Общая cross-repository UI library не создаётся.

## 2. Подтверждённые факты и bugs

### P0 — browser account creation ломается после первой буквы

`web/src/app.tsx` использует один `accountName` и для editable registration input, и как признак
authenticated browser account. После первой буквы `preview && accountName` становится true, UI
самовольно показывает approve screen. `csrf` при этом пуст; кнопка останавливается локальным guard
и пишет «Сначала войдите с passkey», не вызывая API.

### P0 — после успешного подключения browser остаётся в pending UI

Физический flow завершился успешно: staging DB показала `approved=true`, `consumed=true`,
`expired=false`. Но успешный approve только вызывает `setMessage`; `preview` не очищается и
отдельного success state нет. Карточка, expiry и enabled `Подключить` остаются на экране.

### P1 — RockCast смешивает взаимоисключающие состояния

Connect form, pairing QR, refresh, cached connected profile, device list и generic error рисуются
независимыми блоками. Поэтому подключённый PC может одновременно видеть `Connect this PC` и
`Account is unavailable or the session has expired`.

### P1 — product name дублируется

Клиенты отправляют `RockMobile — RMX5056` / `RockCast — DESKTOP-…` как `device_display_name`, а
все UI ещё раз добавляют product из `device_type`. Результат — двойной prefix.

### P1 — QR и handoff не соответствуют реальному устройству

- RockCast: QR `180×180`, без явной quiet zone и с fractional modules.
- RockMobile: bitmap 360px масштабируется до 180dp; margin/error correction не зафиксированы.
- На том же телефоне QR занимает главный экран, хотя primary path — открыть ссылку.
- После browser success нет рабочего «Вернуться в RockMobile».
- Approval secret находится в URL query, а не fragment.

### P1 — ошибки теряют причину

RockMobile хранит только HTTP status, RockCast сводит ответ к широким enums. Server `code` и
`request_id` теряются, поэтому UI смешивает invalid proof, expired, already-used и unavailable.

### P2 — видимый product и язык непоследовательны

На главном экране Android показан заголовок `RockCast`, хотя package — RockMobile. Account UI
RockCast целиком английский, несмотря на существующий `Lang::Ru/Lang::En`.

## 3. Принципы исправления

- На одном экране существует только одно lifecycle state.
- Одна primary action на state; вторичные действия визуально слабее.
- Success — отдельное состояние, а не строка поверх старого экрана.
- `device_display_name` содержит raw label, product берётся из `device_type`.
- Network failure не равен expired session.
- Error UI опирается на server `code`; status — fallback.
- QR/link — short-lived credential: не логировать, не сохранять и не показывать proof отдельно.
- Anonymous radio и offline catalog всегда остаются доступны.
- Использовать существующие Compose Material 3, egui, ZXing/qrcode и RockCast i18n. Новых UI,
  state-machine, QR и navigation dependencies не добавлять.

## 4. Единая модель пользовательских состояний

| State | Что видит пользователь | Primary action |
| --- | --- | --- |
| `Disconnected` | устройство не подключено, raw device name | `Подключить это устройство` |
| `Starting` | короткий progress, повторный tap запрещён | нет |
| `WaitingForApproval` | target, steps, timer, phrase, link/QR | `Открыть защищённую ссылку` |
| `ConnectedFirstTime` | крупный success, account, current device | `Продолжить` / `Открыть устройства` |
| `Connected` | account и device list | `Обновить` только как secondary |
| `RecoverableError` | offline/5xx, сохранённый connected context | `Повторить` |
| `TerminalPairingError` | expired/rejected/limit/upgrade с точной причиной | `Создать новую ссылку` или `Обновить приложение` |

Это продуктовая модель, не shared code. В RockMobile достаточно расширить существующий
`AccountUiState`; в RockCast заменить группу `pairing/account_*` полей одним enum.

## 5. Этап A — web-регистрация, pairing и кабинет

Эти задачи принадлежат RockServer web/API. Они одновременно исправляют самостоятельный web UX и
разблокируют корректный flow обоих native clients.

### A1 — разделить registration input и authenticated account

**Priority:** P0. **Файлы:** `web/src/app.tsx`, web tests.

- `registrationName`: только editable input.
- `authenticatedAccountName`: устанавливается только из verified registration/auth session.
- Approve UI разрешён только при `authenticatedAccountName && csrf`.
- Ввод текста никогда не меняет lifecycle state.
- Нажатие `Создать аккаунт` запускает ceremony явно; пустое имя использует documented default.

### A2 — ввести exclusive browser pairing state

**Priority:** P0. **Файлы:** `web/src/app.tsx`, web tests.

Минимальный discriminated union:

```text
loading | anonymous | authenticated | approving | approved | terminal | unavailable
```

- После `204` перейти в `approved`; убрать QR, expiry, passkey/create и approve.
- Success: `RockMobile/RockCast — <raw label> подключён к «<account>»`.
- Primary для Mobile: `Вернуться в RockMobile`.
- Secondary: `Открыть аккаунт и устройства` → `/`.
- Для RockCast: `Вернитесь в RockCast` + `Открыть аккаунт и устройства`.
- Reload уже approved/consumed request не должен восстанавливать enabled approve.

### A3 — structured client errors

**Priority:** P1. **Файлы:** OpenAPI, server DTO contract, оба client transports.

Клиенты сохраняют только safe tuple:

```text
status + code + request_id
```

Не сохранять response body, URL query, headers или proofs. Канонические codes:

- `pairing_pending`
- `pairing_rejected`
- `pairing_expired`
- `device_limit_reached`
- `client_upgrade_required`
- `pairing_unavailable`

### A4 — безопасный handoff URL

**Priority:** P1.

- Новый link: `https://alex.vault57.ru/?code=<code>#secret=<proof>`.
- Web считывает fragment в памяти и сразу делает `history.replaceState` без proof.
- Query-format временно поддерживается только на rollout window.
- Для возврата в Android использовать verified App Link с узким path
  `/return/rockmobile`; не перехватывать обычную pairing URL.
- App Link не содержит request ID, code или proof; он только поднимает приложение, которое уже
  polling/resume получает результат.

### A5 — naming invariant

**Priority:** P1. **OpenAPI source of truth.**

`device_display_name` — raw пользовательская/machine label без `RockMobile —`/`RockCast —`.
Legacy rows не мигрировать автоматически. Presentation helper добавляет product не более одного
раза и распознаёт старый canonical prefix.

### A6 — понятная anonymous landing page

**Priority:** P1. **Файлы:** `web/src/app.tsx`, `style.css`, browser tests.

На `/` без browser session показывать две ясные операции:

- Primary `Войти с passkey`.
- Secondary `Создать Rock-аккаунт` → registration state/path.

Коротко объяснить разницу: вход открывает существующий account; создание создаёт новый. Pairing
code/UUID/manual token input на landing не добавлять. Новый device всегда начинает flow из
RockMobile/RockCast по защищённой ссылке.

Не добавлять router dependency: достаточно текущего Preact root и `location.pathname/search` для
`/`, `/register` и request-specific pairing URL.

### A7 — отдельная web-регистрация

**Priority:** P0/P1.

- Отдельный heading `Создать Rock-аккаунт` и back link `У меня уже есть аккаунт`.
- Одно поле `Имя аккаунта`, label + пример; ввод не меняет screen/state.
- Primary `Создать аккаунт с passkey` запускает ceremony только по click.
- Во время ceremony button disabled и текст `Создаём…`; double submit невозможен.
- Cancelled passkey возвращает заполненную форму с понятным текстом, не создаёт пустой account.
- После verify показать success: account создан, browser signed in.
- Если registration началась из pairing URL, success ведёт к точному target confirmation; если с
  `/register`, ведёт в account centre.
- Default name допускается только как явно выбранный вариант `Использовать «Rock account»`, а не
  как скрытая реакция на пустое поле.

### A8 — информационная архитектура web-кабинета

**Priority:** P1. **Current route:** `/` с authenticated browser session.

Порядок блоков:

1. Header `Rock-аккаунт «<name>»` и badge `Выполнен вход в браузере`.
2. Quick status: `N из 10 устройств`.
3. Devices.
4. `Как подключить новое устройство`.
5. Browser/passkey security explanation.
6. `Выйти из браузера` как secondary action.

Не смешивать browser session с native device: browser не показывать как RockCast/RockMobile и не
называть `текущим устройством` в native device list.

### A9 — карточки и действия устройств

**Priority:** P1.

Каждая card показывает:

- один product/raw name (`RockMobile — RMX5056`);
- active/inactive session понятным текстом;
- connected/last activity в локальном формате;
- `Переименовать`;
- `Отключить` с confirmation, где объяснено, что native app потеряет account access.

После rename/revoke обновлять только account data и показывать inline success. Не оставлять старую
card рядом с error. Device ID, user ID, session ID, app proof и bearer tokens в DOM не выводить.

Empty state: `Подключённых устройств пока нет` + инструкция открыть RockMobile/RockCast и начать
pairing. Limit state: `10 из 10` + сначала отключить старое устройство.

### A10 — web account/session states

**Priority:** P1.

- `Loading`: skeleton/progress, не anonymous flash.
- `Authenticated`: cabinet.
- `Anonymous`: landing.
- `SessionExpired`: `Сессия браузера завершена`; devices не показывать из stale state.
- `ServiceUnavailable`: не утверждать logout; предложить retry.
- Logout success возвращает anonymous landing, native devices остаются подключёнными.
- Rename/revoke/logout имеют independent busy state, чтобы одна операция не блокировала весь UI.

### A11 — responsive layout и accessibility

**Priority:** P1/P2.

- Проверить 360/390/430px и desktop 700–1200px.
- На mobile actions cards становятся full-width, но destructive action остаётся визуально
  отличимой от primary.
- Visible focus, semantic headings, `role=alert/status`, labels и keyboard navigation.
- Verification phrase/short code читаются screen reader и не зависят только от typography/color.
- Success, warning и error имеют icon + heading + text, не только цвет.
- Minimum contrast WCAG AA; системный zoom 200% не скрывает actions.

### A12 — кабинет после pairing success

**Priority:** P0 UX.

Success screen не должен быть тупиком:

- Primary Mobile: `Вернуться в RockMobile`.
- Primary RockCast: `Вернуться в RockCast`/явная инструкция закрыть browser.
- Secondary для обоих: `Открыть аккаунт и устройства`.
- При переходе в cabinet только что подключённая device card подсвечивается один раз текстом
  `Только что подключено`, без хранения этого marker в DB.
- Reload не повторяет approve и не создаёт вторую device/session.

## 6. Этап B — RockMobile bugs и UX/UI

### M1 — исправить identity приложения

**Priority:** P1. **Минимальный scope:** Android strings/top app bar.

- Заголовок и content descriptions говорят `RockMobile`, не `RockCast`.
- Не переименовывать server/account product: `Rock-аккаунт` остаётся общим.
- Добавить visible build `0.1.1 (<short revision>)` внизу account dialog, не на главном экране.
- Увеличить `versionCode`; каждая устанавливаемая сборка получает уникальный code.

### M2 — raw device name

**Priority:** P1. **Файлы:** `AccountModels.kt`, tests.

- Default: `RMX5056`, fallback `Android device`.
- Поле label: `Имя устройства`; preview рядом: `RockMobile — RMX5056`.
- Legacy `RockMobile — RMX5056` отображается один раз через helper.
- Byte-length validation остаётся в одном существующем helper.

### M3 — starting/disconnected screen

**Priority:** P1. **Файлы:** `AccountViewModel.kt`, `AccountScreen.kt`.

- Добавить `Starting`; блокировать повторное создание request.
- Короткий текст: `Подключите RockMobile к существующему Rock-аккаунту.`
- Secondary: `Радио и сохранённые станции работают без аккаунта.`
- Primary full-width: `Подключить RockMobile`.
- Account creation не объяснять длинным абзацем здесь; это делает browser.

### M4 — waiting screen для того же телефона

**Priority:** P1.

Порядок сверху вниз:

1. `Шаг 1 из 2 · Подтвердите в браузере`.
2. Target `RockMobile — RMX5056`.
3. Countdown `Ссылка действует ещё 09:42`, не сырая ISO date.
4. Primary `Открыть защищённую ссылку`.
5. Phrase для сравнения.
6. Collapsible/secondary `Подключить через другое устройство` с QR.
7. `Отменить` как text/outlined action.

Возврат activity вызывает существующий `resumePairing`. Не создавать второй request при каждом
resume/recomposition.

### M5 — QR component

**Priority:** P1.

- 320dp target, минимум 256dp по доступной ширине.
- Явный ZXing `MARGIN=4`, error correction `M`.
- Integer module scale; не масштабировать готовый bitmap нецелым коэффициентом.
- Black on white, 21:1, без overlay/rounded modules.
- Accessible description: target + expiry, без secret/code.
- Unit/golden test проверяет quiet zone и synthetic link shape.

### M6 — first success и возврат из browser

**Priority:** P0 UX.

- После completion перейти в `ConnectedFirstTime`, а не сразу в обычный длинный список.
- Крупная check icon + `RockMobile подключён`.
- `Аккаунт: Rock account`; `Это устройство: RockMobile — RMX5056`.
- Primary `Готово` закрывает dialog и возвращает к радио.
- Secondary `Открыть устройства` переводит в обычный `Connected`.
- App Link из browser поднимает activity; state берётся из ViewModel/store, не из link params.
- Если browser вернулся раньше completion, показывать `Завершаем подключение…` до `200`.

### M7 — connected account centre

**Priority:** P1.

- Current device всегда первым и отмечен `Этот телефон`.
- У current device нет кнопки `Отключить`; для него отдельная action `Выйти на этом телефоне`.
- Другие devices: product/raw name, connected/last activity, `Отключить` с confirmation.
- Device list failure сохраняет account/current-device card и показывает non-blocking banner.
- Refresh 401 очищает session; network/5xx не делает вид, что logout уже произошёл.

### M8 — error mapping

**Priority:** P1.

| Code | Текст | Action |
| --- | --- | --- |
| `pairing_expired` | `Ссылка истекла` | `Создать новую` |
| `pairing_rejected` | `Подключение не подтверждено` | `Начать заново` |
| `device_limit_reached` | `Достигнут лимит устройств` | `Открыть устройства` |
| `client_upgrade_required` | `Обновите RockMobile` | без retry loop |
| `pairing_unavailable`/5xx/offline | `RockServer временно недоступен` | `Повторить` |

Не использовать диапазон `404..410` как один пользовательский текст.

## 7. Этап C — RockCast bugs и UX/UI

### C1 — один `AccountUiState`

**Priority:** P0/P1. **Файлы:** `src/app/mod.rs`, `ui/account.rs`, `actions/poll.rs`.

Заменить `pairing`, `pairing_status`, `account_message`, `account_profile`, `account_devices` одним
enum с данными. Не вводить generic state-machine crate.

- `Disconnected`
- `Starting`
- `Waiting { request, status }`
- `ConnectedFirstTime { profile, devices }`
- `Connected { profile, devices, banner }`
- `Error { kind, cached_profile? }`

Каждый match рисует один экран. Background result обновляет enum одним assignment.

### C2 — загрузка account при открытии окна

**Priority:** P1.

- При открытии окна, если secure credentials существуют, автоматически вызвать refresh/profile.
- До результата показать progress, не `Connect this PC`.
- `Connect this PC` существует только в `Disconnected`.
- Manual refresh остаётся secondary action только в `Connected`.
- 401 очищает stale profile/credentials; network/5xx сохраняет cached connected card с banner.

### C3 — disconnected/starting screen

**Priority:** P1.

- Title локализован через существующий `Lang`: `Аккаунт и устройства` / `Account & devices`.
- Raw default: `DESKTOP-685GRAQ`, fallback `This PC`.
- Preview: `RockCast — DESKTOP-685GRAQ`.
- Primary: `Подключить этот ПК`.
- Secondary copy: локальное радио продолжает работать без аккаунта.

### C4 — waiting screen

**Priority:** P1.

- Header `Подтвердите подключение в браузере`.
- Steps: scan/open → passkey → compare phrase → approve → return.
- QR 320 logical px, минимум 4 modules quiet zone, integer module size.
- Link action `Открыть защищённую ссылку`.
- Добавить `Копировать ссылку`; confirmation предупреждает не пересылать её.
- Countdown от parsed `expires_at`; terminal state не оставляет QR на экране.
- Cancel закрывает local job; server cancel endpoint — отдельная P2 task после основных fixes.

### C5 — first success

**Priority:** P0 UX.

- После `PairingResult::Ok` → `ConnectedFirstTime`.
- Крупный итог `Этот ПК подключён к «Rock account»`.
- Current device `RockCast — DESKTOP-685GRAQ`.
- Primary `Открыть устройства`; secondary `Готово`.
- Старый QR, connect form и error очищены атомарно переходом state.

### C6 — connected account centre

**Priority:** P1.

- Верхняя card: account + `Этот ПК` + session status.
- Список: current first, остальные далее; current нельзя revoke общей кнопкой.
- Для current — `Выйти на этом ПК`; для other — `Отключить` с confirmation.
- Empty list и unavailable list имеют разные тексты.
- Dates форматируются локально, сырые RFC3339 не показываются.
- Не показывать UUID/session/token fields.

### C7 — локализация account UI

**Priority:** P2.

- Добавить account strings в существующую `i18n::Strings`.
- Не создавать отдельный translation layer.
- Все account buttons/messages меняются вместе с текущим language setting.
- Error enums остаются language-neutral; text выбирается в view.

### C8 — safe diagnostics

**Priority:** P1.

В debug/file log допустимы только:

```text
client_build, endpoint_name, status, code, request_id, ui_transition
```

Запрещены URL, query, QR content, short code, phrase, proofs, tokens, account/device IDs и names.

## 8. Визуальная иерархия

Одинаковая для обоих клиентов:

- Success: зелёная/check семантика, но текст не зависит только от цвета.
- Error: одна короткая причина + recovery action; технические подробности отсутствуют.
- Waiting: progress/step, target, timer, primary action, verification, cancel.
- Connected: account summary сначала, devices затем, destructive actions внизу/справа.
- Minimum touch target Mobile 48dp; keyboard focus RockCast видим.
- Body text не меньше platform default; short code/phrase моноширинные только если это улучшает
  сравнение, не как декоративный стиль.

## 9. Порядок реализации и dependencies

| Порядок | Task | Repository | Зависит от |
| ---: | --- | --- | --- |
| 1 | A1 registration state bug | RockServer web | — |
| 2 | A2 browser success state | RockServer web | A1 |
| 3 | A6–A7 landing + registration | RockServer web | A1 |
| 4 | A8–A12 cabinet + post-success UX | RockServer web | A2 |
| 5 | A3 error DTO parsing contract | RockServer + clients | — |
| 6 | A5 raw-name invariant | RockServer OpenAPI | — |
| 7 | M1–M3 Mobile identity/start/name | RockMobile | A5 |
| 8 | C1–C3 RockCast state/start/name | RockCast | A5 |
| 9 | M4–M8 Mobile waiting/success/account/errors | RockMobile | A2, A3 |
| 10 | C4–C8 RockCast waiting/success/account/errors | RockCast | A2, A3 |
| 11 | A4 fragment + App Link | all three | A2; signing/assetlinks owner |
| 12 | Cross-device staging E2E | all | 1–11 deployed/installed |

RockMobile и RockCast tasks 5–8 можно вести параллельно после contract freeze. Server/web
разворачивается первым, клиенты — после backward-compatible server readiness.

## 10. Тесты

### RockServer web

- Ввод первой буквы оставляет create-account form.
- Anonymous landing ясно разделяет sign-in и registration.
- Registration с полной строкой, empty/default choice, cancel и double-click.
- Registration из pairing сохраняет target context; обычная registration открывает cabinet.
- Approve невозможен без authenticated session + CSRF.
- `204` скрывает approve и показывает success CTAs.
- Reload approved/consumed request не показывает enabled approve.
- Authenticated cabinet показывает точное число devices и не смешивает browser/native sessions.
- Rename/revoke/logout: success, 401, 5xx и stale-state removal.
- Empty, unavailable и 10-device limit states различимы.
- Fragment proof отсутствует в network URL, DOM text, console и referrer.
- Mobile widths 360/390/430px и zoom 200% без overflow/lost actions.

### RockMobile unit/Compose/instrumentation

- State transitions: disconnected → starting → waiting → first success → connected.
- Resume до/после server completion; late result после cancel не сохраняется.
- Error code matrix и no broad-range mapping.
- Current device не имеет revoke button.
- Raw/legacy names получают один prefix.
- QR quiet zone/dimensions и accessibility.
- App Link поднимает app без credential parameters.
- In-place upgrade сохраняет existing Keystore session.

### RockCast unit/UI

- Snapshot/assertions для каждого exclusive state.
- Connected screen не содержит connect/QR/expired text.
- Refresh 401 и 5xx дают разные states.
- Success очищает pairing атомарно.
- Raw/legacy names получают один prefix.
- QR quiet zone и integer modules.
- RU/EN account strings следуют `Lang`.
- Close/cancel race не сохраняет credentials.

### Manual staging

1. Clean disposable browser + supported builds.
2. С landing создать account без pairing; проверить redirect в пустой cabinet.
3. Новый account из pairing: ввести полное имя, создать passkey, approve.
4. Browser success → return Mobile → `ConnectedFirstTime`.
5. Добавить RockCast в тот же account → `ConnectedFirstTime`.
6. В web/Mobile/RockCast account centres видны ровно два devices, без двойных prefixes.
7. В web переименовать и отключить disposable other device; все surfaces обновляются после refresh.
8. Проверить browser logout отдельно от native sessions.
9. Проверить refresh, restart, offline, expiry, cancel, repeated approve, revoke other device,
   logout current device и device limit.
10. Сохранять только build/status/code/request_id; не QR/link/proofs/tokens.

## 11. Критерии приёмки

- Ввод имени account не меняет экран до явного нажатия `Создать аккаунт`.
- Web landing, registration, pairing и cabinet имеют разные headings и primary actions.
- После server `204/200` ни browser, ни native UI не оставляют активную кнопку `Подключить`.
- Browser предлагает понятный выбор: вернуться в приложение или открыть account devices.
- Cabinet сразу показывает только что подключённое устройство и сохраняется после reload.
- Browser logout не отключает RockMobile/RockCast; device revoke отключает только выбранный client.
- RockMobile после возврата показывает отдельный success, затем radio/account centre.
- RockCast после pairing показывает отдельный success и больше не показывает connect form.
- Product prefix везде ровно один.
- Network failure нигде не называется expired session.
- Current device нельзя случайно revoke как «другое».
- QR соответствует 4-module quiet zone, 256–320 target и сканируется двумя физическими камерами.
- Approval secret отсутствует в server-visible URL и logs.
- Builds однозначно различимы versionCode/version/revision.
- Offline radio и local catalog не зависят от account success/failure.

## 12. Что сознательно не входит

- Новый общий UI framework или shared client library.
- Редизайн основного station/player screen, кроме неправильного product title RockMobile.
- Массовая миграция legacy device names.
- Account deletion/passkey management redesign.
- Push notifications, background service или WebSocket вместо существующего короткого polling.
- Автоматический revoke старых duplicate devices.

Ленивая альтернатива, выбранная планом: сохранить существующий polling, Compose, egui и i18n;
исправить state ownership и rendering conditions. Этого достаточно для наблюдаемых bugs и целевого
UX без нового protocol transport или cross-client abstraction.
