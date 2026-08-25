# OPS-001-A — проект production-развёртывания

**Статус:** OPS-001-A одобрена 2026-08-25; локальный OPS-001-B foundation реализован и проверяется
через `verify-compose.ps1`. Этот каталог не содержит production `.env`, секретов, VPS, DNS или
registry credentials.

## OPS-001-B local foundation

`Dockerfile` builds the locked Rust dependency graph in a pinned Rust toolchain image and runs the
service as an unprivileged user. `compose.yaml` defines the three-service topology with no host
ports for RockServer or PostgreSQL. `compose.local.yaml` publishes only loopback HTTP through Caddy
for a disposable local check; `compose.production.yaml` publishes only Caddy TCP 80/443 and swaps
in the HTTPS template. `.env.example` contains safe markers only. Run the local check from the
repository root after Docker Engine is available:

```text
docker build --tag rockserver:local .
powershell -ExecutionPolicy Bypass -File deploy/verify-compose.ps1 -Mode local -Start
```

The script validates rendered Compose configuration without printing environment values, starts a
disposable project, checks `GET /health/ready` through Caddy, and removes only that project's
containers, network and volumes by default. A production launch still requires the manual inputs
listed below; this repository does not substitute a fake domain or secret.

## Граница и фиксированные значения дизайна

Проект предназначен для одного VPS и трёх контейнеров в одном Compose project:
`caddy`, `rockserver` и `postgres`.  Единственная записанная здесь endpoint-цель —
`api.rockserver.example.invalid`. Это зарезервированный, не маршрутизируемый placeholder
(RFC 2606), выбранный специально потому, что для этой задачи не разрешено использовать
реальный домен или выполнять deployment. До ручного запуска владелец должен заменить его в
секретной production-конфигурации на принадлежащий ему домен; такая замена является частью
OPS-001-D, а не публикацией из этого репозитория.

| Граница | Точный порт | Доступ |
| --- | --- | --- |
| Internet → Caddy | TCP 80 | public; только HTTP→HTTPS redirect и ACME challenge |
| Internet → Caddy | TCP 443 | public; HTTPS API |
| Caddy → RockServer | TCP 3000 | только сеть Compose `edge` |
| RockServer → PostgreSQL | TCP 5432 | только сеть Compose `database` |
| Internet → RockServer/PostgreSQL | любые | запрещено |
| SSH → VPS | TCP 22 | только утверждённые администраторские source addresses/ключи; password login disabled |

`caddy` подключён к `edge`; `rockserver` подключён одновременно к `edge` и изолированной
`database`; `postgres` подключён только к `database`. В production Compose не публикует `3000`
или `5432` через `ports:`. Caddy reverse-proxy направляет трафик исключительно на
`rockserver:3000`; HTTPS завершается у Caddy. Внутри контейнерной сети приложение слушает
`0.0.0.0:3000`, но это не делает порт публичным.

## Persistent data и ownership

| Ресурс | Владелец | Хранение | Восстановление/удаление |
| --- | --- | --- | --- |
| PostgreSQL cluster | RockServer service owner | named volume `rockserver-postgres-data` | восстанавливается только из проверенного encrypted logical backup |
| Caddy certificates/account state | VPS operator | named volumes `caddy-data`, `caddy-config` | восстанавливается с VPS volume; повторная выдача сертификата допустима только для реального домена |
| Images и Compose/config templates | release owner | staging: checksummed SSH artifact; production policy is separate | immutable commit-SHA image; не редактируется в работающем контейнере |
| Backups | backup owner, отдельный от runtime VPS | зашифрованное off-VPS storage | доступ проверяется restore rehearsal; retention и encryption key custody требуют human approval |

Docker volumes не являются backup. PostgreSQL не должен получать host bind mount с неясным
ownership; backup-файлы не должны храниться в repository, image или обычном service volume.

## Production environment contract

