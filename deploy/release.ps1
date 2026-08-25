[CmdletBinding()]
param(
    [ValidateSet('preflight', 'deploy', 'rollback')]
    [string]$Mode = 'preflight',
    [Parameter(Mandatory = $true)]
    [ValidatePattern('@sha256:[0-9a-fA-F]{64}$')]
    [string]$Image,
    [string]$EnvironmentFile = '',
    [string]$ReadinessUrl = '',
    [string]$BackupDirectory = '',
    [string]$ProjectName = 'rockserver',
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$composeFile = Join-Path $PSScriptRoot 'compose.yaml'
$productionOverride = Join-Path $PSScriptRoot 'compose.production.yaml'

function Fail([string]$Message) {
    throw $Message
}

function Invoke-Compose([string[]]$Arguments, [hashtable]$EnvironmentOverrides = @{}) {
    $saved = @{}
    foreach ($key in $EnvironmentOverrides.Keys) {
        $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, [string]$EnvironmentOverrides[$key], 'Process')
    }
    try {
        & docker @composeArguments @Arguments
        if ($LASTEXITCODE -ne 0) {
            Fail "Docker Compose operation failed (exit code $LASTEXITCODE)."
        }
    } finally {
        foreach ($key in $EnvironmentOverrides.Keys) {
            [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process')
        }
    }
}

function Test-ProductionPorts($Config) {
    foreach ($serviceName in @('postgres', 'rockserver')) {
        $service = $Config.services.$serviceName
        if ($null -ne $service.ports -and @($service.ports).Count -gt 0) {
            Fail "Production Compose must not publish $serviceName ports."
        }
    }
    $caddyPorts = @($Config.services.caddy.ports | ForEach-Object {
        if ($null -ne $_.published -and $null -ne $_.target) {
            "{0}:{1}" -f $_.published, $_.target
        } else {
            [string]$_
        }
    })
    if ($caddyPorts.Count -ne 2 -or ($caddyPorts -notcontains '80:80') -or ($caddyPorts -notcontains '443:443')) {
        Fail 'Production Compose must publish only Caddy ports 80 and 443.'
    }
}

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Fail 'Docker CLI is required for the release preflight.'
}
if (-not (Test-Path -LiteralPath $composeFile) -or -not (Test-Path -LiteralPath $productionOverride)) {
    Fail 'Production Compose files are missing.'
}

