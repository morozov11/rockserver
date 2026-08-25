Set-StrictMode -Version Latest

function Stop-Ops001D([string]$Message) { throw $Message }

function Test-Ops001DPrivateInventory {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$RepositoryRoot)
    $fullPath = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $root = (Resolve-Path -LiteralPath $RepositoryRoot -ErrorAction Stop).Path
    $rootPrefix = $root.TrimEnd('\') + '\'
    if (-not $fullPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) { Stop-Ops001D 'Private inventory must be inside this repository.' }
    $relative = $fullPath.Substring($rootPrefix.Length).Replace('\', '/')
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & git -C $root check-ignore --quiet -- $relative
        $ignoredExit = $LASTEXITCODE
        & git -C $root ls-files --error-unmatch -- $relative 2>$null
        $trackedExit = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference }
    if ($ignoredExit -ne 0) { Stop-Ops001D 'Private inventory is not ignored by Git; refusing to continue.' }
    if ($trackedExit -eq 0) { Stop-Ops001D 'Private inventory is tracked by Git; refusing to continue.' }
    return $fullPath
}

function Test-Ops001DInventoryValues {
    [CmdletBinding()]
    param([Parameter(Mandatory)][System.Collections.IDictionary]$Inventory)
    $allowed = @('SshUser', 'SshHost', 'Domain')
    foreach ($key in $Inventory.Keys) {
        if ($allowed -notcontains [string]$key) { Stop-Ops001D "Private inventory contains unsupported field $key." }
    }
    foreach ($field in $allowed) {
        if ([string]::IsNullOrWhiteSpace([string]$Inventory[$field])) { Stop-Ops001D "Private inventory is missing $field." }
    }
    if ([string]$Inventory.SshUser -notmatch '^[a-z_][a-z0-9_-]{0,31}$') { Stop-Ops001D 'SshUser contains unsafe characters.' }
    if ([string]$Inventory.SshHost -notmatch '^[A-Za-z0-9.-]+$') { Stop-Ops001D 'SshHost contains unsafe characters.' }
    if ([string]$Inventory.Domain -notmatch '^[A-Za-z0-9.-]+$') { Stop-Ops001D 'Domain contains unsafe characters.' }
    return $true
}

function Get-Ops001DAllowedYandexEnvironment {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path)
    $allowed = @('YANDEX_AI_API_KEY', 'YANDEX_FOLDER_ID', 'YANDEX_SPEECHKIT_API_KEY', 'YANDEX_SPEECHKIT_FOLDER_ID')
    $result = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Path)) { return $result }
    foreach ($line in Get-Content -LiteralPath $Path -ErrorAction Stop) {
        if ($line -match '^\s*(?<key>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?<value>.*)\s*$' -and $allowed -contains $Matches.key) {
            if ($Matches.value -match "[`r`n`0]") { Stop-Ops001D "Yandex value for $($Matches.key) contains a forbidden control character." }
            $result[$Matches.key] = $Matches.value
        }
    }
    return $result
}

