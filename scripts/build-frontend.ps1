param(
    [switch]$Release,
    [string]$OutputDirectory = '',
    [string]$Bindgen = 'wasm-bindgen'
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = Get-OfficeWorkspaceMetadata
    if (-not $OutputDirectory) { $OutputDirectory = Join-Path $metadata.target_directory 'web' }
    $OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
    if ($OutputDirectory.Replace('\', '/') -match '(?i)^(D:/BuildCache|/mnt/d/BuildCache)(/|$)|(^|/)\.codex/build-targets(/|$)') {
        throw 'OutputDirectory must not use a prohibited legacy build/cache path.'
    }
    $lock = Get-Content -Raw Cargo.lock
    $version = [regex]::Match($lock, 'name = "wasm-bindgen"\r?\nversion = "([^"]+)"').Groups[1].Value
    $installedVersion = & $Bindgen --version
    if ($LASTEXITCODE -ne 0 -or ([string]$installedVersion).Trim() -ne "wasm-bindgen $version") {
        throw "Requires wasm-bindgen-cli $version on PATH or supplied with -Bindgen."
    }
    $arguments = @('build', '--locked', '-p', 'contrix-frontend', '--lib', '--target', 'wasm32-unknown-unknown')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw 'frontend compilation failed' }
    $profile = if ($Release) { 'release' } else { 'debug' }
    $wasm = Join-Path $metadata.target_directory "wasm32-unknown-unknown/$profile/contrix_frontend.wasm"
    $assets = Join-Path $OutputDirectory 'assets'
    New-Item -ItemType Directory -Force -Path $assets | Out-Null
    & $Bindgen $wasm --target web --out-dir $assets --out-name contrix_frontend
    if ($LASTEXITCODE -ne 0) { throw 'wasm-bindgen failed' }
    Copy-Item -Path static/* -Destination $OutputDirectory -Recurse -Force
    Copy-Item -Path crates/frontend/assets/* -Destination $assets -Recurse -Force
    $sapid = $metadata.packages | Where-Object name -eq 'contrix-frontend' | ForEach-Object { $_.dependencies | Where-Object name -eq 'sapid' }
    Copy-Item -LiteralPath (Join-Path $sapid.path 'styles/sapid.css') -Destination (Join-Path $assets 'sapid.css')
    Write-Host "Browser assets: $OutputDirectory"
}
finally { Pop-Location }
