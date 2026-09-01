# Starts RockServer on the host and serves the administrator SPA through a disposable Caddy container.
# Prerequisites: host PostgreSQL (DATABASE_URL in .env), admin bootstrap, ONNX assets for run-rockserver-local.ps1,
# Docker Desktop for Caddy, and pnpm for the web bundle when web/dist is missing.
param(
    [int]$AdminUiPort = 3080
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$assetsRoot = 'C:\Users\alex\Documents\Codex\2026-08-17\referenced-chatgpt-conversation-this-is-an\work\e5-assets'
$envFile = Join-Path $projectRoot '.env'
$caddyImage = 'caddy:2.10.0-alpine'
$caddyfile = Join-Path $projectRoot 'deploy\Caddyfile.dev-host'
$webDist = Join-Path $projectRoot 'web\dist'
$webIndex = Join-Path $webDist 'index.html'

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw 'Docker is required to serve the administrator SPA locally. Install Docker Desktop or use run-rockserver-local.ps1 for API-only development.'
}
if (-not (Test-Path -LiteralPath $envFile)) { throw "Missing local configuration: $envFile" }
if (-not (Test-Path -LiteralPath $caddyfile)) { throw "Missing Caddyfile: $caddyfile" }

function Initialize-LocalDockerEngine {
    # Docker Desktop on Windows listens through a named pipe. A stale WSL/Linux
    # DOCKER_HOST (or the same value in .env) makes the client try unix:///var/run/docker.sock.
    if ($env:OS -eq 'Windows_NT') {
        if ($env:DOCKER_HOST -match '^unix://') {
            Write-Host 'Ignoring incompatible Unix DOCKER_HOST for Windows Docker Desktop.' -ForegroundColor Yellow
            $env:DOCKER_HOST = 'npipe:////./pipe/docker_engine'
        }
        elseif ([string]::IsNullOrWhiteSpace($env:DOCKER_HOST)) {
            $env:DOCKER_HOST = 'npipe:////./pipe/docker_engine'
        }
    }
    $null = & docker info --format '{{.ServerVersion}}' 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw 'Docker Engine is unavailable. Start Docker Desktop, wait until the engine is running, then retry.'
    }
}

Get-Content -LiteralPath $envFile | ForEach-Object {
    if ($_ -match '^\s*([^#=]+)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2], 'Process')
    }
}

Initialize-LocalDockerEngine

if ([string]::IsNullOrWhiteSpace($env:DATABASE_URL)) { throw 'DATABASE_URL is missing from the local .env file.' }
if ([string]::IsNullOrWhiteSpace($env:ROCKSERVER_API_BEARER_TOKEN)) {
    $tokenBytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($tokenBytes)
    $env:ROCKSERVER_API_BEARER_TOKEN = [Convert]::ToBase64String($tokenBytes)
    Write-Host 'ROCKSERVER_API_BEARER_TOKEN is unset; using a process-local development credential.' -ForegroundColor Yellow
}
if ($env:ROCKSERVER_API_BEARER_TOKEN.Trim().Length -lt 32) { throw 'ROCKSERVER_API_BEARER_TOKEN must contain at least 32 characters.' }
if ([string]::IsNullOrWhiteSpace($env:ROCKSERVER_TRUSTED_PROXY_TOKEN)) {
    $proxyTokenBytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($proxyTokenBytes)
    $env:ROCKSERVER_TRUSTED_PROXY_TOKEN = [Convert]::ToBase64String($proxyTokenBytes)
    Write-Host 'ROCKSERVER_TRUSTED_PROXY_TOKEN is unset; using a process-local proxy credential.' -ForegroundColor Yellow
}
if ($env:ROCKSERVER_TRUSTED_PROXY_TOKEN.Trim().Length -lt 32) { throw 'ROCKSERVER_TRUSTED_PROXY_TOKEN must contain at least 32 characters.' }