Production values находятся вне Git: например, в root-owned env-file на VPS с правами чтения
только у deploy operator, либо в одобренном secret store. В Compose можно ссылаться только на
имена ниже; логи, `docker compose config` в общем канале и ticket attachments не должны печатать
значения. Значение `__INJECTED_OUTSIDE_GIT__` ниже является маркером документации, не рабочим
секретом.

| Переменная | Класс | Назначение / правило |
| --- | --- | --- |
| `ROCKSERVER_DOMAIN` | non-secret deployment setting | точное имя Caddy site; пока только `api.rockserver.example.invalid` в этом дизайне |
| `ROCKSERVER_BIND_ADDR` | non-secret | `0.0.0.0:3000` внутри сети `edge` |
| `DATABASE_URL` | secret | URL к `postgres:5432`; содержит отдельные DB credentials и не выводится |
| `POSTGRES_DB`, `POSTGRES_USER` | restricted configuration | отдельные production names, не development defaults |
| `POSTGRES_PASSWORD` | secret | `__INJECTED_OUTSIDE_GIT__`; не повторяется в URL, shell history или YAML |
| `ROCKSERVER_API_BEARER_TOKEN` | secret | `__INJECTED_OUTSIDE_GIT__`; случайный уникальный production credential минимум 32 bytes |
| `RUST_LOG` | non-secret | безопасный уровень без request body, credentials или personal data |
| `YANDEX_AI_API_KEY`, `YANDEX_FOLDER_ID` | optional secret/restricted configuration | отсутствуют, пока провайдер не утверждён; оба задаются вместе |
| `ROCKSERVER_ONNX_*`, `ORT_DYLIB_PATH` | restricted paths/settings | только локально смонтированные, проверенные runtime assets; не URL для download |

The service now requires `ROCKSERVER_API_BEARER_TOKEN` and `DATABASE_URL` at startup. It does not
select the in-memory catalog when `DATABASE_URL` is absent; that repository remains available only
to isolated deterministic tests. The local launcher reads both values from the ignored `.env` and
never prints the credential.

## Health, release и rollback

* Контейнерный liveness check RockServer: `GET http://rockserver:3000/health/live`.
* Readiness gate Caddy/release: `GET https://<domain>/health/ready` возвращает HTTP 200 только
  после доступности repository/database. Это единственный success criterion rollout.
* PostgreSQL healthcheck использует `pg_isready` внутри `database`; его порт не проверяется с
  Internet.
* CI сначала выполняет format, strict Clippy, tests и строит immutable image с commit-SHA tag.
  Публикация в private registry и deployment требуют отдельного ручного approval.
* Перед migration release owner создаёт encrypted off-VPS `pg_dump` backup и фиксирует image tag,
  migration versions, backup checksum и timestamp в защищённом release record. Если preflight,
  migration или public readiness не проходит, traffic не считается переключённым.
* Rollback приложения означает возврат Caddy/Compose к предыдущему **проверенному** image tag.
  Он разрешён только когда migrations backward-compatible. При несовместимой migration rollback
  включает остановку writer, восстановление проверенного `pg_restore` backup в чистый PostgreSQL
  cluster, validation, затем запуск прежнего image и readiness check. Нельзя откатывать image
  поверх более новой несовместимой schema.

## OPS-001-C: CI, release gate и deploy script

`.github/workflows/ci-release.yml` выполняет `cargo fmt --check`, strict Clippy, `cargo test` и
собирает image с label `org.opencontainers.image.revision=$GITHUB_SHA` и tag
`sha-$GITHUB_SHA`. В job выполняется локальный Compose readiness smoke через loopback Caddy;
временные CI credentials генерируются только внутри job. Публикация в GHCR запускается только
через `workflow_dispatch` с input `publish_image=true` и environment `release-gate`, где approval
должен быть настроен владельцем репозитория. Workflow не содержит registry или application secret.

`deploy/release.ps1` — единственный описанный rollout entry point. Он принимает только immutable
image reference с digest (`...@sha256:<64 hex>`), рендерит production Compose без вывода значений,
проверяет, что host ports есть только у Caddy, а затем в режиме `deploy`:

