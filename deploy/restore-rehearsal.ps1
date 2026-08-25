[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BackupFile,
    [string]$RockServerImage = 'rockserver:local',
    [string]$PostgresImage = 'pgvector/pgvector:pg17@sha256:7ae6051efd0e60444282c27c7e141af07f322ce033300e727a49c3dd11075e38',
    [switch]$DryRun,
    [switch]$KeepArtifacts
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    throw $Message
}

function Invoke-Docker([string[]]$Arguments) {
    & docker @Arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "Docker operation failed (exit code $LASTEXITCODE)."
    }
}

function Wait-Postgres([string]$Container) {
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        & docker exec $Container pg_isready -U rockserver -d rockserver *> $null
        if ($LASTEXITCODE -eq 0) {
            return
        }
        Start-Sleep -Seconds 2
    }
    Fail 'Disposable PostgreSQL did not become ready.'
}

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Fail 'Docker CLI is required for the restore rehearsal.'
}
$resolvedBackup = (Resolve-Path -LiteralPath $BackupFile).Path
if ((Get-Item -LiteralPath $resolvedBackup).Length -eq 0) {
    Fail 'Backup file is empty.'
}
$hash = (Get-FileHash -LiteralPath $resolvedBackup -Algorithm SHA256).Hash.ToLowerInvariant()
if ($DryRun) {
    Write-Host "Restore rehearsal dry-run accepted backup $resolvedBackup (SHA-256 $hash)."
    return
}

$suffix = [Guid]::NewGuid().ToString('N').Substring(0, 12)
$network = "rockserver-ops001c-restore-$suffix"
$dbContainer = "rockserver-ops001c-db-$suffix"
$appContainer = "rockserver-ops001c-app-$suffix"
$databasePassword = [Guid]::NewGuid().ToString('N')
$apiToken = [Guid]::NewGuid().ToString('N') + [Guid]::NewGuid().ToString('N')
$created = @()

try {
    Invoke-Docker @('network', 'create', $network)
    $created += $network
    Invoke-Docker @(
        'run', '--detach', '--name', $dbContainer, '--network', $network, '--network-alias', 'postgres',
        '--env', 'POSTGRES_DB=rockserver', '--env', 'POSTGRES_USER=rockserver',
        '--env', "POSTGRES_PASSWORD=$databasePassword", $PostgresImage
    )
    $created += $dbContainer
    Wait-Postgres $dbContainer
    Invoke-Docker @('cp', $resolvedBackup, "${dbContainer}:/tmp/rockserver-restore.dump")
    $restoreCommand = 'PGPASSWORD="$POSTGRES_PASSWORD" pg_restore --clean --if-exists --no-owner --no-privileges --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" /tmp/rockserver-restore.dump'
    Invoke-Docker @('exec', $dbContainer, 'sh', '-c', $restoreCommand)
    $checkCommand = 'PGPASSWORD="$POSTGRES_PASSWORD" psql --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" --command="SELECT 1 FROM stations LIMIT 1;"'
    Invoke-Docker @('exec', $dbContainer, 'sh', '-c', $checkCommand)

    Invoke-Docker @(
        'run', '--detach', '--name', $appContainer, '--network', $network,
        '--env', 'ROCKSERVER_BIND_ADDR=0.0.0.0:3000', '--env', 'ROCKSERVER_LOG_DIR=/tmp',
        '--env', 'RUST_LOG=warn', '--env', "DATABASE_URL=postgres://rockserver:$databasePassword@postgres:5432/rockserver",
        '--env', "ROCKSERVER_API_BEARER_TOKEN=$apiToken", $RockServerImage
    )
    $created += $appContainer
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        & docker exec $appContainer curl --fail --silent http://127.0.0.1:3000/health/ready *> $null
        if ($LASTEXITCODE -eq 0) {
            Write-Host "Non-production pg_dump/pg_restore rehearsal passed (backup SHA-256 $hash)."
            return
        }
        Start-Sleep -Seconds 2
    }
    Fail 'Restored RockServer did not reach readiness.'
} finally {
    if (-not $KeepArtifacts) {
        foreach ($container in @($appContainer, $dbContainer)) {
            & docker rm --force $container *> $null
        }
        & docker network rm $network *> $null
    } else {
        Write-Host "Kept disposable restore network/container artifacts for inspection: $network."
    }
}