$model = Join-Path $assetsRoot 'model.onnx'
$tokenizer = Join-Path $assetsRoot 'tokenizer.json'
$runtime = Get-ChildItem -LiteralPath (Join-Path $assetsRoot 'ort') -Recurse -Filter 'onnxruntime.dll' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
if (-not (Test-Path -LiteralPath $model) -or -not (Test-Path -LiteralPath $tokenizer) -or -not $runtime) {
    throw 'Local E5/ONNX assets are missing. Run the model setup once before starting the server.'
}

$env:ROCKSERVER_SEMANTIC_PROVIDER = 'onnx-e5-local'
$env:ROCKSERVER_ONNX_MODEL_PATH = $model
$env:ROCKSERVER_ONNX_TOKENIZER_PATH = $tokenizer
$env:ORT_DYLIB_PATH = $runtime

$bindAddress = $env:ROCKSERVER_BIND_ADDR
if ([string]::IsNullOrWhiteSpace($bindAddress)) {
    $bindAddress = '0.0.0.0:3000'
}
if ($bindAddress -notmatch ':(?<port>\d+)$') {
    throw "ROCKSERVER_BIND_ADDR has no valid port: $bindAddress"
}
$backendPort = [int]$Matches.port
$adminOrigin = "http://127.0.0.1:$AdminUiPort"
$env:ROCKSERVER_LOCAL_ADMIN_ORIGIN = $adminOrigin
$env:ROCKSERVER_ADMIN_UI_PORT = "$AdminUiPort"
$env:ROCKSERVER_BACKEND_UPSTREAM = "host.docker.internal:$backendPort"

if (-not (Test-Path -LiteralPath $webIndex)) {
    Write-Host 'web/dist is missing; building the administrator SPA bundle...' -ForegroundColor Yellow
    if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
        throw 'pnpm is required to build web/dist. Run "cd web && pnpm install && pnpm build" once, then retry.'
    }
    Push-Location (Join-Path $projectRoot 'web')
    try {
        pnpm install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw 'pnpm install failed.' }
        pnpm build
        if ($LASTEXITCODE -ne 0) { throw 'pnpm build failed.' }
    }
    finally {
        Pop-Location
    }
}

Set-Location -LiteralPath $projectRoot

Write-Host "Starting RockServer at $bindAddress (API backend for Caddy) ..."
$rockserver = Start-Process -FilePath 'cargo' `
    -ArgumentList @('run', '--features', 'onnx-local', '--bin', 'rockserver') `
    -WorkingDirectory $projectRoot `
    -PassThru `
    -NoNewWindow

function Stop-LocalStack {
    if ($rockserver -and -not $rockserver.HasExited) {
        Write-Host 'Stopping RockServer...'
        Stop-Process -Id $rockserver.Id -Force -ErrorAction SilentlyContinue
    }
}

try {
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    while (-not $rockserver.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$backendPort/health/live" -TimeoutSec 2
            if ($response.StatusCode -eq 200) { break }
        }
        catch {
            Start-Sleep -Seconds 1
        }
    }
    if ($rockserver.HasExited) {
        throw 'RockServer exited before becoming ready. Check cargo output above.'
    }

    Write-Host ("Administrator console: http://127.0.0.1:{0}/admin" -f $AdminUiPort)
    Write-Host ("Local admin login is enabled only for {0}." -f $adminOrigin) -ForegroundColor Yellow
    Write-Host 'Press Ctrl+C to stop RockServer and Caddy.'

    docker run --rm `
        --name rockserver-admin-caddy `
        -p "127.0.0.1:${AdminUiPort}:${AdminUiPort}" `
        -v "${webDist}:/srv:ro" `
        -v "${caddyfile}:/etc/caddy/Caddyfile:ro" `
        -e ROCKSERVER_ADMIN_UI_PORT="$AdminUiPort" `
        -e ROCKSERVER_BACKEND_UPSTREAM="$env:ROCKSERVER_BACKEND_UPSTREAM" `
        -e ROCKSERVER_TRUSTED_PROXY_TOKEN="$env:ROCKSERVER_TRUSTED_PROXY_TOKEN" `
        $caddyImage
    if ($LASTEXITCODE -ne 0) {
        throw 'Caddy container exited with an error.'
    }
}
finally {
    Stop-LocalStack
}
