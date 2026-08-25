[CmdletBinding()]
param(
    [ValidateSet('bootstrap', 'deploy')][string]$Action = 'deploy',
    [string]$InventoryPath = '',
    [switch]$InstallDocker,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ops-001-d.psm1') -Force
$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($InventoryPath)) { $InventoryPath = Join-Path $PSScriptRoot 'private.inventory.psd1' }

function Initialize-Ops001DLocalDocker {
    # Docker Desktop on Windows listens through a named pipe. A stale WSL/Linux
    # DOCKER_HOST makes the Windows client try unix:///var/run/docker.sock instead.
    if ($env:OS -eq 'Windows_NT' -and $env:DOCKER_HOST -match '^unix://') {
        Write-Host 'Ignoring incompatible Unix DOCKER_HOST for this Windows deployment process.'
        $env:DOCKER_HOST = 'npipe:////./pipe/docker_engine'
    }
    $null = & docker info --format '{{.ServerVersion}}' 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw 'Local Docker Engine is unavailable. Start Docker Desktop and wait for “Engine running”, then retry the deploy command.'
    }
}

$inventoryFile = Test-Ops001DPrivateInventory -Path $InventoryPath -RepositoryRoot $repoRoot
$inventory = Import-PowerShellDataFile -LiteralPath $inventoryFile
Test-Ops001DInventoryValues -Inventory $inventory | Out-Null
$catalog = Get-Ops001DFullCatalogMetadata -ManifestPath (Join-Path $repoRoot 'release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.manifest.json') -CatalogPath (Join-Path $repoRoot 'release/mobile-catalog/rockmobile-extended-2026.08.2-mobile.1.sqlite')
$yandex = Get-Ops001DAllowedYandexEnvironment -Path (Join-Path $repoRoot '.env')
$onnxLock = Join-Path $PSScriptRoot 'onnx-assets.lock.json'
$onnx = Test-Ops001DOnnxManifest -Path $onnxLock
$commit = Get-Ops001DCurrentCommit -RepositoryRoot $repoRoot -AllowDirty:$DryRun
$image = "rockserver:sha-$commit"

if ($DryRun) {
    $summary = Format-Ops001DSafeSummary -Environment $yandex -Commit $commit -ImageId 'not-built-in-dry-run' -Readiness 'not-contacted'
    Write-Host "Dry-run passed: action=$Action catalog=$($catalog.Version) count=$($catalog.Count) $summary"
    return
}

Initialize-Ops001DLocalDocker

$keyDir = Join-Path $PSScriptRoot '.keys'
$key = Join-Path $keyDir 'rockserver_ed25519'
$target = "$($inventory.SshUser)@$($inventory.SshHost)"
if (-not (Test-Path -LiteralPath $key)) {
    New-Item -ItemType Directory -Force -Path $keyDir | Out-Null
    # Windows PowerShell drops an empty native argument, turning `-N ''` into a
    # missing value. Run from the key directory through cmd.exe so `-N ""` is
    # passed to OpenSSH as an actual empty passphrase.
    Push-Location $keyDir
    try {
        if (Get-Command cmd.exe -ErrorAction SilentlyContinue) {
            & cmd.exe /d /c 'ssh-keygen -q -t ed25519 -f rockserver_ed25519 -N ""'
        } else {
            & ssh-keygen -q -t ed25519 -f $key -N ([string]::Empty)
        }
    } finally {
        Pop-Location
    }
    if ($LASTEXITCODE -ne 0) { throw 'Could not generate the ignored deployment SSH key.' }
    Write-Host 'OpenSSH will now prompt interactively for the VPS login password once. No password is read from config, passed in argv, stored, or logged.'
    Get-Content -LiteralPath "$key.pub" | & ssh $target 'umask 077; mkdir -p ~/.ssh; cat >> ~/.ssh/authorized_keys'
    if ($LASTEXITCODE -ne 0) { throw 'Could not install the SSH key through the interactive password prompt.' }
}

