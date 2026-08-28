# RM-011-G6 — staging cleanup runbook

## Назначение и границы

Этот runbook предназначен только для владельца staging `https://alex.vault57.ru`. Он не
объединяет аккаунты и не выполняет очистку автоматически. Команда `account_cleanup` по умолчанию
делает только read-only preview; применение возможно лишь для одного заранее выбранного UUID и
с точной фразой подтверждения.

Отчёт намеренно не показывает имена аккаунтов/устройств, credential bytes, credential IDs
WebAuthn, token hashes, bearer/refresh tokens или произвольные audit details. Для выбора цели
используйте независимую защищённую запись staging-владельца и проверку рабочего аккаунта через
обычный браузерный account centre. Если UUID нельзя однозначно связать с тестовым аккаунтом,
остановитесь и не угадывайте.

## Что сервер может сделать

- Deactivate один точно выбранный тестовый аккаунт: сохранить tombstone и audit, отозвать
  passkey rows, account identities, устройства, native sessions, refresh tokens, browser sessions,
  одобренные pairing requests и WebAuthn challenges.
- Revoke одно точно выбранное устройство: отозвать его native sessions и refresh tokens.
- Revoke одну точно выбранную server-side passkey credential row. Последний активный passkey
  живого аккаунта защищён и не отзывается этой командой.
- Сохранить записи для аудита и возможного восстановления из проверенного backup.

Физического `DELETE` строк нет: деактивация и revoke записывают `revoked_at`/tombstone. Активный
admin identity блокирует деактивацию аккаунта. В текущей схеме отдельного persisted admin principal
нет, поэтому операторский доступ остаётся deployment-контуром; это не новая публичная auth-модель.

## Предварительная проверка

1. Зафиксируйте approved image, migration version, время и checksum актуального encrypted
   `pg_dump` вне репозитория. Не прикладывайте backup, env-file или secret к тикету.
2. Убедитесь, что работа выполняется на staging, не на production, и что нет параллельного
   deploy/restore.
   Если staging host был bootstrap-нут до этой версии, сначала выполните стандартный `bootstrap`
   для обновления command-scoped sudo rule; bootstrap сохраняет существующие секреты. Без этого
   обновления cleanup-команда недоступна непривилегированному deploy operator, а вручную расширять
   sudoers нельзя.
3. В обычном браузере войдите через рабочий passkey и проверьте account name и список устройств.
   Сначала подтвердите, какой аккаунт должен остаться. Одинаковая строка `RockServer user` в
   системном менеджере ключей не является доказательством, что passkey старый.
4. Получите точный `account_id`, `device_id` или server-side credential row ID из защищённой
   staging-инвентаризации. Не копируйте имена или другие персональные данные в отчёт.

## Preview

После релиза, содержащего этот binary, выполните на staging host через разрешённый deploy
операторский контур:

```text
sudo -n /opt/rockserver/remote-ops-001-d.sh cleanup preview
```

Команда возвращает JSON с аккаунтами, dependency IDs, статусами, безопасными временными полями,
counts и `candidate_reason`. Preview не изменяет базу. Для локальной disposable базы допустим
эквивалент:

```text
ROCKSERVER_CLEANUP_ENV=staging cargo run --bin account_cleanup -- preview
```

Перед применением проверьте в preview:

- `candidate_status` равен `review_required`, а `status` — `active`;
- UUID и counts совпадают с одной конкретной тестовой записью;
- нет `protected: true`;
- активный рабочий аккаунт/устройство не выбран по имени или давности «на глаз».

## Точное применение

Используйте только одну из следующих форм. В `<UUID>` подставляется ровно один ID из preview;
wildcard, список ID и диапазон не поддерживаются.

```text
sudo -n /opt/rockserver/remote-ops-001-d.sh cleanup apply account --id '<ACCOUNT_UUID>' --confirm 'DEACTIVATE ACCOUNT <ACCOUNT_UUID>'
sudo -n /opt/rockserver/remote-ops-001-d.sh cleanup apply device --id '<DEVICE_UUID>' --confirm 'REVOKE DEVICE <DEVICE_UUID>'
sudo -n /opt/rockserver/remote-ops-001-d.sh cleanup apply credential --id '<CREDENTIAL_ROW_UUID>' --confirm 'REVOKE CREDENTIAL <CREDENTIAL_ROW_UUID>'
```

Для локальной disposable базы задайте `ROCKSERVER_CLEANUP_ENV=staging` и используйте ту же форму
`cargo run --bin account_cleanup -- apply ...`. Без этого маркера команда fail-closed и ничего не
читает/изменяет.
Неверная/отсутствующая фраза, не-UUID, уже отозванная запись, protected admin identity или
последний рабочий passkey останавливают действие до изменения данных. Account action не является
merge и не переносит устройства между аккаунтами.

## Проверка результата

1. Повторите `cleanup preview` и убедитесь, что выбранный account имеет `status: deleted`, а
   выбранные device/passkey — `revoked`.
2. Убедитесь по counts и статусам, что у деактивированного аккаунта нет active device, native
   session, refresh token, browser session, passkey, pairing request или WebAuthn challenge.
3. Проверьте, что появился только безопасный audit event соответствующего operator action; не
   извлекайте `details`, hashes или token columns в отчёт.
4. В браузере повторно проверьте, что рабочий сохранённый аккаунт и его устройство по-прежнему
   входят в систему. Если вход не подтверждается, остановите дальнейшую очистку.

## Восстановление и ручные действия

Приложение можно откатить на предыдущий проверенный image только если migration совместима, но
это не отменяет уже записанный tombstone/revoke. Автоматического undo нет. Для восстановления
данных используется только проверенный encrypted backup: остановить writer, восстановить backup
в чистый PostgreSQL cluster по restore rehearsal, проверить readiness и запустить утверждённый
image. Не исправляйте `users.status` или `revoked_at` вручную в рабочей базе.

Сервер не может удалить passkey из браузерного или Google Password Manager. Владелец вручную
открывает штатный менеджер passkey, находит запись для RP/site `alex.vault57.ru`, сначала
успешно проверяет новый рабочий passkey, затем удаляет старую запись одну за одной. Проверяйте
аккаунт и результат входа, а не одинаковое отображаемое имя; сохраните хотя бы один проверенный
рабочий passkey. Production credentials массово не переименовываются и не удаляются этой
staging-процедурой.