1. ждёт healthy PostgreSQL;
2. создаёт `pg_dump --format=custom` во внешнем backup directory и записывает checksum в соседний
   release record;
3. запускает конкретный image без build/ручной правки контейнера, что применяет embedded migrations;
4. проверяет переданный approved `-ReadinessUrl` и принимает rollout только при HTTP 200.

`-Mode rollback -Image <previous-digest>` повторно поднимает предыдущий проверенный image и
проверяет тот же readiness URL. Такой rollback разрешён только для backward-compatible migrations;
для несовместимой схемы сначала требуется восстановление backup по recovery procedure. Скрипт не
принимает production env-file из репозитория и не печатает его значения. Пример безопасного dry-run:

```text
powershell -ExecutionPolicy Bypass -File deploy/release.ps1 `
  -Mode preflight `
  -Image ghcr.io/example/rockserver@sha256:<64-hex-digest> `
  -DryRun
```

## OPS-001-C: pg_dump/pg_restore rehearsal

`deploy/restore-rehearsal.ps1` принимает backup-файл, поднимает отдельную disposable PostgreSQL
сеть без host port, выполняет `pg_restore --clean --if-exists --no-owner --no-privileges`,
проверяет наличие RockServer tables и запускает указанный local RockServer image against the
restored database. Readiness проверяется только внутри disposable network; после завершения
контейнеры и сеть удаляются, если не указан `-KeepArtifacts`. Временные database/API credentials
генерируются процессом и не выводятся. Dry-run проверяет только непустой backup и checksum:

```text
powershell -ExecutionPolicy Bypass -File deploy/restore-rehearsal.ps1 `
  -BackupFile C:\outside-repo\rockserver-backup.dump `
  -DryRun
