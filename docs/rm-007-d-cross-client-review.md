# RM-007-D — межклиентская проверка локальных личных данных

**Дата проверки:** 2026-08-25  
**Область:** текущие незакоммиченные реализации RM-007-B в `C:\repos\rockmobile` и
RM-007-C в `C:\repos\rockcast`, контракт RM-007-A и локальные catalog release artifacts.  
**Результат:** **RM-007-D not passed.** Реализации не совместимы с portable profile v1 и
не готовы быть основанием для RM-011. OPS-001-A не зависит от RM-007-D и может продолжаться в
своих документальных границах; RM-011-A, зависящий от RM-007-D, заблокирован.

Проверка была read-only для клиентских репозиториев. Новый продуктовый код, shared catalog data,
сеть, production services, secrets, live databases, device tests и commits не использовались.

## Итог по критериям

- Одинаковые canonical station IDs из baseline `2026.08.2` подтверждены offline release gate во
  всех трёх consumers. URL и primary-stream не входят в личную identity в обоих local-store
  моделях.
- Field mapping **несовместим**: RockCast сериализует RM-007-A shape, RockMobile — отдельный
  SharedPreferences JSON с другими полями и типами времени.
- Lifecycle pure resolvers запрещают произвольный split и сохраняют split/removed/missing в
  quarantine, но Mobile resolver не подключён к production lifecycle data, а Cast remote/voice
  создаёт URL-derived identity.
- Offline catalog fallback подтверждён кодом и ранее существующими тестами; Android tests/lint в
  этой проверке не запустились из-за недоступного Gradle wrapper lock.
- Есть незакрытые High findings с риском потери читаемого профиля и portable station identity.

## Сопоставление полей и правил

| Контракт RM-007-A | RockMobile RM-007-B | RockCast RM-007-C | Вывод |
|---|---|---|---|
| `LocalProfile.schemaVersion`, `profileId`, `createdAt`, `updatedAt`, `metadata` | JSON содержит только `schemaVersion`, `launchCount`, `lastPlayedStationId`, `favourites`, `history`, `unresolved`; profile UUID и profile timestamps отсутствуют (`PersonalData.kt:17-20,84-96`). | Поля и camelCase serialization присутствуют (`personal_data.rs:35-48`). | **High incompatible shape.** |
| RFC 3339 UTC timestamps | Epoch milliseconds в Kotlin `Long` (`PersonalData.kt:17-19,61-69`). | RFC 3339 strings (`personal_data.rs:52-72,437-443`). | **High incompatible type/transport.** |
| `favourites`, `playbackHistory`, `unresolvedReferences`; metadata objects | Mobile использует ключи `history`, `unresolved`, плоские `name/source`, `candidates`, а duration хранится как `duration` (`PersonalData.kt:84-96`). | Контрактные имена и вложенная metadata (`personal_data.rs:35-106`). | **High incompatible shape.** |
| Canonical `stationId`; URL/stream ID не являются identity | Mobile сохраняет `Station.id`; baseline, extended и server DTO дают ID из catalog/server (`StationSources.kt:85-100`, `ExtendedCatalogStationSource.kt:103-108`, `RockserverDtos.kt:14-24`). | Local catalog сохраняет canonical ID, но server search и voice конструируют `rockserver-<hash(url)>` (`rockserver.rs:48-57`, `voice/dto.rs:49-61`), а успешный play записывает этот ID (`app/actions/poll.rs:62-64`). | **High Cast remote identity break.** |
| 500 favourites, 500 history, 90 days, five-minute coalescing; deterministic ordering | Константы совпадают; favourite/history ordering и retention совпадают (`PersonalData.kt:12-15,38-43,65-77`). Dedup сохраняет earliest favourite, но не объединяет latest metadata/greatest `updatedAt`. | Константы и history ordering/retention совпадают (`personal_data.rs:13-16,368-380`). Favourite `sort + dedup_by` сохраняет первую запись, но также не выполняет контрактное объединение latest metadata/greatest `updatedAt` (`personal_data.rs:324-366`). | **Medium:** limits/order в основном совместимы, merge rule неполон у обоих. |
| Не хранить secrets, telemetry, stream URL | Ни encoder, ни модели personal data не имеют таких полей (`PersonalData.kt:17-20,84-96`). | Profile types содержат только IDs, времена и разрешённую metadata (`personal_data.rs:35-106`). | Совместимо в проверенном personal storage; network/log telemetry отдельно не оценивалась. |

## Lifecycle matrix

