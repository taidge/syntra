param([switch]$Release, [switch]$SkipFrontendBuild)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
$savedStaticDirectory = $env:SYNTRA_STATIC_DIR
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = Get-SyntraWorkspaceMetadata
    Write-Host "Cargo target: $($metadata.target_directory)"
    $web = Join-Path $metadata.target_directory 'syntra-web'
    if (-not $SkipFrontendBuild) { & "$PSScriptRoot/build-frontend.ps1" -Release:$Release -OutputDirectory $web }
    $legacyStaticDirectory = [Environment]::GetEnvironmentVariable('CON' + 'TRIX_STATIC_DIR')
    if (-not ($env:SYNTRA_STATIC_DIR -or $legacyStaticDirectory)) { $env:SYNTRA_STATIC_DIR = $web }
    $arguments = @('run', '--locked', '-p', 'syntra-server')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    $runExitCode = $LASTEXITCODE
}
finally {
    Pop-Location
    $env:SYNTRA_STATIC_DIR = $savedStaticDirectory
}
exit $runExitCode