function Write-Ops001DOwnerEnvironmentFile {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Domain,
        [Parameter(Mandatory)]$Catalog,
        [Parameter(Mandatory)][System.Collections.IDictionary]$Yandex,
        [Parameter(Mandatory)][bool]$OnnxEnabled
    )
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("ROCKSERVER_DOMAIN=$Domain")
    $lines.Add("OPS001D_CATALOG_VERSION=$($Catalog.Version)")
    $lines.Add("OPS001D_CATALOG_COUNT=$($Catalog.Count)")
    if ($OnnxEnabled) {
        $lines.Add('ROCKSERVER_SEMANTIC_PROVIDER=onnx-e5-local')
        $lines.Add('ROCKSERVER_ONNX_ASSET_DIR=/opt/rockserver/assets/onnx')
        $lines.Add('ROCKSERVER_ONNX_MODEL_PATH=/opt/rockserver/assets/onnx/model.onnx')
        $lines.Add('ROCKSERVER_ONNX_TOKENIZER_PATH=/opt/rockserver/assets/onnx/tokenizer.json')
        $lines.Add('ORT_DYLIB_PATH=/opt/rockserver/assets/onnx/libonnxruntime.so')
    }
    foreach ($key in $Yandex.Keys) { $lines.Add("$key=$($Yandex[$key])") }
    # This file is consumed by a Linux shell over SCP.  Write LF explicitly:
    # WriteAllLines uses the Windows CRLF convention on the deployment host,
    # which must not become part of a container environment value.
    [IO.File]::WriteAllText($Path, (($lines -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
}

function Get-Ops001DCurrentCommit {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$RepositoryRoot, [switch]$AllowDirty)
    $commit = (& git -C $RepositoryRoot rev-parse --verify HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $commit -notmatch '^[0-9a-f]{40}$') { Stop-Ops001D 'Could not resolve the current full Git commit.' }
    if (-not $AllowDirty) {
        $status = @(& git -C $RepositoryRoot status --porcelain --untracked-files=all)
        if ($LASTEXITCODE -ne 0) { Stop-Ops001D 'Could not verify Git worktree state.' }
        if ($status.Count -ne 0) { Stop-Ops001D 'Deploy requires a clean worktree so the image exactly represents the current Git commit.' }
    }
    return $commit
}

function Test-Ops001DArtifactIdentity {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Image, [Parameter(Mandatory)][string]$Commit, [Parameter(Mandatory)][string]$ImageId, [Parameter(Mandatory)][string]$LabelCommit)
    if ($Commit -notmatch '^[0-9a-f]{40}$') { Stop-Ops001D 'Commit must be a full lowercase 40-character SHA.' }
    if ($Image -ne "rockserver:sha-$Commit") { Stop-Ops001D 'Image must be the exact local rockserver:sha-<current-commit> reference.' }
    if ($ImageId -notmatch '^sha256:[0-9a-f]{64}$') { Stop-Ops001D 'Image ID must be an immutable sha256 identifier.' }
    if ($LabelCommit -cne $Commit) { Stop-Ops001D 'Image revision label does not match the current Git commit.' }
    return $true
}

function Get-Ops001DSshCommand {
    [CmdletBinding()]
    param([Parameter(Mandatory)][ValidateSet('bootstrap', 'deploy')][string]$Action, [Parameter(Mandatory)][string]$KeyPath, [Parameter(Mandatory)][string]$Target, [Parameter(Mandatory)][string]$RemoteCommand)
    $args = [System.Collections.Generic.List[string]]::new()
    if ($Action -eq 'bootstrap') { $args.Add('-tt') }
    $args.Add('-i'); $args.Add($KeyPath)
    $args.Add('-o'); $args.Add('ConnectTimeout=15')
    $args.Add('-o'); $args.Add('ServerAliveInterval=10')
    $args.Add('-o'); $args.Add('ServerAliveCountMax=3')
    if ($Action -eq 'deploy') {
        $args.Add('-n')
        $args.Add('-o'); $args.Add('BatchMode=yes')
    }
    $args.Add($Target); $args.Add($RemoteCommand)
    return ,$args.ToArray()
}

