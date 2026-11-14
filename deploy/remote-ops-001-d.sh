#!/usr/bin/env bash
# Root-side staging bootstrap/deploy entry point. Secrets are accepted only in protected files.
set -euo pipefail

action="${1:?action required}"
release_root="/opt/rockserver"
env_file="$release_root/release.env"
host_log_dir="/home/rockserver/logs"
host_station_icon_dir="/home/rockserver/station-icons"
deploy_lock="$release_root/.deploy.lock"

fail() { printf '%s\n' "$1" >&2; exit 1; }
require_root() { [ "$(id -u)" -eq 0 ] || fail 'remote operation must run through sudo'; }
validate_commit() {
  [[ "$1" =~ ^[0-9a-f]{40}$ ]] || fail 'commit must be a full lowercase SHA'
}
validate_stage() {
  local stage="$1"
  [[ "$stage" =~ ^/tmp/rockserver-ops001d-[0-9a-f]{32}$ ]] || fail 'unsafe remote staging path'
  [ -d "$stage" ] && [ ! -L "$stage" ] || fail 'remote staging directory is missing or unsafe'
}
deploy_status_path() {
  printf '%s/releases/deploy-%s.status' "$release_root" "$1"
}
write_deploy_status() {
  local commit="$1" state="$2" status_path
  validate_commit "$commit"
  case "$state" in queued|running|succeeded|failed) ;; *) fail 'unsafe deployment status';; esac
  status_path="$(deploy_status_path "$commit")"
  printf 'status=%s commit=%s log=%s/deploy-%s.log\n' "$state" "$commit" "$host_log_dir" "$commit" > "$status_path"
  chmod 0600 "$status_path"
}
install_docker_if_requested() {
  if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then return; fi
  [ "${OPS001D_INSTALL_DOCKER:-0}" = 1 ] || fail 'Docker Engine with Compose is required. Re-run bootstrap with -InstallDocker after reviewing the documented Ubuntu/Debian path.'
  command -v apt-get >/dev/null 2>&1 || fail 'Automatic Docker installation is implemented only for apt-based Ubuntu/Debian hosts.'
  apt-get update
  # Ubuntu 20.04 packages Compose v2 as docker-compose-v2; newer Debian/Ubuntu
  # releases may instead expose docker-compose-plugin. Try the native v2 package
  # first, then the newer package name, while keeping Docker Engine installation
  # in the same explicit bootstrap step.
  if ! apt-get install -y ca-certificates curl docker.io docker-compose-v2; then
    apt-get install -y ca-certificates curl docker.io docker-compose-plugin || fail 'Docker prerequisite installation failed.'
  fi
  command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 || fail 'Docker Engine with Compose is still unavailable after installation.'
}
ensure_host_log_dir() {
  install -d -m 0750 -o 10001 "$host_log_dir"
}
ensure_host_station_icon_dir() {
  install -d -m 0750 -o 10001 "$host_station_icon_dir"
}
ensure_low_memory_swap() {
  local memory_kib
  memory_kib="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
  [ "${memory_kib:-0}" -ge 1572864 ] && return
  if swapon --noheadings --show=NAME | grep -q .; then return; fi
  [ ! -e /swapfile ] || fail 'low-memory host has an inactive /swapfile; inspect it manually before bootstrap can continue'
  if ! fallocate -l 2G /swapfile; then
    dd if=/dev/zero of=/swapfile bs=1M count=2048 status=none
  fi
  chmod 0600 /swapfile
  mkswap /swapfile >/dev/null
  swapon /swapfile
  grep -qE '^/swapfile[[:space:]]' /etc/fstab || printf '/swapfile none swap sw 0 0\n' >> /etc/fstab
}
write_or_keep_secret() {
  local key="$1" value
  if grep -q "^${key}=" "$env_file" 2>/dev/null; then return; fi
  value="$(openssl rand -hex 32)"
  printf '%s=%s\n' "$key" "$value" >> "$env_file"
}
install_owner_files() {
  local stage="$1" clean
  for file in compose.yaml compose.production.yaml Caddyfile.production.template owner.env; do
    [ -f "$stage/$file" ] && [ ! -L "$stage/$file" ] || fail "deployment bundle is missing $file"
  done
  if grep -Ev '^(ROCKSERVER_DOMAIN|OPS001D_CATALOG_VERSION|OPS001D_CATALOG_COUNT|OPS001D_CATALOG_SHA256|YANDEX_AI_API_KEY|YANDEX_FOLDER_ID|YANDEX_LLM_MODEL|YANDEX_LLM_MODEL_VERSION|YANDEX_LLM_TIMEOUT_MS|YANDEX_SPEECHKIT_API_KEY|YANDEX_SPEECHKIT_FOLDER_ID|YANDEX_HOME_CLIENTID|YANDEX_HOME_SECRET)=[^[:cntrl:]]*$' "$stage/owner.env" | grep -q .; then
    fail 'owner.env contains a non-allowlisted or malformed entry'
  fi
  [ "$(wc -l < "$stage/owner.env")" -ge 3 ] || fail 'owner.env entries are not newline-separated'
  install -m 0640 "$stage/compose.yaml" "$release_root/compose.yaml"
  install -m 0640 "$stage/compose.production.yaml" "$release_root/compose.production.yaml"
  install -m 0640 "$stage/Caddyfile.production.template" "$release_root/Caddyfile.production.template"
  touch "$env_file"; chmod 0600 "$env_file"
  clean="$(mktemp "$release_root/.release.env.XXXXXX")"
  awk -F= '!/^(ROCKSERVER_DOMAIN|OPS001D_CATALOG_VERSION|OPS001D_CATALOG_COUNT|OPS001D_CATALOG_SHA256|YANDEX_AI_API_KEY|YANDEX_FOLDER_ID|YANDEX_LLM_MODEL|YANDEX_LLM_MODEL_VERSION|YANDEX_LLM_TIMEOUT_MS|YANDEX_SPEECHKIT_API_KEY|YANDEX_SPEECHKIT_FOLDER_ID|YANDEX_HOME_CLIENTID|YANDEX_HOME_SECRET)=/' "$env_file" > "$clean"
  cat "$stage/owner.env" >> "$clean"
  chmod 0600 "$clean"; mv "$clean" "$env_file"
}
bootstrap() {
  local stage="${1:?stage required}" deploy_user="${2:?deploy user required}" password rule
  require_root; validate_stage "$stage"
  [[ "$deploy_user" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || fail 'unsafe deploy user'
  install_docker_if_requested
  ensure_low_memory_swap
  install -d -m 0750 "$release_root" "$release_root/backups" "$release_root/releases"
  ensure_host_log_dir
  ensure_host_station_icon_dir
  install_owner_files "$stage"
  write_or_keep_secret POSTGRES_PASSWORD
  write_or_keep_secret ROCKSERVER_API_BEARER_TOKEN
  write_or_keep_secret ROCKSERVER_TRUSTED_PROXY_TOKEN
  if ! grep -q '^POSTGRES_DB=' "$env_file"; then
    password="$(sed -n 's/^POSTGRES_PASSWORD=//p' "$env_file" | head -n1)"
    printf 'POSTGRES_DB=rockserver\nPOSTGRES_USER=rockserver\nDATABASE_URL=postgres://rockserver:%s@postgres:5432/rockserver\n' "$password" >> "$env_file"
  fi
  install -m 0750 "$stage/remote-ops-001-d.sh" "$release_root/remote-ops-001-d.sh"
  rule="$(mktemp /etc/sudoers.d/rockserver-deploy.XXXXXX)"
  printf '%s ALL=(root) NOPASSWD: %s deploy *, %s status *, %s cleanup *\n' "$deploy_user" "$release_root/remote-ops-001-d.sh" "$release_root/remote-ops-001-d.sh" "$release_root/remote-ops-001-d.sh" > "$rule"
  chmod 0440 "$rule"
  visudo -cf "$rule" >/dev/null || fail 'generated least-privilege sudo rule failed validation'
  mv "$rule" /etc/sudoers.d/rockserver-deploy
  chmod 0600 "$env_file"
  rm -rf -- "$stage"
  printf '%s\n' 'Bootstrap completed. Key login and a command-scoped non-interactive deploy sudo rule are installed. Review firewall manually: restrict TCP 22; expose only 80/443; never expose 3000/5432. SSH password-login policy is unchanged.'
}
validate_artifact() {
  local stage="$1" image="$2" commit="$3" archive_hash="$4" expected_id="${5:-}" actual_hash loaded_id label
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail 'commit must be a full lowercase SHA'
  [ "$image" = "rockserver:sha-$commit" ] || fail 'image reference must exactly match the source commit'
  [[ "$archive_hash" =~ ^[0-9a-f]{64}$ ]] || fail 'artifact SHA-256 is invalid'
  [ -f "$stage/rockserver-image.tar" ] && [ ! -L "$stage/rockserver-image.tar" ] || fail 'image artifact is missing or unsafe'
  actual_hash="$(sha256sum "$stage/rockserver-image.tar" | awk '{print $1}')"
  [ "$actual_hash" = "$archive_hash" ] || fail 'transferred image artifact checksum mismatch'
  docker image load --input "$stage/rockserver-image.tar" >/dev/null
  loaded_id="$(docker image inspect --format '{{.Id}}' "$image")"
  label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$image")"
  # Docker Desktop may save an OCI manifest list while a Linux Docker Engine
  # loads the platform image's config ID. The tarball checksum proves byte-for-
  # byte transfer; the revision label binds that verified artifact to commit.
  [[ "$loaded_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'loaded image ID is invalid'
  [ "$label" = "$commit" ] || fail 'loaded image revision label does not match the current commit'
  if [ -n "$expected_id" ]; then
    [[ "$expected_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'portable image ID is invalid'
    [ "$loaded_id" = "$expected_id" ] || fail 'portable image config ID mismatch'
  fi
  printf '%s\n' "$loaded_id"
}
validate_caddy_artifact() {
  local stage="$1" image="$2" commit="$3" archive_hash="$4" actual_hash loaded_id label
  [ "$image" = "rockserver-caddy:sha-$commit" ] || fail 'Caddy image reference must exactly match the source commit'
  [[ "$archive_hash" =~ ^[0-9a-f]{64}$ ]] || fail 'Caddy artifact SHA-256 is invalid'
  [ -f "$stage/rockserver-caddy-image.tar" ] && [ ! -L "$stage/rockserver-caddy-image.tar" ] || fail 'Caddy image artifact is missing or unsafe'
  actual_hash="$(sha256sum "$stage/rockserver-caddy-image.tar" | awk '{print $1}')"
  [ "$actual_hash" = "$archive_hash" ] || fail 'transferred Caddy image artifact checksum mismatch'
  docker image load --input "$stage/rockserver-caddy-image.tar" >/dev/null
  loaded_id="$(docker image inspect --format '{{.Id}}' "$image")"
  label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$image")"
  [[ "$loaded_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'loaded Caddy image ID is invalid'
  [ "$label" = "$commit" ] || fail 'loaded Caddy image revision label does not match the current commit'
}
catalog_marker_metadata() {
  local version count checksum
  version="$(sed -n 's/^OPS001D_CATALOG_VERSION=//p' "$env_file" | head -n1)"
  count="$(sed -n 's/^OPS001D_CATALOG_COUNT=//p' "$env_file" | head -n1)"
  checksum="$(sed -n 's/^OPS001D_CATALOG_SHA256=//p' "$env_file" | head -n1)"
  [[ "$version" =~ ^[A-Za-z0-9._-]{1,120}$ ]] || fail 'catalog version in protected env is invalid'
  [[ "$count" =~ ^[1-9][0-9]*$ ]] || fail 'catalog count in protected env is invalid'
  [[ "$checksum" =~ ^[0-9a-f]{64}$ ]] || fail 'catalog checksum in protected env is invalid'
  printf '%s|%s|%s\n' "$version" "$count" "$checksum"
}
catalog_marker_path() {
  local version="$1"
  printf '%s/releases/catalog-%s.ready' "$release_root" "$version"
}
catalog_seed_is_current() {
  local compose="$1" image="$2" caddy_image="$3" metadata version count checksum marker actual_counts
  metadata="$(catalog_marker_metadata)"
  IFS='|' read -r version count checksum <<< "$metadata"
  marker="$(catalog_marker_path "$version")"
  [ -f "$marker" ] && [ ! -L "$marker" ] || return 1
  grep -Fx "catalog_version=$version" "$marker" >/dev/null || return 1
  grep -Fx "catalog_count=$count" "$marker" >/dev/null || return 1
  grep -Fx "catalog_sha256=$checksum" "$marker" >/dev/null || return 1
  actual_counts="$(ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose exec -T postgres sh -c 'psql -Atq -U "$POSTGRES_USER" -d "$POSTGRES_DB"' 2>/dev/null <<'SQL'
SELECT count(*) FROM stations WHERE retired_at IS NULL;
SQL
)"
  [ "$actual_counts" = "$count" ]
}
write_catalog_marker() {
  local metadata version count checksum marker
  metadata="$(catalog_marker_metadata)"
  IFS='|' read -r version count checksum <<< "$metadata"
  marker="$(catalog_marker_path "$version")"
  printf 'catalog_version=%s\ncatalog_count=%s\ncatalog_sha256=%s\n' "$version" "$count" "$checksum" > "$marker"
  chmod 0600 "$marker"
}
deploy_internal() {
  local stage="${1:?stage required}" image="${2:?image required}" caddy_image="${3:?Caddy image required}" commit="${4:?commit required}" archive_hash="${5:?artifact hash required}" caddy_archive_hash="${6:?Caddy artifact hash required}" portable_image_id="${7:-}" legacy_caddy="${8:-0}"
  local backup_hash domain catalog_version catalog_count compose loaded_id
  require_root; validate_stage "$stage"
  [ -f "$env_file" ] || fail 'bootstrap has not provisioned the protected runtime env-file'
  ensure_host_log_dir
  install_owner_files "$stage"
  loaded_id="$(validate_artifact "$stage" "$image" "$commit" "$archive_hash" "$portable_image_id")"
  if [ "$legacy_caddy" = 1 ]; then
    [[ "$caddy_image" =~ ^[A-Za-z0-9._:/@-]+$ ]] || fail 'existing Caddy image reference is invalid'
  else
    validate_caddy_artifact "$stage" "$caddy_image" "$commit" "$caddy_archive_hash"
  fi
  compose="docker compose --project-name rockserver --env-file $env_file --file $release_root/compose.yaml --file $release_root/compose.production.yaml"
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose config >/dev/null
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose up --detach --wait postgres
  # Deploy-time pg_dump is disabled by operator decision: the small VPS cannot
  # afford a full dump on every release, and backups are a separate owner-
  # managed duty. The summary keeps the field only for format continuity.
  backup_hash='-'
  # Skip completed pinned releases; new releases run import
  # as a separate fail-closed job before readiness is checked.
  if catalog_seed_is_current "$compose" "$image" "$caddy_image"; then
    printf '%s\n' 'catalog seed skipped: exact pinned catalog is already ready'
  else
    # The pinned importer first applies embedded migrations, then transactionally activates the full catalog.
    ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose run --rm catalog_seed >/dev/null
    write_catalog_marker
  fi
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose up --detach --no-build --wait rockserver caddy
  domain="$(sed -n 's/^ROCKSERVER_DOMAIN=//p' "$env_file" | head -n1)"
  curl --fail --silent --show-error --max-time 30 "https://${domain}/health/ready" >/dev/null
  catalog_version="$(sed -n 's/^OPS001D_CATALOG_VERSION=//p' "$env_file" | head -n1)"
  catalog_count="$(sed -n 's/^OPS001D_CATALOG_COUNT=//p' "$env_file" | head -n1)"
  printf '{"commit":"%s","image_id":"%s","artifact_sha256":"%s","catalog_version":"%s","catalog_count":%s,"backup_sha256":"%s","readiness":"passed"}\n' "$commit" "$loaded_id" "$archive_hash" "$catalog_version" "$catalog_count" "$backup_hash" > "$release_root/releases/current.json"
  chmod 0600 "$release_root/releases/current.json"
  rm -rf -- "$stage"
  printf 'deployed commit=%s image_id=%s catalog=%s count=%s readiness=passed\n' "$commit" "$loaded_id" "$catalog_version" "$catalog_count"
}

# The deployment can take several minutes on a small VPS. It must not
# be tied to a caller's SSH session: a lost laptop/network connection must not
# leave a loaded image and seeded database without starting the web service.
submit_deploy() {
  local stage image caddy_image commit archive_hash caddy_archive_hash portable_image_id legacy_caddy
  local existing_commit worker_log pid
  # Accept the former five-argument form while an older root-owned operator
  # script may still be installed on the VPS. Its fourth value is the
  # portable image config ID; the current detached form adds a Caddy image and
  # its artifact hash. The four-argument form is retained for the same
  # migration window, without weakening the validation below.
  case "$#" in
    6)
      stage="$1"; image="$2"; caddy_image="$3"; commit="$4"
      archive_hash="$5"; caddy_archive_hash="$6"; portable_image_id=''; legacy_caddy=0
      ;;
    5)
      stage="$1"; image="$2"; commit="$3"; portable_image_id="$4"; archive_hash="$5"
      caddy_image="$(docker ps --filter label=com.docker.compose.project=rockserver --filter label=com.docker.compose.service=caddy --format '{{.Image}}')"
      [[ -n "$caddy_image" && "$caddy_image" != *$'\n'* ]] || fail 'former deploy form requires an existing Caddy container'
      caddy_archive_hash='legacy-existing'; legacy_caddy=1
      ;;
    4)
      stage="$1"; image="$2"; commit="$3"; archive_hash="$4"; portable_image_id=''
      caddy_image="$(docker ps --filter label=com.docker.compose.project=rockserver --filter label=com.docker.compose.service=caddy --format '{{.Image}}')"
      [[ -n "$caddy_image" && "$caddy_image" != *$'\n'* ]] || fail 'former deploy form requires an existing Caddy container'
      caddy_archive_hash='legacy-existing'; legacy_caddy=1
      ;;
    *)
      fail 'deploy requires stage, image, commit, and artifact hash'
      ;;
  esac
  require_root; validate_stage "$stage"; validate_commit "$commit"
  [ -f "$env_file" ] || fail 'bootstrap has not provisioned the protected runtime env-file'
  install -d -m 0750 "$release_root/releases"
  ensure_host_log_dir
  worker_log="$host_log_dir/deploy-$commit.log"
  if ! mkdir "$deploy_lock" 2>/dev/null; then
    existing_commit="$(cat "$deploy_lock/commit" 2>/dev/null || true)"
    if [ "$existing_commit" = "$commit" ]; then
      # A caller may safely rerun the PowerShell command after an SSH loss.
      # The original worker still owns its trusted staging directory; discard
      # only this newly uploaded, validated temporary bundle.
      rm -rf -- "$stage"
      write_deploy_status "$commit" running
      printf 'status=running commit=%s reattached=true\n' "$commit"
      return
    fi
    fail 'another deployment is already running; wait for it to finish before submitting a different commit'
  fi
  [ -f "$stage/remote-ops-001-d.sh" ] && [ ! -L "$stage/remote-ops-001-d.sh" ] || { rm -rf -- "$deploy_lock"; fail 'deployment bundle operator script is missing or unsafe'; }
  # Activate the validated operator script before starting the worker. This
  # keeps retention and other root-side deployment fixes from waiting for a
  # separate bootstrap run.
  if ! install -m 0750 "$stage/remote-ops-001-d.sh" "$release_root/remote-ops-001-d.sh"; then
    rm -rf -- "$deploy_lock"
    fail 'could not activate the deployment bundle operator script'
  fi
  printf '%s\n' "$commit" > "$deploy_lock/commit"
  write_deploy_status "$commit" queued
  # nohup detaches the worker from SSH's SIGHUP.  All output goes to a
  # protected host log, while the status file contains only safe metadata.
  nohup "$release_root/remote-ops-001-d.sh" deploy-worker "$stage" "$image" "$caddy_image" "$commit" "$archive_hash" "$caddy_archive_hash" "$portable_image_id" "$legacy_caddy" > "$worker_log" 2>&1 < /dev/null &
  pid="$!"
  printf '%s\n' "$pid" > "$deploy_lock/pid"
  printf 'status=queued commit=%s pid=%s\n' "$commit" "$pid"
}

deploy_worker() {
  local stage="${1:?stage required}" commit="${4:?commit required}" exit_code=0
  require_root; validate_stage "$stage"; validate_commit "$commit"
  [ -d "$deploy_lock" ] || fail 'deployment worker lock is missing'
  write_deploy_status "$commit" running
  # Run the actual deployment in a fresh shell so its `set -e` behavior stays
  # fail-closed even though this wrapper needs to record an outcome.
  if "$release_root/remote-ops-001-d.sh" deploy-internal "$@"; then
    write_deploy_status "$commit" succeeded
  else
    exit_code="$?"
    write_deploy_status "$commit" failed
    rm -rf -- "$stage"
  fi
  rm -rf -- "$deploy_lock"
  return "$exit_code"
}

deploy_status() {
  local commit="${1:?commit required}" status_path
  require_root; validate_commit "$commit"
  status_path="$(deploy_status_path "$commit")"
  if [ -f "$status_path" ] && [ ! -L "$status_path" ]; then
    cat "$status_path"
    return
  fi
  printf 'status=unknown commit=%s\n' "$commit"
}

validate_uuid() {
  [[ "$1" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || fail 'cleanup target must be one lowercase UUID from preview'
}

cleanup_operator() {
  local mode="${1:-}" target id confirmation expected image caddy_image rockserver_container caddy_container compose
  require_root
  [ -f "$env_file" ] || fail 'bootstrap has not provisioned the protected runtime env-file'
  if [ "$mode" = 'preview' ] && [ "$#" -eq 1 ]; then
    target='preview'
  elif [ "$mode" = 'apply' ] && [ "$#" -eq 6 ] && [ "$3" = '--id' ] && [ "$5" = '--confirm' ]; then
    target="${2:?cleanup action required}"
    id="$4"
    confirmation="$6"
    # The remote wrapper repeats the same exact-target checks as the binary so
    # the sudo rule cannot be used to pass arbitrary commands or wildcards.
    if [[ "$target" != account && "$target" != device && "$target" != credential ]]; then
      fail 'cleanup action must be account, device, or credential'
    fi
    validate_uuid "$id"
    case "$target" in
      account) expected="DEACTIVATE ACCOUNT $id" ;;
      device) expected="REVOKE DEVICE $id" ;;
      credential) expected="REVOKE CREDENTIAL $id" ;;
    esac
    [ "$confirmation" = "$expected" ] || fail 'confirmation does not match the exact target'
  else
    fail 'cleanup usage is preview or apply <account|device|credential> --id <UUID> --confirm <exact phrase>'
  fi

  rockserver_container="$(docker ps --filter label=com.docker.compose.project=rockserver --filter label=com.docker.compose.service=rockserver --format '{{.ID}}')"
  caddy_container="$(docker ps --filter label=com.docker.compose.project=rockserver --filter label=com.docker.compose.service=caddy --format '{{.ID}}')"
  [[ -n "$rockserver_container" && "$rockserver_container" != *$'\n'* ]] || fail 'exactly one running RockServer container is required'
  [[ -n "$caddy_container" && "$caddy_container" != *$'\n'* ]] || fail 'exactly one running Caddy container is required'
  image="$(docker inspect --format '{{.Config.Image}}' "$rockserver_container")"
  caddy_image="$(docker inspect --format '{{.Config.Image}}' "$caddy_container")"
  [ -n "$image" ] && [ -n "$caddy_image" ] || fail 'deployed image identity is unavailable'
  compose="docker compose --project-name rockserver --env-file $env_file --file $release_root/compose.yaml --file $release_root/compose.production.yaml"
  if [ "$target" = 'preview' ]; then
    ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose run --rm --no-deps -e ROCKSERVER_CLEANUP_ENV=staging --entrypoint /usr/local/bin/account_cleanup rockserver preview
  else
    ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose run --rm --no-deps -e ROCKSERVER_CLEANUP_ENV=staging --entrypoint /usr/local/bin/account_cleanup rockserver apply "$target" --id "$id" --confirm "$confirmation"
  fi
}

case "$action" in
  bootstrap) shift; bootstrap "$@" ;;
  deploy) shift; submit_deploy "$@" ;;
  deploy-worker) shift; deploy_worker "$@" ;;
  deploy-internal) shift; deploy_internal "$@" ;;
  status) shift; deploy_status "$@" ;;
  cleanup) shift; cleanup_operator "$@" ;;
  *) fail 'unsupported action' ;;
esac