$stage = Join-Path ([IO.Path]::GetTempPath()) ("rockserver-ops001d-" + [Guid]::NewGuid().ToString('N'))
$remoteId = 'rockserver-ops001d-' + [Guid]::NewGuid().ToString('N')
$remoteStage = "/tmp/$remoteId"
$nonInteractiveSshOptions = @('-n', '-o', 'BatchMode=yes', '-o', 'StdinNull=yes', '-o', 'ConnectTimeout=15', '-o', 'ServerAliveInterval=10', '-o', 'ServerAliveCountMax=3')
$nonInteractiveScpOptions = @('-o', 'BatchMode=yes', '-o', 'StdinNull=yes', '-o', 'ConnectTimeout=15', '-o', 'ServerAliveInterval=10', '-o', 'ServerAliveCountMax=3')
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Copy-Item (Join-Path $PSScriptRoot 'compose.yaml'), (Join-Path $PSScriptRoot 'compose.production.yaml'), (Join-Path $PSScriptRoot 'Caddyfile.production.template'), (Join-Path $PSScriptRoot 'remote-ops-001-d.sh') -Destination $stage
    Write-Ops001DOwnerEnvironmentFile -Path (Join-Path $stage 'owner.env') -Domain $inventory.Domain -Catalog $catalog -Yandex $yandex -OnnxEnabled ([bool]$onnx.enabled)
    Copy-Item $onnxLock (Join-Path $stage 'onnx-assets.json')

    $imageId = ''
    $archiveHash = ''
    if ($Action -eq 'deploy') {
        & docker build --label "org.opencontainers.image.revision=$commit" --tag $image $repoRoot
        if ($LASTEXITCODE -ne 0) { throw 'Local Docker build failed.' }
        $imageId = (& docker image inspect --format '{{.Id}}' $image).Trim()
        $labelCommit = (& docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' $image).Trim()
        if ($LASTEXITCODE -ne 0) { throw 'Could not inspect the local deployment image.' }
        Test-Ops001DArtifactIdentity -Image $image -Commit $commit -ImageId $imageId -LabelCommit $labelCommit | Out-Null
        $archive = Join-Path $stage 'rockserver-image.tar'
        & docker image save --output $archive $image
        if ($LASTEXITCODE -ne 0) { throw 'Could not create the local image artifact.' }
        $archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    }

    & ssh -i $key @nonInteractiveSshOptions $target "umask 077; mkdir '$remoteStage'"
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the protected remote staging directory.' }
    $uploadFiles = @(
        (Join-Path $stage 'compose.yaml'),
        (Join-Path $stage 'compose.production.yaml'),
        (Join-Path $stage 'Caddyfile.production.template'),
        (Join-Path $stage 'remote-ops-001-d.sh'),
        (Join-Path $stage 'owner.env'),
        (Join-Path $stage 'onnx-assets.json')
    )
    if ($Action -eq 'deploy') { $uploadFiles += (Join-Path $stage 'rockserver-image.tar') }
    & scp -i $key @nonInteractiveScpOptions @uploadFiles "${target}:$remoteStage/"
    if ($LASTEXITCODE -ne 0) { throw 'Secure copy of the deployment bundle to the VPS failed.' }

    if ($Action -eq 'bootstrap') {
        Write-Host 'The VPS may now prompt interactively for sudo. The password is entered only into the remote TTY and is never stored or logged.'
        $remoteCommand = "sudo env OPS001D_INSTALL_DOCKER=$([int][bool]$InstallDocker) bash '$remoteStage/remote-ops-001-d.sh' bootstrap '$remoteStage' '$($inventory.SshUser)'"
        $sshArgs = Get-Ops001DSshCommand -Action bootstrap -KeyPath $key -Target $target -RemoteCommand $remoteCommand
    } else {
        $remoteCommand = "sudo -n /opt/rockserver/remote-ops-001-d.sh deploy '$remoteStage' '$image' '$commit' '$imageId' '$archiveHash'"
        $sshArgs = Get-Ops001DSshCommand -Action deploy -KeyPath $key -Target $target -RemoteCommand $remoteCommand
    }
    & ssh @sshArgs
    if ($LASTEXITCODE -ne 0) {
        if ($Action -eq 'deploy') { throw 'Remote deploy failed. If sudo reported that a password is required, rerun the documented bootstrap command interactively to install/repair the least-privilege deploy rule.' }
        throw 'Remote bootstrap failed.'
    }
    Write-Host (Format-Ops001DSafeSummary -Environment $yandex -Commit $commit -ImageId $imageId -Readiness $(if ($Action -eq 'deploy') { 'passed' } else { 'bootstrap-only' }))
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
