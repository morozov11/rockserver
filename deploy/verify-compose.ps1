[CmdletBinding()]
param(
    [ValidateSet('local', 'production')]
    [string]$Mode = 'local',
    [switch]$Start,
    [switch]$Keep
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$composeFile = Join-Path $PSScriptRoot 'compose.yaml'
$localOverride = Join-Path $PSScriptRoot 'compose.local.yaml'
$productionOverride = Join-Path $PSScriptRoot 'compose.production.yaml'
$environmentFile = Join-Path $repoRoot '.env.example'
$projectName = 'rockserver-ops001b-preflight'

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw 'Docker CLI is required for Compose verification.'
}
if (-not (Test-Path -LiteralPath $environmentFile)) {
    throw "Missing safe environment example: $environmentFile"
}

$composeArguments = @(
    'compose', '--project-name', $projectName, '--env-file', $environmentFile,
    '--file', $composeFile
)
if ($Mode -eq 'local') {
    $composeArguments += @('--file', $localOverride)
} else {
    $composeArguments += @('--file', $productionOverride)
}

Push-Location $repoRoot
try {
    & docker @composeArguments config --quiet
    if ($LASTEXITCODE -ne 0) {
        throw "Compose $Mode configuration validation failed."
    }
    Write-Host "Compose $Mode configuration passed without printing environment values."

    if (-not $Start) {
        return
    }

    if ($Mode -ne 'local') {
        throw 'The local startup check supports only -Mode local; production launch is a manual VPS step.'
    }

    & docker build --file Dockerfile --tag rockserver:local .
    if ($LASTEXITCODE -ne 0) {
        throw 'Local RockServer image build failed.'
    }

    try {
        & docker @composeArguments up --detach --wait
        if ($LASTEXITCODE -ne 0) {
            throw 'Local Compose startup failed.'
        }

        $port = 80
        $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$port/health/ready" -TimeoutSec 15
        if ($response.StatusCode -ne 200) {
            throw "Local readiness returned HTTP $($response.StatusCode)."
        }
        Write-Host 'Local Compose startup and readiness check passed (HTTP 200).'
    } finally {
        if (-not $Keep) {
            & docker @composeArguments down --volumes --remove-orphans | Out-Null
        }
    }
} finally {
    Pop-Location
}
