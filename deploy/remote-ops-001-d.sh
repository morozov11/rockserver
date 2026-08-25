#!/usr/bin/env bash
# Root-side staging bootstrap/deploy entry point. Secrets are accepted only in protected files.
set -euo pipefail

action="${1:?action required}"
release_root="/opt/rockserver"
env_file="$release_root/release.env"
host_log_dir="/home/rockserver/logs"

fail() { printf '%s\n' "$1" >&2; exit 1; }
require_root() { [ "$(id -u)" -eq 0 ] || fail 'remote operation must run through sudo'; }
validate_stage() {
  local stage="$1"
  [[ "$stage" =~ ^/tmp/rockserver-ops001d-[0-9a-f]{32}$ ]] || fail 'unsafe remote staging path'
  [ -d "$stage" ] && [ ! -L "$stage" ] || fail 'remote staging directory is missing or unsafe'
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
  if grep -Ev '^(ROCKSERVER_DOMAIN|OPS001D_CATALOG_VERSION|OPS001D_CATALOG_COUNT|ROCKSERVER_SEMANTIC_PROVIDER|ROCKSERVER_ONNX_ASSET_DIR|ROCKSERVER_ONNX_MODEL_PATH|ROCKSERVER_ONNX_TOKENIZER_PATH|ORT_DYLIB_PATH|YANDEX_AI_API_KEY|YANDEX_FOLDER_ID|YANDEX_SPEECHKIT_API_KEY|YANDEX_SPEECHKIT_FOLDER_ID)=[^[:cntrl:]]*$' "$stage/owner.env" | grep -q .; then
    fail 'owner.env contains a non-allowlisted or malformed entry'
  fi
  [ "$(wc -l < "$stage/owner.env")" -ge 3 ] || fail 'owner.env entries are not newline-separated'
  install -m 0640 "$stage/compose.yaml" "$release_root/compose.yaml"
  install -m 0640 "$stage/compose.production.yaml" "$release_root/compose.production.yaml"
  install -m 0640 "$stage/Caddyfile.production.template" "$release_root/Caddyfile.production.template"
  install -m 0600 "$stage/onnx-assets.json" "$release_root/onnx-assets.json"
  touch "$env_file"; chmod 0600 "$env_file"
  clean="$(mktemp "$release_root/.release.env.XXXXXX")"
  awk -F= '!/^(ROCKSERVER_DOMAIN|OPS001D_CATALOG_VERSION|OPS001D_CATALOG_COUNT|ROCKSERVER_SEMANTIC_PROVIDER|ROCKSERVER_ONNX_ASSET_DIR|ROCKSERVER_ONNX_MODEL_PATH|ROCKSERVER_ONNX_TOKENIZER_PATH|ORT_DYLIB_PATH|YANDEX_AI_API_KEY|YANDEX_FOLDER_ID|YANDEX_SPEECHKIT_API_KEY|YANDEX_SPEECHKIT_FOLDER_ID)=/' "$env_file" > "$clean"
  cat "$stage/owner.env" >> "$clean"
  chmod 0600 "$clean"; mv "$clean" "$env_file"
}
bootstrap() {
  local stage="${1:?stage required}" deploy_user="${2:?deploy user required}" password rule
  require_root; validate_stage "$stage"
  [[ "$deploy_user" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || fail 'unsafe deploy user'
  install_docker_if_requested
  install -d -m 0750 "$release_root" "$release_root/backups" "$release_root/releases" "$release_root/assets/onnx"
  ensure_host_log_dir
  install_owner_files "$stage"
  write_or_keep_secret POSTGRES_PASSWORD
  write_or_keep_secret ROCKSERVER_API_BEARER_TOKEN
  if ! grep -q '^POSTGRES_DB=' "$env_file"; then
    password="$(sed -n 's/^POSTGRES_PASSWORD=//p' "$env_file" | head -n1)"
    printf 'POSTGRES_DB=rockserver\nPOSTGRES_USER=rockserver\nDATABASE_URL=postgres://rockserver:%s@postgres:5432/rockserver\n' "$password" >> "$env_file"
  fi
  install -m 0750 "$stage/remote-ops-001-d.sh" "$release_root/remote-ops-001-d.sh"
  rule="$(mktemp /etc/sudoers.d/rockserver-deploy.XXXXXX)"
  printf '%s ALL=(root) NOPASSWD: %s deploy *\n' "$deploy_user" "$release_root/remote-ops-001-d.sh" > "$rule"
  chmod 0440 "$rule"
  visudo -cf "$rule" >/dev/null || fail 'generated least-privilege sudo rule failed validation'
  mv "$rule" /etc/sudoers.d/rockserver-deploy
  chmod 0600 "$env_file"
  rm -rf -- "$stage"
  printf '%s\n' 'Bootstrap completed. Key login and a command-scoped non-interactive deploy sudo rule are installed. Review firewall manually: restrict TCP 22; expose only 80/443; never expose 3000/5432. SSH password-login policy is unchanged.'
}
validate_artifact() {
  local stage="$1" image="$2" commit="$3" archive_hash="$4" actual_hash loaded_id label
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
  printf '%s\n' "$loaded_id"
}
download_onnx() {
  local manifest="$release_root/onnx-assets.json"
  [ -f "$manifest" ] || fail 'ONNX manifest is missing'
  python3 - "$manifest" <<'PY'
import hashlib, json, os, pathlib, subprocess, sys
manifest = json.load(open(sys.argv[1], encoding='utf-8'))
if not manifest.get('enabled', False):
    raise SystemExit(0)
if manifest.get('assetDirectory') != '/opt/rockserver/assets/onnx':
    raise SystemExit('enabled ONNX manifest has an unsafe asset directory')
assets = manifest.get('assets', [])
if not assets:
    raise SystemExit('enabled ONNX manifest has no assets')
root = pathlib.Path(manifest['assetDirectory'])
for item in assets:
    name, url, expected = item.get('name',''), item.get('url',''), item.get('sha256','').lower()
    archive_member = item.get('archiveMember')
    if '/' in name or '\\' in name or not url.startswith('https://') or len(expected) != 64 or any(c not in '0123456789abcdef' for c in expected):
        raise SystemExit('ONNX manifest requires exact HTTPS URLs and SHA-256 values')
    path = root / name
    if path.exists() and hashlib.sha256(path.read_bytes()).hexdigest() == expected:
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
        with open(extracted, 'wb') as output:
            subprocess.run(['tar', '-xOzf', str(temp), archive_member], check=True, stdout=output)
        temp.unlink(missing_ok=True)
        os.replace(extracted, path)
    else:
        os.replace(temp, path)
PY
}
deploy() {
  local stage="${1:?stage required}" image="${2:?image required}" commit="${3:?commit required}" archive_hash
  # Accept the former five-argument form while an older root-owned operator
  # script may still be installed on the VPS. The fourth value was its local
  # image ID; current verification uses the archive checksum and revision label.
  if [ "$#" -eq 5 ]; then
    archive_hash="${5:?archive hash required}"
  elif [ "$#" -eq 4 ]; then
    archive_hash="${4:?archive hash required}"
  else
    fail 'deploy requires stage, image, commit, and artifact hash'
  fi
  local backup container backup_hash domain catalog_version catalog_count compose loaded_id
  require_root; validate_stage "$stage"
  [ -f "$env_file" ] || fail 'bootstrap has not provisioned the protected runtime env-file'
  ensure_host_log_dir
  install_owner_files "$stage"
  loaded_id="$(validate_artifact "$stage" "$image" "$commit" "$archive_hash")"
  download_onnx
  compose="docker compose --project-name rockserver --env-file $env_file --file $release_root/compose.yaml --file $release_root/compose.production.yaml"
  ROCKSERVER_IMAGE="$image" $compose config >/dev/null
  ROCKSERVER_IMAGE="$image" $compose up --detach --wait postgres
  backup="$release_root/backups/rockserver-$(date -u +%Y%m%d-%H%M%SZ).dump"
  $compose exec -T postgres sh -c 'PGPASSWORD="$POSTGRES_PASSWORD" pg_dump --format=custom --file=/tmp/ops001d.dump --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"'
  container="$($compose ps -q postgres)"
  docker cp "${container}:/tmp/ops001d.dump" "$backup"
  $compose exec -T postgres rm -f /tmp/ops001d.dump
  backup_hash="$(sha256sum "$backup" | awk '{print $1}')"
  # The pinned importer first applies embedded migrations, then transactionally activates the full catalog.
  ROCKSERVER_IMAGE="$image" $compose run --rm catalog_seed >/dev/null
  ROCKSERVER_IMAGE="$image" $compose up --detach --no-build --wait rockserver caddy
  domain="$(sed -n 's/^ROCKSERVER_DOMAIN=//p' "$env_file" | head -n1)"
  curl --fail --silent --show-error --max-time 30 "https://${domain}/health/ready" >/dev/null
  catalog_version="$(sed -n 's/^OPS001D_CATALOG_VERSION=//p' "$env_file" | head -n1)"
  catalog_count="$(sed -n 's/^OPS001D_CATALOG_COUNT=//p' "$env_file" | head -n1)"
  printf '{"commit":"%s","image_id":"%s","artifact_sha256":"%s","catalog_version":"%s","catalog_count":%s,"backup_sha256":"%s","readiness":"passed"}\n' "$commit" "$loaded_id" "$archive_hash" "$catalog_version" "$catalog_count" "$backup_hash" > "$release_root/releases/current.json"
  chmod 0600 "$release_root/releases/current.json"
  rm -rf -- "$stage"
  printf 'deployed commit=%s image_id=%s catalog=%s count=%s readiness=passed\n' "$commit" "$loaded_id" "$catalog_version" "$catalog_count"
}

case "$action" in
  bootstrap) shift; bootstrap "$@" ;;
  deploy) shift; deploy "$@" ;;
  *) fail 'unsupported action' ;;
esac