```

Реальный rehearsal перед production registration обязателен; его результат фиксируется с
checksum, duration, readiness и cleanup в release record. Backup-файлы и release records должны
оставаться вне Git и вне application/container volumes.

## OPS-001-D: one-time bootstrap and one-command staging update

The Windows owner copies `deploy/private.inventory.example.psd1` to the ignored
`deploy/private.inventory.psd1` and fills only `SshUser`, `SshHost`, and `Domain`. A password is not
a field, is never read from config, and is never placed in argv or logs. On the first bootstrap the
launcher creates the ignored `deploy/.keys/rockserver_ed25519` key, and OpenSSH itself shows one
explicit interactive login-password prompt to append the public key. All later transfers use that
generated key.

Before the first deploy, the owner explicitly reviews the host prerequisites (supported automatic
path: Ubuntu/Debian with apt), then runs:

```text
Copy-Item deploy/private.inventory.example.psd1 deploy/private.inventory.psd1
powershell -ExecutionPolicy Bypass -File deploy/ops-001-d.ps1 -Action bootstrap -InstallDocker
```

Bootstrap allocates a TTY, so `sudo` may display its own interactive prompt when the account needs
one; the sudo password also stays exclusively in the remote terminal. The safer one-time host setup
is to have a VPS administrator run this bootstrap while watching the session, rather than granting
the account general passwordless sudo. Bootstrap can install Docker only with the explicit
`-InstallDocker` switch on apt-based Ubuntu/Debian, provisions `/opt/rockserver`, creates DB/API
secrets only when absent, and installs a `visudo`-validated rule limited to the root-owned
`/opt/rockserver/remote-ops-001-d.sh deploy *` command. Normal deploy uses `sudo -n` and stops with
an instruction to rerun bootstrap if this rule is missing; it never falls back to a password prompt.
The automation does not change SSH password-login policy or firewall rules.

After bootstrap, every staging update is exactly one local command:

```text
powershell -ExecutionPolicy Bypass -File deploy/ops-001-d.ps1 -Action deploy
```

The launcher requires a clean worktree, resolves the exact current `HEAD` full SHA, builds
`rockserver:sha-<SHA>` locally with the same SHA in `org.opencontainers.image.revision`, and verifies
the immutable Docker image ID. It creates a checksummed `docker save` artifact, copies it over SSH,
then the VPS verifies the transfer hash, runs `docker load`, and checks both the loaded image ID and
revision label before Compose may use that exact local tag. There is no `latest`, remote Git build/
pull, GitHub, GHCR, external registry, or registry credential in the staging path.

The launcher reads the ignored root `.env` and copies only allowlisted `YANDEX_AI_API_KEY`,
`YANDEX_FOLDER_ID`, `YANDEX_SPEECHKIT_API_KEY`, and `YANDEX_SPEECHKIT_FOLDER_ID`; absent
optional values are omitted. It writes one UTF-8 env entry per line; the remote root-owned
`/opt/rockserver/release.env` is mode `0600`. Repeated runs replace only owner-controlled settings
and those four optional Yandex entries while preserving generated `POSTGRES_PASSWORD`,
`ROCKSERVER_API_BEARER_TOKEN`, and database settings. No secret value appears in summaries or
release metadata. The VPS creates a custom-format backup, runs embedded migrations and then imports
the bundled checksum-pinned complete SQLite catalog after PostgreSQL is healthy, before the service
starts. The current pinned release contains 16,825 active playable stations and deployment refuses
a release below the 16,000-station gate. The same one-shot seed job then runs the checksum-pinned
ONNX E5 backfill for every imported station; the service starts only after both catalog and vector
steps succeed. Repeating the importer and backfill is idempotent by stable station and stream
identities and cannot substitute the 41-station development fixture. The only release
output/record fields are commit, image ID, artifact checksum, catalog version/count, backup checksum
and readiness.

ONNX semantic search is enabled automatically. The committed
`deploy/onnx-assets.lock.json` pins the exact `intfloat/multilingual-e5-small` ONNX graph,
matching tokenizer, and the Linux x64 ONNX Runtime archive to official HTTPS sources plus
SHA-256 values. On the first deployment the VPS downloads them only when absent or invalid,
checks every archive/download before atomically placing the files, then starts RockServer with
semantic search enabled. Subsequent deployments use the verified cache. No ONNX URL, hash, or
local asset path is entered by the owner. Updating the model/runtime is a normal reviewed Git
change to this lock file, never an unpinned automatic update.

Local validation, with no VPS or credential/network use:

```text
powershell -ExecutionPolicy Bypass -File deploy/ops-001-d.ps1 -Action deploy -DryRun
powershell -ExecutionPolicy Bypass -File deploy/tests/ops-001-d-tests.ps1
```

## Backup и restore rehearsal

Backup owner выполняет scheduled encrypted `pg_dump --format=custom` в off-VPS storage. Частота,
retention, encryption mechanism, key custodian, target и RPO/RTO ещё не утверждены — roadmap
требует их определить до public registration, но не задаёт чисел, поэтому этот дизайн их не
выдумывает. До запуска нужно провести и записать non-production rehearsal:

1. получить конкретный backup без вывода credentials;
2. создать пустой disposable PostgreSQL cluster с совместимой major version;
3. выполнить `pg_restore`, проверить миграции и `GET /health/ready` через локальный reverse proxy;
4. зафиксировать checksum backup, duration, результат readiness и cleanup disposable resources;
5. удалить disposable cluster и убедиться, что production volume не был затронут.

## Требуемая ручная design review

Human reviewer должен утвердить: реальный owned domain и DNS owner; VPS/deploy, backup и incident
owners; SSH source allowlist; secret injection path; backup retention/encryption/key custody и
RPO/RTO; приемлемость Caddy ACME; а также migration compatibility/restore authority. После этого
можно открыть OPS-001-D и выполнить отдельные ручные staging inputs. До такой записи статус
production launch — не passed, а реальные VPS, DNS, production registry policy, firewall changes и
deployment остаются вне scope.
