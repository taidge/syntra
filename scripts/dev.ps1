param([switch]$Release, [switch]$SkipFrontendBuild)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
$savedStaticDirectory = $env:CONTRIX_STATIC_DIR
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = Get-OfficeWorkspaceMetadata
    Write-Host "Cargo target: $($metadata.target_directory)"
    $web = Join-Path $metadata.target_directory 'web'
    if (-not $SkipFrontendBuild) { & "$PSScriptRoot/build-frontend.ps1" -Release:$Release -OutputDirectory $web }
    if (-not $env:CONTRIX_STATIC_DIR) { $env:CONTRIX_STATIC_DIR = $web }
    $arguments = @('run', '--locked', '-p', 'contrix-server')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    $runExitCode = $LASTEXITCODE
}
finally {
    Pop-Location
    $env:CONTRIX_STATIC_DIR = $savedStaticDirectory
}
exit $runExitCode
