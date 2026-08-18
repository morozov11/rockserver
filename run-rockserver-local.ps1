param(
    [string]$ApiBearerToken
)

$ErrorActionPreference = 'Stop'
$projectRoot = 'C:\repos\rockserver'
$assetsRoot = 'C:\Users\alex\Documents\Codex\2026-08-17\referenced-chatgpt-conversation-this-is-an\work\e5-assets'
$envFile = Join-Path $projectRoot '.env'

if (-not (Test-Path -LiteralPath $envFile)) { throw "Missing local configuration: $envFile" }
Get-Content -LiteralPath $envFile | ForEach-Object {
    if ($_ -match '^\s*([^#=]+)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2], 'Process')
    }
}

if ([string]::IsNullOrWhiteSpace($ApiBearerToken)) {
    $ApiBearerToken = $env:ROCKSERVER_API_BEARER_TOKEN
}
if ([string]::IsNullOrWhiteSpace($ApiBearerToken)) {
    $randomBytes = New-Object byte[] 32
    $randomGenerator = [Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $randomGenerator.GetBytes($randomBytes)
        $ApiBearerToken = -join ($randomBytes | ForEach-Object { $_.ToString('x2') })
    }
    finally {
        $randomGenerator.Dispose()
    }
    Write-Host 'ROCKSERVER_API_BEARER_TOKEN was not set; generated a temporary token for this local run.' -ForegroundColor Yellow
}
$env:ROCKSERVER_API_BEARER_TOKEN = $ApiBearerToken

if ([string]::IsNullOrWhiteSpace($env:DATABASE_URL)) { throw 'DATABASE_URL is missing from the local .env file.' }
$model = Join-Path $assetsRoot 'model.onnx'
$tokenizer = Join-Path $assetsRoot 'tokenizer.json'
$runtime = Get-ChildItem -LiteralPath (Join-Path $assetsRoot 'ort') -Recurse -Filter 'onnxruntime.dll' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
if (-not (Test-Path -LiteralPath $model) -or -not (Test-Path -LiteralPath $tokenizer) -or -not $runtime) { throw 'Local E5/ONNX assets are missing. Run the model setup once before starting the server.' }

$env:ROCKSERVER_SEMANTIC_PROVIDER = 'onnx-e5-local'
$env:ROCKSERVER_ONNX_MODEL_PATH = $model
$env:ROCKSERVER_ONNX_TOKENIZER_PATH = $tokenizer
$env:ORT_DYLIB_PATH = $runtime

Set-Location -LiteralPath $projectRoot
Write-Host 'Starting RockServer at http://127.0.0.1:3000 ...'
Write-Host "Admin preview: http://127.0.0.1:3000/admin (token: $ApiBearerToken)"
cargo run --features onnx-local --bin rockserver
