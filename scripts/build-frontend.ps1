param(
    [switch]$Release,
    [string]$BuildRoot = $(if ($IsWindows) { 'D:/BuildCache/office-glory' } elseif ($env:GITHUB_ACTIONS -eq 'true') { "$env:RUNNER_TEMP/office-glory" } else { '/mnt/d/BuildCache/office-glory' }),
    [string]$Bindgen = ''
)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskBuild = [IO.Path]::GetFullPath($BuildRoot)
if ($IsWindows -and [IO.Path]::GetPathRoot($taskBuild) -ne 'D:\') {
    throw 'BuildRoot must be on D: (all compiler output and caches stay there).'
}
if ($env:WSL_DISTRO_NAME) {
    if (-not $taskBuild.StartsWith('/mnt/d/BuildCache/')) { throw 'WSL BuildRoot must be under /mnt/d/BuildCache on the Windows D: drive.' }
    $mountSource = ([string](& findmnt --noheadings --output SOURCE --target /mnt/d)).Trim()
    if ($LASTEXITCODE -ne 0 -or $mountSource -notmatch '(?i)^d:([\\/]|$)') { throw '/mnt/d must be verified as the Windows D: drive before building.' }
}
$saved = @{}
foreach ($name in @('CARGO_HOME', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'TEMP', 'TMP', 'TMPDIR', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_DEV_DEBUG')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
Push-Location $taskRoot
try {
    $env:CARGO_HOME = "$taskBuild/cargo"
    $env:CARGO_TARGET_DIR = "$taskBuild/target"
    $env:TEMP = "$taskBuild/tmp"
    $env:TMP = $env:TEMP
    $env:TMPDIR = $env:TEMP
    $env:CARGO_INCREMENTAL = '0'
    $env:CARGO_BUILD_JOBS = '2'
    $env:CARGO_PROFILE_DEV_DEBUG = '0'
    New-Item -ItemType Directory -Force -Path $env:TEMP, $env:CARGO_HOME, "$taskBuild/web/assets" | Out-Null
    $dependencyRoots = @('.dependencies/glory', '.dependencies/sapid')
    if ($dependencyRoots | Where-Object { -not (Test-Path -LiteralPath "$_/Cargo.toml") }) {
        Write-Host 'Initializing the pinned Glory and Sapid submodules...'
        & git submodule update --init --recursive -- @dependencyRoots
        if ($LASTEXITCODE -ne 0) { throw 'Dependency initialization failed. Run git submodule update --init --recursive from the repository root.' }
    }
    foreach ($dependencyRoot in $dependencyRoots) {
        if (-not (Test-Path -LiteralPath "$dependencyRoot/Cargo.toml")) { throw "Missing $dependencyRoot/Cargo.toml. Clone with --recurse-submodules or run git submodule update --init --recursive." }
    }
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
        throw "Requires wasm-bindgen-cli $version. Install it under $taskBuild/tools and pass -Bindgen."
    }
    $arguments = @('build', '--locked', '-p', 'contrix-frontend', '--lib', '--target', 'wasm32-unknown-unknown')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw 'frontend compilation failed' }
    $profile = if ($Release) { 'release' } else { 'debug' }
    & $Bindgen "$taskBuild/target/wasm32-unknown-unknown/$profile/contrix_frontend.wasm" --target web --out-dir "$taskBuild/web/assets" --out-name contrix_frontend
    if ($LASTEXITCODE -ne 0) { throw 'wasm-bindgen failed' }
    Copy-Item -Path static/* -Destination "$taskBuild/web" -Recurse -Force
    Copy-Item -Path crates/frontend/assets/* -Destination "$taskBuild/web/assets" -Recurse -Force
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
