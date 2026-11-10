param([switch]$BuildWeb)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = Get-OfficeWorkspaceMetadata
    Write-Host "Cargo target: $($metadata.target_directory)"
    & cargo fmt -- --check
    if ($LASTEXITCODE -ne 0) { throw 'format check failed' }
    & cargo test --locked --workspace --features contrix-frontend/ssr
    if ($LASTEXITCODE -ne 0) { throw 'workspace tests failed' }
    & cargo check --locked -p contrix-frontend --target wasm32-unknown-unknown
    if ($LASTEXITCODE -ne 0) { throw 'WASM check failed' }
    & cargo clippy --locked --workspace --all-targets --no-deps --features contrix-frontend/ssr -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed' }
    if ($BuildWeb) { & "$PSScriptRoot/build-frontend.ps1" }
}
finally { Pop-Location }