| Case | RockMobile | RockCast | Verdict |
|---|---|---|---|
| URL или primary-stream change при том же station ID | Pure test проверяет сохранение ID; store URL не пишет (`PersonalDataTest.kt:12-15`). | Persistence test меняет URL и повторно открывает профиль, ID остаётся прежним (`personal_data.rs:509-521`). | Совместимо для local canonical stations. |
| `merged` | Pure resolver следует merge chain до active (`PersonalData.kt:30-33`). | Resolver следует acyclic single-target chain (`personal_data.rs:160-190`). | Базовая семантика совпадает. Mobile проверяет split/removed только для исходного ID, а не terminal merge-chain node; этот edge case не покрыт. |
| `split` | Создаёт unresolved с candidates, active запись удаляется после построения результата (`PersonalData.kt:34-44`). | Создаёт unresolved с candidates (`personal_data.rs:178-184,324-366`). | Нет автоматического выбора; совпадает в pure logic. |
| `removed` / missing | Оба сохраняют unresolved и не делают URL/name lookup (`PersonalData.kt:34-44`; `personal_data.rs:165-189,324-366`). | То же. | Нет тихого удаления внутри вызванного resolver. |
| Реальная lifecycle активация | `PersonalDataStore.reconcile` нигде в production не вызывается; baseline parser проверяет наличие tombstones, но не передаёт их personal store, extended snapshot не даёт lifecycle graph. | `catalog_resolver()` строится из принятого локального JSON snapshot и передаётся при open (`stations/catalog.rs:89-151`, `app/actions/poll.rs:137-144`). | **High Mobile integration gap.** Pure tests не доказывают migration/restart behavior приложения. |

Текущий baseline содержит zero tombstones, поэтому release verification подтверждает identity bytes,
но не является end-to-end доказательством merge/split/retire на обоих клиентах.

## Offline-first и источники каталогов

- RockMobile: `RockcastAssetStationSource` читает checksum-pinned baseline без сети; extended
  SQLite используется только после локального release gate, иначе `FallbackLocalStationSource`
  возвращает baseline (`StationSources.kt:29-63`, `FallbackLocalStationSourceTest.kt:11-33`).
  Remote failure не нужен для чтения personal SharedPreferences. Однако lifecycle reconciliation
  намеренно не запускается ни по baseline, ни по extended catalog, поэтому offline persistence
  работает, а offline lifecycle migration — нет.
- RockCast: `load_catalog` и personal store локальны; resolver строится из принятого embedded или
  override JSON/TXT snapshot. Radio Browser enrichment и RockServer search являются отдельными
  путями. Personal open не вызывает сеть (`stations/mod.rs:102-107`,
  `stations/catalog.rs:89-176`, `personal_data.rs:200-221`).
- Release gate подтвердил одинаковый baseline checksum
  `3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d` для RockServer,
  RockMobile и RockCast и проверил Mobile extended manifest локально.

## Migration, restart и rollback

RockCast atomically replace-writes JSON, fail-closes unsupported schema, делает pre-migration copy
до lifecycle rewrite и пишет journal (`personal_data.rs:200-221,304-321,445-487`). Targeted test
подтвердил restart persistence и legacy-unmapped quarantine. Не проверены контрактные требования
удалить backup только после успешного restart, автоматический rollback API и сохранение later user
edits; journal не содержит требуемых counts. Это Medium incompleteness.

RockMobile использует один SharedPreferences value и synchronous `commit`, но результат `commit()`
игнорируется (`PersonalData.kt:72-81`). `read()` перехватывает любую decode/schema ошибку и молча
возвращает новый пустой `PersonalData`, после чего `init` сразу записывает его. Нет durable backup,
journal, replace-on-success migration или rollback. Unsupported schema и повреждение поэтому могут
тихо уничтожить читаемое старое значение. Это High data-loss finding. Фактический restart test
для store отсутствует; pure resolver idempotence не заменяет storage/restart verification.

## Findings по приоритету

### High

1. **RockMobile profile не соответствует portable profile v1:** отсутствуют profile identity и
   profile timestamps, timestamps имеют другой тип, JSON keys/metadata layout несовместимы.
2. **RockMobile fail-open чтение может стереть данные:** corrupt/unsupported profile превращается
   в empty, а init переписывает store; backup/journal/rollback отсутствуют и commit failure
   игнорируется.
3. **RockMobile lifecycle resolver не подключён в приложении:** `reconcile` не вызывается,
   tombstones не доходят до personal data; проверена только pure функция.
4. **RockCast remote/voice personal identity URL-derived:** `rockserver-*` записывается в историю,
   хотя RM-007-A разрешает только canonical server station ID. При следующем local resolution такая
   запись становится `legacy-unmapped`, поэтому portable identity теряется.

### Medium

1. Оба resolver-а при favourite dedup не объединяют greatest `updatedAt` и latest non-null display
   metadata согласно контракту.