function Test-Ops001DOnnxManifest {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path)
    $manifest = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if (-not $manifest.enabled) { Stop-Ops001D 'The committed ONNX lock must enable the required production assets.' }
    if ($manifest.assetDirectory -ne '/opt/rockserver/assets/onnx') { Stop-Ops001D 'Enabled ONNX manifest requires the protected /opt/rockserver/assets/onnx directory.' }
    $assets = @($manifest.assets)
    if ($assets.Count -ne 3) { Stop-Ops001D 'The committed ONNX lock requires exactly model, tokenizer, and runtime assets.' }
    $expectedNames = @('model.onnx', 'tokenizer.json', 'libonnxruntime.so')
    $seen = @{}
    foreach ($asset in $assets) {
        $archiveMember = if ($null -ne $asset.PSObject.Properties['archiveMember']) { [string]$asset.archiveMember } else { '' }
        if ([string]::IsNullOrWhiteSpace($asset.name) -or $asset.name -match '[\\/]') { Stop-Ops001D 'ONNX asset name is unsafe.' }
        if ($asset.url -notmatch '^https://') { Stop-Ops001D "ONNX asset $($asset.name) requires an exact HTTPS URL." }
        if ($asset.sha256 -notmatch '^[0-9a-fA-F]{64}$') { Stop-Ops001D "ONNX asset $($asset.name) requires a verified SHA-256." }
        if ($expectedNames -notcontains [string]$asset.name -or $seen.ContainsKey([string]$asset.name)) { Stop-Ops001D 'ONNX lock contains an unexpected or duplicate asset name.' }
        $seen[[string]$asset.name] = $true
        if ($asset.name -eq 'libonnxruntime.so') {
            if ($archiveMember -ne 'onnxruntime-linux-x64-1.23.2/lib/libonnxruntime.so') { Stop-Ops001D 'ONNX runtime archive member is not the pinned Linux x64 library.' }
        } elseif (-not [string]::IsNullOrWhiteSpace($archiveMember)) { Stop-Ops001D "ONNX asset $($asset.name) must not use archive extraction." }
    }
    return $manifest
}

function Get-Ops001DCatalogMetadata {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$ManifestPath, [Parameter(Mandatory)][string]$CatalogPath)
    $manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
    $hash = (Get-FileHash -LiteralPath $CatalogPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifest.algorithm -ne 'sha256' -or $hash -ne $manifest.sha256.ToLowerInvariant()) { Stop-Ops001D 'Pinned catalog checksum verification failed.' }
    $catalog = Get-Content -LiteralPath $CatalogPath -Raw | ConvertFrom-Json
    if ($catalog.catalogVersion -ne $manifest.catalogVersion -or @($catalog.stations).Count -lt 1) { Stop-Ops001D 'Pinned catalog version or station count is invalid.' }
    return [pscustomobject]@{ Version = $manifest.catalogVersion; Count = @($catalog.stations).Count; Sha256 = $hash }
}

function Get-Ops001DFullCatalogMetadata {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$ManifestPath, [Parameter(Mandatory)][string]$CatalogPath)
    $manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
    $hash = (Get-FileHash -LiteralPath $CatalogPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifest.manifest_schema_version -ne 1 -or $manifest.database_schema_version -ne 1 -or $manifest.file -ne (Split-Path -Leaf $CatalogPath) -or $hash -ne $manifest.sha256.ToLowerInvariant()) { Stop-Ops001D 'Full catalog checksum or manifest verification failed.' }
    if ([int64]$manifest.station_count -lt 16000 -or [string]::IsNullOrWhiteSpace([string]$manifest.catalog_version)) { Stop-Ops001D 'Full catalog does not meet the complete-catalog station gate.' }
    return [pscustomobject]@{ Version = $manifest.catalog_version; Count = [int64]$manifest.station_count; Sha256 = $hash }
}

function Format-Ops001DSafeSummary {
    [CmdletBinding()]
    param([System.Collections.IDictionary]$Environment = @{}, [string]$Commit = '', [string]$ImageId = '', [string]$Readiness = '')
    return "commit=$Commit image_id=$ImageId yandex_keys=$($Environment.Keys.Count) readiness=$Readiness"
}

Export-ModuleMember -Function Test-Ops001DPrivateInventory, Test-Ops001DInventoryValues, Get-Ops001DAllowedYandexEnvironment, Write-Ops001DOwnerEnvironmentFile, Get-Ops001DCurrentCommit, Test-Ops001DArtifactIdentity, Get-Ops001DSshCommand, Test-Ops001DOnnxManifest, Get-Ops001DCatalogMetadata, Get-Ops001DFullCatalogMetadata, Format-Ops001DSafeSummary
