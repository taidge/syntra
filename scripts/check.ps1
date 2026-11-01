param([switch]$BuildWeb)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskBuild = 'D:/BuildCache/office-glory'
$env:CARGO_HOME = 'D:/BuildCache/office-glory/cargo'
$env:CARGO_TARGET_DIR = "$taskBuild/target"
$env:TEMP = "$taskBuild/tmp"
$env:TMP = $env:TEMP
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
New-Item -ItemType Directory -Force -Path $env:TEMP | Out-Null
Push-Location $taskRoot
try {
    $metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or [IO.Path]::GetFullPath($metadata.target_directory) -ne [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)) { throw 'Cargo target verification failed' }
    Write-Host "Verified Cargo target: $($metadata.target_directory)"
    & cargo fmt -- --check
    if ($LASTEXITCODE -ne 0) { throw 'format check failed' }
    & cargo test --locked --workspace --features contrix-frontend/ssr
    if ($LASTEXITCODE -ne 0) { throw 'workspace tests failed' }
    & cargo check --locked -p contrix-frontend --target wasm32-unknown-unknown
    if ($LASTEXITCODE -ne 0) { throw 'WASM check failed' }
    & cargo clippy --locked --workspace --all-targets --features contrix-frontend/ssr -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    if ($BuildWeb) { & "$PSScriptRoot/build-frontend.ps1" }
}
finally { Pop-Location }
