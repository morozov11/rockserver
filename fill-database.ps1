# Fill rockserver DB: multi-tag import + stream probe.
# Credentials are loaded from .env.

$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $scriptDir

# Load .env
Get-Content .env | ForEach-Object {
    if ($_ -match '^\s*([^#][^=]+?)\s*=\s*(.+)$') {
        [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
    }
}

if (-not $env:DATABASE_URL) {
    Write-Error "DATABASE_URL not found in .env"
    exit 1
}

$tags = @(
    "rock", "metal", "jazz", "pop", "electronic", "classical",
    "hip hop", "country", "folk", "blues", "punk", "reggae",
    "latin", "ambient", "soul", "funk", "dance", "world music",
    "ska", "gospel", "r&b", "new age", "house", "techno",
    "trance", "drum and bass", "dubstep", "indie", "alternative",
    "hard rock", "classic rock", "smooth jazz", "heavy metal",
    "black metal", "death metal", "thrash metal", "power metal",
    "progressive rock", "grunge", "synthwave", "chillout",
    "downtempo", "trip hop", "disco", "salsa", "bossa nova",
    "bluegrass", "americana"
)

$env:RADIO_BROWSER_TAGS = $tags -join ","
$env:RADIO_BROWSER_MAX_PAGES = "100"
$env:RADIO_BROWSER_PAGE_SIZE = "500"

Write-Host "=== Importing stations for $($tags.Count) tags ===" -ForegroundColor Cyan
cargo run --bin import_radio_browser
if ($LASTEXITCODE -ne 0) {
    Write-Error "Import failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host ""
Write-Host "=== Probing stream availability ===" -ForegroundColor Cyan
cargo run --bin probe_streams
if ($LASTEXITCODE -ne 0) {
    Write-Error "Probe failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host ""
Write-Host "=== Backfilling station embeddings ===" -ForegroundColor Cyan
cargo run --features onnx-local --bin backfill_embeddings
if ($LASTEXITCODE -ne 0) {
    Write-Error "Backfill failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host ""
Write-Host "=== Done ===" -ForegroundColor Green