2. RockCast migration journal не содержит counts; lifecycle backup/restart-retention и rollback
   restore не проверены. Mobile migration/rollback отсутствует полностью и учтён как High.
3. Mobile merge-chain, заканчивающийся split/removed, классифицируется по исходному ID и не имеет
   targeted coverage; legacy-map target также не перепроверяется как active.
4. Ни один клиентский suite не имеет одной общей сериализованной cross-client fixture; текущие
   tests сравнивают похожие правила только внутри разных native shapes.

## Выполненные проверки

| Команда | Результат |
|---|---|
| `python tools\release_sync.py verify rockserver C:\repos\rockserver --release release\2026.08.2` | PASS, baseline `2026.08.2`, ожидаемый SHA-256. |
| То же для `rockcast C:\repos\rockcast` | PASS. |
| То же для `rockmobile C:\repos\rockmobile` с `--extended-manifest ...mobile.1.manifest.json` | PASS, baseline и extended manifest. |
| `python -m unittest discover -s tests -v` в catalog repo | PASS, 12/12, включая tombstones, corruption и repeatable rollback. |
| `cargo test catalog --lib` в RockServer | PASS, 14/14 selected, 67 filtered. |
| `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test` в RockServer | PASS; full test result: 81 library + regular binary/API suites, 7 credential/database-dependent tests ignored. |
| `cargo test personal_data --lib` в RockCast | Не стартовал: access denied к существующему `target\debug\.cargo-lock`. |
| `cargo test personal_data --lib --target-dir C:\repos\rockserver\target\rm007d-rockcast` | PASS, 5/5, 50 filtered. |
| RockCast `cargo fmt --check`; strict all-target/all-feature Clippy; full `cargo test` с тем же `--target-dir` | PASS; 55 library + 2 relay integration tests, 8 live-network tests ignored. |
| `$env:JAVA_TOOL_OPTIONS='-Xmx8G -Xms512m -Duser.home=C:\Users\alex'; .\gradlew.bat testDebugUnitTest --tests com.rockmobile.data.personal.PersonalDataTest --console=plain` | **NOT RUN:** wrapper до конфигурации/компиляции получил access denied на `gradle-9.3.1-bin.zip.lck`. |
| Та же environment policy; `.\gradlew.bat lintDebug --console=plain` | **NOT RUN:** тот же wrapper lock. Lint не объявляется прошедшим; ранее известные три lint errors этой проверкой не переоценены. |

## Ограничения и readiness

Не выполнялись Android compile/unit/lint вследствие wrapper blocker, device/UI restart smoke,
реальный rollback личного store, stream playback, сеть, live RockServer/PostgreSQL или import/export.
RockCast lifecycle tests используют synthetic tombstones; Mobile lifecycle tests — только pure
objects. Full RockServer и RockCast suites прошли; Android suite остаётся непрогнанным.

**Verdict:** OPS-001-A может начинаться независимо как design-only задача. RM-011-A и любая схема,
которая предполагает переносимость/синхронизацию текущих local profiles, не готовы до устранения
всех High findings и повторной cross-client проверки одной общей fixture.

**RM-007-D not passed.**

## Remediation update — 2026-08-25

По явному запросу пользователя реализации были исправлены после первоначального review:

- RockMobile заменил draft SharedPreferences shape на portable v1 field names, UUID profile/
  record identity и RFC-3339 timestamps. Старый epoch-based shape мигрируется с сохранением raw
  backup и count journal; invalid/unsupported input fail-closes без перезаписи, synchronous commit
  проверяется, rollback выполняется только явно.
- Mobile resolver теперь проверяет active legacy targets, классифицирует terminal merge-chain
  split/removed, объединяет latest favourite metadata и вызывается при startup с checksum-pinned
  baseline active/legacy/tombstone index.
- RockCast search/voice DTO сохраняет canonical RockServer `id`, а не URL-derived `rockserver-*`;
  favourite dedup объединяет latest metadata, migration journal содержит counts и добавлен explicit
  backup rollback.
- RockCast `cargo fmt --check`, strict Clippy и full `cargo test` после remediation прошли: 55
  library + 2 relay integration tests; 8 live-network tests ignored.

RockMobile targeted unit и `lintDebug` повторно остановились до configuration/compilation на том же
недоступном Gradle wrapper `.zip.lck` при обязательном process-local
`-Duser.home=C:\Users\alex`. Поэтому исправления Mobile остаются source-reviewed, но не
compile-verified; три ранее известные lint errors не объявлены исправленными. До успешного Android
unit/lint запуска итоговый gate остаётся **RM-007-D not passed**.
