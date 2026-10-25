param(
    [switch]$Release,
    [string]$BuildRoot = 'D:/BuildCache/office-glory',
    [string]$Bindgen = ''
)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskBuild = [IO.Path]::GetFullPath($BuildRoot)
if ($IsWindows -and [IO.Path]::GetPathRoot($taskBuild) -ne 'D:\') {
    throw 'BuildRoot must be on D: (all compiler output and caches stay there).'
}
$saved = @{}
foreach ($name in @('CARGO_HOME', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'TEMP', 'TMP', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_DEV_DEBUG')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
Push-Location $taskRoot
try {
    $env:CARGO_HOME = if ($IsWindows) { 'D:/BuildCache/office-glory/cargo' } else { "$taskBuild/cargo" }
    $env:CARGO_TARGET_DIR = "$taskBuild/target"
    $env:TEMP = "$taskBuild/tmp"
    $env:TMP = $env:TEMP
    $env:CARGO_INCREMENTAL = '0'
    $env:CARGO_BUILD_JOBS = '2'
    $env:CARGO_PROFILE_DEV_DEBUG = '0'
    New-Item -ItemType Directory -Force -Path $env:TEMP, "$taskBuild/web/assets" | Out-Null
    $metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    if ([IO.Path]::GetFullPath($metadata.target_directory) -ne [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)) {
        throw "Unexpected Cargo target directory: $($metadata.target_directory)"
    }
    Write-Host "Verified Cargo target: $($metadata.target_directory)"
    if ([string]::IsNullOrEmpty($Bindgen)) {
        $Bindgen = "$taskBuild/tools/bin/wasm-bindgen" + $(if ($IsWindows) { '.exe' } else { '' })
    }
    if (-not (Test-Path -LiteralPath $Bindgen)) { $Bindgen = 'wasm-bindgen' }
    $lock = Get-Content -Raw Cargo.lock
    $version = [regex]::Match($lock, 'name = "wasm-bindgen"\r?\nversion = "([^"]+)"').Groups[1].Value
    if ((& $Bindgen --version).Trim() -ne "wasm-bindgen $version") {
        throw "Requires wasm-bindgen-cli $version. Install it under D:/BuildCache/office-glory/tools and pass -Bindgen."
    }
    $arguments = @('build', '--locked', '-p', 'contrix-frontend', '--lib', '--target', 'wasm32-unknown-unknown')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw 'frontend compilation failed' }
    $profile = if ($Release) { 'release' } else { 'debug' }
    & $Bindgen "$taskBuild/target/wasm32-unknown-unknown/$profile/contrix_frontend.wasm" --target web --out-dir "$taskBuild/web/assets" --out-name contrix_frontend
    if ($LASTEXITCODE -ne 0) { throw 'wasm-bindgen failed' }
    Copy-Item -Path static/* -Destination "$taskBuild/web" -Recurse -Force
    Copy-Item -LiteralPath crates/frontend/assets/main.css -Destination "$taskBuild/web/assets/main.css"
    $sapid = $metadata.packages | Where-Object name -eq 'contrix-frontend' | ForEach-Object { $_.dependencies | Where-Object name -eq 'sapid' }
    Copy-Item -LiteralPath (Join-Path $sapid.path 'styles/sapid.css') -Destination "$taskBuild/web/assets/sapid.css"
    Write-Host "Browser assets: $taskBuild/web"
    Write-Host "Serve with CONTRIX_STATIC_DIR=$taskBuild/web and cargo run -p contrix-server"
}
finally {
    Pop-Location
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
    }
}