if ([string]::IsNullOrWhiteSpace($EnvironmentFile)) {
    $EnvironmentFile = if ($DryRun -or $Mode -eq 'preflight') {
        Join-Path $repoRoot '.env.example'
    } else {
        Fail 'Provide -EnvironmentFile for deploy or rollback. It must live outside the repository.'
    }
}
$script:environmentFile = (Resolve-Path -LiteralPath $EnvironmentFile).Path
$repoFullPath = (Resolve-Path -LiteralPath $repoRoot).Path.TrimEnd('\') + '\'
if (-not $DryRun -and $script:environmentFile.StartsWith($repoFullPath, [StringComparison]::OrdinalIgnoreCase)) {
    Fail 'Production environment files must live outside the repository.'
}

$composeArguments = @(
    'compose', '--project-name', $ProjectName, '--env-file', $script:environmentFile,
    '--file', $composeFile, '--file', $productionOverride
)

$savedImage = [Environment]::GetEnvironmentVariable('ROCKSERVER_IMAGE', 'Process')
[Environment]::SetEnvironmentVariable('ROCKSERVER_IMAGE', $Image, 'Process')
try {
    $configJson = & docker @composeArguments config --format json 2>$null | Out-String
    if ($LASTEXITCODE -ne 0) {
        Fail 'Production Compose rendering failed.'
    }
} finally {
    [Environment]::SetEnvironmentVariable('ROCKSERVER_IMAGE', $savedImage, 'Process')
}
$config = $configJson | ConvertFrom-Json
Test-ProductionPorts $config

if ($Mode -eq 'preflight' -or $DryRun) {
    Write-Host "Release preflight passed for immutable image reference $Image."
    if ($DryRun) {
        Write-Host 'Dry-run completed without starting containers, creating backups, or contacting a deployment target.'
    }
    return
}

if ([string]::IsNullOrWhiteSpace($ReadinessUrl)) {
    Fail 'Provide -ReadinessUrl for deploy or rollback; it must be the approved HTTPS health URL.'
}
try {
    $readinessUri = [Uri]$ReadinessUrl
} catch {
    Fail 'ReadinessUrl must be a valid HTTPS URL.'
}
if ($readinessUri.Scheme -ne 'https') {
    Fail 'ReadinessUrl must use HTTPS for deploy or rollback.'
}
if ($Mode -eq 'deploy') {
    if ([string]::IsNullOrWhiteSpace($BackupDirectory)) {
        Fail 'Provide -BackupDirectory for deploy; backups must be stored outside the repository.'
    }
    $backupRoot = [IO.Path]::GetFullPath($BackupDirectory)
    if ($backupRoot.StartsWith($repoFullPath, [StringComparison]::OrdinalIgnoreCase)) {
        Fail 'BackupDirectory must be outside the repository.'
    }
    New-Item -ItemType Directory -Path $backupRoot -Force | Out-Null
    $backupName = "rockserver-{0}.dump" -f (Get-Date -Format 'yyyyMMdd-HHmmssZ')
    $backupPath = Join-Path $backupRoot $backupName
    $containerDumpPath = "/tmp/rockserver-release-$([Guid]::NewGuid().ToString('N')).dump"
    $dumpCommand = 'PGPASSWORD="$POSTGRES_PASSWORD" pg_dump --format=custom --file=' +
        $containerDumpPath + ' --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"'

    Invoke-Compose -Arguments @('up', '--detach', '--wait', 'postgres')
    Invoke-Compose -Arguments @('exec', '--no-TTY', 'postgres', 'sh', '-c', $dumpCommand)
    $postgresContainer = (& docker @composeArguments ps --quiet postgres 2>$null | Select-Object -First 1).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($postgresContainer)) {
        Fail 'Could not resolve the PostgreSQL container for backup export.'
    }
    & docker cp "${postgresContainer}:$containerDumpPath" $backupPath *> $null
    $copyExitCode = $LASTEXITCODE
    & docker @composeArguments exec --no-TTY postgres rm -f $containerDumpPath *> $null
    if ($copyExitCode -ne 0) {
        Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue
        Fail 'PostgreSQL backup failed; the release was not started.'
    }
    $hash = (Get-FileHash -LiteralPath $backupPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $record = [ordered]@{
        image = $Image
        backup_file = $backupName
        backup_sha256 = $hash
        created_utc = (Get-Date).ToUniversalTime().ToString('o')
        readiness_url = $ReadinessUrl
    }
    $recordPath = [IO.Path]::ChangeExtension($backupPath, '.json')
    $record | ConvertTo-Json | Set-Content -LiteralPath $recordPath -Encoding utf8NoBOM
    Write-Host "Backup completed; SHA-256 recorded in $recordPath."
    Invoke-Compose -Arguments @('up', '--detach', '--no-build', '--wait', 'rockserver', 'caddy') -EnvironmentOverrides @{ ROCKSERVER_IMAGE = $Image }
} elseif ($Mode -eq 'rollback') {
    Invoke-Compose -Arguments @('up', '--detach', '--no-build', '--wait', 'rockserver', 'caddy') -EnvironmentOverrides @{ ROCKSERVER_IMAGE = $Image }
}

$response = Invoke-WebRequest -UseBasicParsing -Uri $ReadinessUrl -TimeoutSec 30
if ($response.StatusCode -ne 200) {
    Fail "Readiness check returned HTTP $($response.StatusCode)."
}
Write-Host "Release $Mode passed the external readiness gate with HTTP 200."
