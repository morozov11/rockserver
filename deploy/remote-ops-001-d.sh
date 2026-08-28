#!/usr/bin/env bash
# Root-side staging bootstrap/deploy entry point. Secrets are accepted only in protected files.
set -euo pipefail

action="${1:?action required}"
release_root="/opt/rockserver"
env_file="$release_root/release.env"
host_log_dir="/home/rockserver/logs"
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
  for file in compose.yaml compose.production.yaml Caddyfile.production.template owner.env onnx-assets.json; do
    [ -f "$stage/$file" ] && [ ! -L "$stage/$file" ] || fail "deployment bundle is missing $file"
  done
  if grep -Ev '^(ROCKSERVER_DOMAIN|OPS001D_CATALOG_VERSION|OPS001D_CATALOG_COUNT|OPS001D_CATALOG_SHA256|ROCKSERVER_SEMANTIC_PROVIDER|ROCKSERVER_ONNX_ASSET_DIR|ROCKSERVER_ONNX_MODEL_PATH|ROCKSERVER_ONNX_TOKENIZER_PATH|ORT_DYLIB_PATH|YANDEX_AI_API_KEY|YANDEX_FOLDER_ID|YANDEX_SPEECHKIT_API_KEY|YANDEX_SPEECHKIT_FOLDER_ID)=[^[:cntrl:]]*$' "$stage/owner.env" | grep -q .; then
    fail 'owner.env contains a non-allowlisted or malformed entry'
  fi
  [ "$(wc -l < "$stage/owner.env")" -ge 3 ] || fail 'owner.env entries are not newline-separated'
  install -m 0640 "$stage/compose.yaml" "$release_root/compose.yaml"
  install -m 0640 "$stage/compose.production.yaml" "$release_root/compose.production.yaml"
  install -m 0640 "$stage/Caddyfile.production.template" "$release_root/Caddyfile.production.template"
  install -m 0600 "$stage/onnx-assets.json" "$release_root/onnx-assets.json"
  touch "$env_file"; chmod 0600 "$env_file"
  clean="$(mktemp "$release_root/.release.env.XXXXXX")"
  awk -F= '!/^(ROCKSERVER_DOMAIN|OPS001D_CATALOG_VERSION|OPS001D_CATALOG_COUNT|OPS001D_CATALOG_SHA256|ROCKSERVER_SEMANTIC_PROVIDER|ROCKSERVER_ONNX_ASSET_DIR|ROCKSERVER_ONNX_MODEL_PATH|ROCKSERVER_ONNX_TOKENIZER_PATH|ORT_DYLIB_PATH|YANDEX_AI_API_KEY|YANDEX_FOLDER_ID|YANDEX_SPEECHKIT_API_KEY|YANDEX_SPEECHKIT_FOLDER_ID)=/' "$env_file" > "$clean"
  cat "$stage/owner.env" >> "$clean"
  chmod 0600 "$clean"; mv "$clean" "$env_file"
}
bootstrap() {
  local stage="${1:?stage required}" deploy_user="${2:?deploy user required}" password rule
  require_root; validate_stage "$stage"
  [[ "$deploy_user" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || fail 'unsafe deploy user'
  install_docker_if_requested
  ensure_low_memory_swap
  install -d -m 0750 "$release_root" "$release_root/backups" "$release_root/releases" "$release_root/assets/onnx"
  ensure_host_log_dir
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
download_onnx() {
  local manifest="$release_root/onnx-assets.json"
  [ -f "$manifest" ] || fail 'ONNX manifest is missing'
  python3 - "$manifest" <<'PY'
import hashlib, json, os, pathlib, posixpath, subprocess, tarfile, sys
manifest = json.load(open(sys.argv[1], encoding='utf-8'))
if not manifest.get('enabled', False):
    raise SystemExit(0)
if manifest.get('assetDirectory') != '/opt/rockserver/assets/onnx':
    raise SystemExit('enabled ONNX manifest has an unsafe asset directory')
assets = manifest.get('assets', [])
if not assets:
    raise SystemExit('enabled ONNX manifest has no assets')
root = pathlib.Path(manifest['assetDirectory'])
root.mkdir(parents=True, exist_ok=True)
# The service runs as UID 10001 inside the container. The host-side bootstrap
# may leave this bind-mounted directory owned by root, but it must remain
# traversable by the container user.
os.chmod(root, 0o755)
for item in assets:
    name, url, expected = item.get('name',''), item.get('url',''), item.get('sha256','').lower()
    archive_member = item.get('archiveMember')
    if '/' in name or '\\' in name or not url.startswith('https://') or len(expected) != 64 or any(c not in '0123456789abcdef' for c in expected):
        raise SystemExit('ONNX manifest requires exact HTTPS URLs and SHA-256 values')
    path = root / name
    if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == expected:
        os.chmod(path, 0o755 if name == 'libonnxruntime.so' else 0o644)
        continue
    temp = path.with_suffix(path.suffix + '.download')
    subprocess.run(['curl', '--fail', '--location', '--proto', '=https', '--silent', '--show-error', '--output', str(temp), url], check=True)
    if hashlib.sha256(temp.read_bytes()).hexdigest() != expected:
        temp.unlink(missing_ok=True)
        raise SystemExit('ONNX SHA-256 mismatch; refusing asset')
    if archive_member:
        expected_member = 'onnxruntime-linux-x64-1.23.2/lib/libonnxruntime.so'
        if name != 'libonnxruntime.so' or archive_member != expected_member:
            temp.unlink(missing_ok=True)
            raise SystemExit('ONNX runtime archive member is invalid')
        extracted = path.with_suffix(path.suffix + '.partial')
        with tarfile.open(temp, 'r:gz') as archive:
            member = archive.getmember(archive_member)
            for _ in range(8):
                if not (member.issym() or member.islnk()):
                    break
                if posixpath.isabs(member.linkname):
                    raise SystemExit('ONNX runtime archive member has an unsafe absolute symlink')
                target = posixpath.normpath(posixpath.join(posixpath.dirname(member.name), member.linkname))
                if target == '..' or target.startswith('../'):
                    raise SystemExit('ONNX runtime archive member escapes its archive directory')
                member = archive.getmember(target)
            else:
                raise SystemExit('ONNX runtime archive has too many symlink levels')
            if not member.isfile():
                raise SystemExit('ONNX runtime archive member is not a regular file')
            source = archive.extractfile(member)
            if source is None:
                raise SystemExit('ONNX runtime archive member could not be read')
            with source, open(extracted, 'wb') as output:
                output.write(source.read())
        if extracted.stat().st_size == 0:
            extracted.unlink(missing_ok=True)
            raise SystemExit('ONNX runtime archive member is empty')
        temp.unlink(missing_ok=True)
        os.replace(extracted, path)
    else:
        os.replace(temp, path)
    os.chmod(path, 0o755 if name == 'libonnxruntime.so' else 0o644)
PY
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
  actual_counts="$(ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose exec -T postgres sh -c 'psql -Atq -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT (SELECT count(*) FROM stations), (SELECT count(*) FROM station_embeddings);"')"
  [ "$actual_counts" = "$count|$count" ]
}
write_catalog_marker() {
  local metadata version count checksum marker
  metadata="$(catalog_marker_metadata)"
  IFS='|' read -r version count checksum <<< "$metadata"
  marker="$(catalog_marker_path "$version")"
  printf 'catalog_version=%s\ncatalog_count=%s\ncatalog_sha256=%s\n' "$version" "$count" "$checksum" > "$marker"
  chmod 0600 "$marker"
}
prune_backups() {
  local retained_backup
  retained_backup="${1:?retained backup required}"
  [ -f "$retained_backup" ] || fail 'refusing to prune backups before the new backup is available'
  find -P "$release_root/backups" -mindepth 1 -maxdepth 1 -type f -name 'rockserver-*.dump' ! -path "$retained_backup" -delete
}
deploy_internal() {
  local stage="${1:?stage required}" image="${2:?image required}" caddy_image="${3:?Caddy image required}" commit="${4:?commit required}" archive_hash="${5:?artifact hash required}" caddy_archive_hash="${6:?Caddy artifact hash required}" portable_image_id="${7:-}" legacy_caddy="${8:-0}"
  local backup container backup_hash domain catalog_version catalog_count compose loaded_id
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
  download_onnx
  compose="docker compose --project-name rockserver --env-file $env_file --file $release_root/compose.yaml --file $release_root/compose.production.yaml"
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose config >/dev/null
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose up --detach --wait postgres
  backup="$release_root/backups/rockserver-$(date -u +%Y%m%d-%H%M%SZ).dump"
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose exec -T postgres sh -c 'PGPASSWORD="$POSTGRES_PASSWORD" pg_dump --format=custom --file=/tmp/ops001d.dump --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"'
  container="$(ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose ps -q postgres)"
  docker cp "${container}:/tmp/ops001d.dump" "$backup"
  ROCKSERVER_IMAGE="$image" ROCKSERVER_CADDY_IMAGE="$caddy_image" $compose exec -T postgres rm -f /tmp/ops001d.dump
  backup_hash="$(sha256sum "$backup" | awk '{print $1}')"
  [ "${#backup_hash}" -eq 64 ] || fail 'new PostgreSQL backup checksum is invalid; previous backups were kept'
  # Keep exactly one on-VPS deploy rollback point.  This happens only after a
  # complete new dump was copied and checksummed, so a failed backup never
  # erases the last recoverable dump.
  prune_backups "$backup"
  # A full release replacement deletes/rebuilds derived vectors.  Skip that
  # costly operation only when the exact pinned release was already completed
  # and PostgreSQL still contains every station and every embedding.
  if catalog_seed_is_current "$compose" "$image" "$caddy_image"; then
    printf '%s\n' 'catalog seed skipped: exact pinned catalog and embeddings are already ready'
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

# The initial ONNX backfill can take many minutes on a small VPS.  It must not
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
