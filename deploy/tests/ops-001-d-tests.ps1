$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Import-Module (Join-Path $root 'deploy/ops-001-d.psm1') -Force
$temp = Join-Path ([IO.Path]::GetTempPath()) ('ops001d-tests-' + [Guid]::NewGuid().ToString('N'))

function Assert-Throws([scriptblock]$Operation, [string]$Failure) {
    try { & $Operation; throw $Failure } catch { if ($_.Exception.Message -eq $Failure) { throw } }
}

New-Item -ItemType Directory -Path $temp | Out-Null
try {
    $git = Join-Path $temp 'repo'
    New-Item -ItemType Directory -Path $git | Out-Null
    & git -C $git init -q
    'deploy/private.inventory.psd1' | Set-Content "$git/.gitignore"
    New-Item -ItemType Directory "$git/deploy" | Out-Null
    '@{}' | Set-Content "$git/deploy/private.inventory.psd1"
    Test-Ops001DPrivateInventory "$git/deploy/private.inventory.psd1" $git | Out-Null
    & git -C $git add -f deploy/private.inventory.psd1
    & git -C $git -c user.email=test@example.invalid -c user.name=test commit -qm test
    Assert-Throws { Test-Ops001DPrivateInventory "$git/deploy/private.inventory.psd1" $git | Out-Null } 'tracked inventory was accepted'

    $inventory = [ordered]@{ SshUser = 'deploy'; SshHost = 'staging.example.test'; Domain = 'api.example.test' }
    Test-Ops001DInventoryValues $inventory | Out-Null
    Assert-Throws { Test-Ops001DInventoryValues ([ordered]@{ SshUser = 'deploy'; SshHost = 'host'; Domain = 'domain'; SshPassword = 'must-not-exist' }) | Out-Null } 'SshPassword was accepted'

    @('YANDEX_AI_API_KEY=secret', 'UNRELATED_SECRET=nope', 'YANDEX_FOLDER_ID=folder', 'YANDEX_SPEECHKIT_API_KEY=speech') | Set-Content "$temp/.env"
    $env = Get-Ops001DAllowedYandexEnvironment "$temp/.env"
    if ($env.Keys.Count -ne 3 -or $env.Contains('UNRELATED_SECRET')) { throw 'allowed-env filter failed' }
    $safe = Format-Ops001DSafeSummary -Environment $env -Commit 'safe' -ImageId 'safe' -Readiness 'passed'
    if ($safe -match 'secret|folder|speech|nope') { throw 'safe summary exposed a secret value' }

    $catalog = [pscustomobject]@{ Version = 'v1-test'; Count = 41 }
    $ownerPath = Join-Path $temp 'owner.env'
    Write-Ops001DOwnerEnvironmentFile -Path $ownerPath -Domain 'api.example.test' -Catalog $catalog -Yandex $env -OnnxEnabled $false
    $ownerLines = [IO.File]::ReadAllLines($ownerPath)
    if ($ownerLines.Count -ne 6 -or $ownerLines[0] -ne 'ROCKSERVER_DOMAIN=api.example.test' -or $ownerLines[1] -ne 'OPS001D_CATALOG_VERSION=v1-test' -or $ownerLines[2] -ne 'OPS001D_CATALOG_COUNT=41') { throw 'owner.env entries were not serialized as distinct correct lines' }
    if (($ownerLines -join "`n") -match 'UNRELATED_SECRET|nope') { throw 'owner.env included a non-allowlisted value' }

    $commit = '0123456789012345678901234567890123456789'
    $imageId = 'sha256:' + ('a' * 64)
    Test-Ops001DArtifactIdentity "rockserver:sha-$commit" $commit $imageId $commit | Out-Null
    Assert-Throws { Test-Ops001DArtifactIdentity 'ghcr.io/example/rockserver:latest' $commit $imageId $commit | Out-Null } 'registry/latest artifact was accepted'
    Assert-Throws { Test-Ops001DArtifactIdentity "rockserver:sha-$commit" $commit $imageId ('f' * 40) | Out-Null } 'mismatched revision label was accepted'

    $bootstrapSsh = Get-Ops001DSshCommand bootstrap 'key path' 'deploy@host' "sudo env OPS001D_INSTALL_DOCKER=1 '/tmp/stage/remote.sh' bootstrap '/tmp/stage' 'deploy'"
    $deploySsh = Get-Ops001DSshCommand deploy 'key path' 'deploy@host' "sudo -n /opt/rockserver/remote-ops-001-d.sh deploy '/tmp/stage' 'rockserver:sha-$commit' '$commit' '$imageId' $('b' * 64)"
    if ($bootstrapSsh -notcontains '-tt' -or ($bootstrapSsh -join ' ') -notmatch 'sudo env' -or ($bootstrapSsh -join ' ') -match 'password') { throw 'bootstrap SSH/sudo construction is not interactive and secret-safe' }
    if ($deploySsh -contains '-tt' -or $deploySsh -notcontains 'BatchMode=yes' -or ($deploySsh -join ' ') -notmatch 'sudo -n') { throw 'normal deploy SSH/sudo construction is not non-interactive' }
    $launcherScript = Get-Content -Raw (Join-Path $root 'deploy/ops-001-d.ps1')
    if ($launcherScript -notmatch 'cmd\.exe /d /c.*ssh-keygen.*-N') { throw 'Windows-safe empty-passphrase SSH key generation is missing' }
    if ($launcherScript -notmatch "DOCKER_HOST -match '\^unix://'" -or $launcherScript -notmatch 'npipe:////\./pipe/docker_engine' -or $launcherScript -notmatch 'docker info --format') { throw 'Windows Docker Engine preflight is missing' }

    '{"enabled":true,"assetDirectory":"/opt/rockserver/assets/onnx","assets":[{"name":"model.onnx","url":"REQUIRED_OFFICIAL_HTTPS_URL","sha256":"REQUIRED_64_HEX_SHA256"}]}' | Set-Content "$temp/onnx.json"
    Assert-Throws { Test-Ops001DOnnxManifest "$temp/onnx.json" | Out-Null } 'incomplete ONNX lock was accepted'
    $onnxLock = Test-Ops001DOnnxManifest "$root/deploy/onnx-assets.lock.json"
    if (-not $onnxLock.enabled -or @($onnxLock.assets).Count -ne 3) { throw 'committed automatic ONNX lock is invalid' }
    $catalogActual = Get-Ops001DFullCatalogMetadata "$root/release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.manifest.json" "$root/release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.sqlite"
    $catalogAgain = Get-Ops001DFullCatalogMetadata "$root/release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.manifest.json" "$root/release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.sqlite"
    if ($catalogActual.Count -lt 16000 -or $catalogActual.Version -ne $catalogAgain.Version -or $catalogActual.Sha256 -ne $catalogAgain.Sha256) { throw 'pinned complete catalog seed input is not stable' }

    $launcher = Get-Content -Raw (Join-Path $root 'deploy/ops-001-d.ps1')
    $remote = Get-Content -Raw (Join-Path $root 'deploy/remote-ops-001-d.sh')
    $compose = Get-Content -Raw (Join-Path $root 'deploy/compose.yaml')
    $productionCompose = Get-Content -Raw (Join-Path $root 'deploy/compose.production.yaml')
    if ($launcher -match 'ghcr|docker push|docker pull|SshPassword') { throw 'launcher still has a registry or password dependency' }
    if ($launcher -notmatch 'docker image save' -or $remote -notmatch 'docker image load') { throw 'registry-free artifact transfer is missing' }
    $backupAt = $remote.IndexOf('pg_dump --format=custom')
    $seedAt = $remote.IndexOf('run --rm catalog_seed')
    $readyAt = $remote.IndexOf('/health/ready')
    if ($backupAt -lt 0 -or $seedAt -le $backupAt -or $readyAt -le $seedAt -or $remote -match 'fixture') { throw 'backup/seed/readiness fail-closed ordering changed' }
    if ($remote -notmatch 'applies embedded migrations' -or $remote -notmatch 'exact HTTPS URLs and SHA-256' -or $launcher -notmatch 'onnx-assets.lock.json') { throw 'migration or automatic ONNX safeguards are missing' }
    if ($compose -notmatch 'import_full_catalog.*backfill_embeddings' -or $compose -notmatch 'ROCKSERVER_SEMANTIC_PROVIDER: onnx-e5-local' -or $compose -notmatch 'ORT_DYLIB_PATH') { throw 'first-deploy ONNX backfill is not wired after the full catalog import' }
    if ($productionCompose -notmatch '/home/rockserver/logs:/var/log/rockserver' -or $remote -notmatch 'host_log_dir="/home/rockserver/logs"' -or $remote -notmatch 'ensure_host_log_dir') { throw 'persistent host log directory is not wired for production' }

    Write-Host 'OPS-001-D local tests passed: registry-free artifact identity, password-free inventory, secret-safe env/summary, TTY/sudo construction, seed, and automatic pinned ONNX safeguards.'
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
