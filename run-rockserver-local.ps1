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

if ([string]::IsNullOrWhiteSpace($env:DATABASE_URL)) { throw 'DATABASE_URL is missing from the local .env file.' }
if ([string]::IsNullOrWhiteSpace($env:ROCKSERVER_API_BEARER_TOKEN)) {
    # Public /v1 does not use this value. It only keeps legacy /api/v1 and
    # local admin routes protected while developing without a secret in .env.
    $tokenBytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($tokenBytes)
    $env:ROCKSERVER_API_BEARER_TOKEN = [Convert]::ToBase64String($tokenBytes)
    Write-Host 'ROCKSERVER_API_BEARER_TOKEN is unset; using a process-local development credential for protected local routes.' -ForegroundColor Yellow
}
if ($env:ROCKSERVER_API_BEARER_TOKEN.Trim().Length -lt 32) { throw 'ROCKSERVER_API_BEARER_TOKEN must contain at least 32 characters.' }
$model = Join-Path $assetsRoot 'model.onnx'
$tokenizer = Join-Path $assetsRoot 'tokenizer.json'
$runtime = Get-ChildItem -LiteralPath (Join-Path $assetsRoot 'ort') -Recurse -Filter 'onnxruntime.dll' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
if (-not (Test-Path -LiteralPath $model) -or -not (Test-Path -LiteralPath $tokenizer) -or -not $runtime) { throw 'Local E5/ONNX assets are missing. Run the model setup once before starting the server.' }

$env:ROCKSERVER_SEMANTIC_PROVIDER = 'onnx-e5-local'
$env:ROCKSERVER_ONNX_MODEL_PATH = $model
$env:ROCKSERVER_ONNX_TOKENIZER_PATH = $tokenizer
$env:ORT_DYLIB_PATH = $runtime

function Get-LanIPv4Addresses {
    $addresses = @()
    try {
        $addresses += Get-NetIPConfiguration -ErrorAction Stop |
            Where-Object { $_.IPv4DefaultGateway -and $_.IPv4Address } |
            ForEach-Object { $_.IPv4Address.IPAddress }
    }
    catch {
        # Fall back to .NET network interfaces when the network cmdlets are unavailable.
        $addresses += [System.Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces() |
            ForEach-Object {
                if ($_.OperationalStatus -ne [System.Net.NetworkInformation.OperationalStatus]::Up) {
                    return
                }
                if ($_.NetworkInterfaceType -in @(
                        [System.Net.NetworkInformation.NetworkInterfaceType]::Loopback,
                        [System.Net.NetworkInformation.NetworkInterfaceType]::Tunnel,
                        [System.Net.NetworkInformation.NetworkInterfaceType]::Unknown
                    )) {
                    return
                }
                if ($_.Description -match '(?i)vpn|virtual|wireguard|tunnel') {
                    return
                }
                $properties = $_.GetIPProperties()
                $hasIpv4Gateway = $properties.GatewayAddresses |
                    Where-Object { $_.Address.AddressFamily -eq [System.Net.Sockets.AddressFamily]::InterNetwork }
                if (-not $hasIpv4Gateway) {
                    return
                }
                $properties.UnicastAddresses |
                    Where-Object { $_.Address.AddressFamily -eq [System.Net.Sockets.AddressFamily]::InterNetwork } |
                    ForEach-Object { $_.Address.ToString() }
            }
    }

    $addresses |
        Where-Object { $_ -and $_ -notlike '127.*' -and $_ -notlike '169.254.*' } |
        Sort-Object -Unique
}

$bindAddress = $env:ROCKSERVER_BIND_ADDR
if ([string]::IsNullOrWhiteSpace($bindAddress)) {
    $bindAddress = '0.0.0.0:3000'
}
if ($bindAddress -notmatch ':(?<port>\d+)$') {
    throw "ROCKSERVER_BIND_ADDR has no valid port: $bindAddress"
}
$port = [int]$Matches.port

Set-Location -LiteralPath $projectRoot
Write-Host "Starting RockServer listener at $bindAddress ..."
if ($bindAddress -match '^0\.0\.0\.0:') {
    Write-Host ("Admin preview (localhost): http://127.0.0.1:{0}/admin (token loaded from .env)" -f $port)
    $lanAddresses = @(Get-LanIPv4Addresses)
    if ($lanAddresses.Count -eq 0) {
        Write-Host 'Admin preview (LAN): no active LAN IPv4 address detected.' -ForegroundColor Yellow
    }
    else {
        foreach ($address in $lanAddresses) {
            Write-Host ("Admin preview (LAN): http://{0}:{1}/admin (token loaded from .env)" -f $address, $port)
        }
    }
}
else {
    $displayAddress = $bindAddress -replace ':\d+$', ''
    Write-Host ("Admin preview: http://{0}:{1}/admin (token loaded from .env)" -f $displayAddress, $port)
}
cargo run --features onnx-local --bin rockserver
