param([switch]$Release, [switch]$SkipFrontendBuild)
$ErrorActionPreference = 'Stop'
$taskBuild = 'D:/BuildCache/office-glory'
if (-not $SkipFrontendBuild) { & "$PSScriptRoot/build-frontend.ps1" -Release:$Release }
$env:CARGO_HOME = 'D:/BuildCache/office-glory/cargo'
$env:CARGO_TARGET_DIR = "$taskBuild/target"
$env:TEMP = "$taskBuild/tmp"
$env:TMP = $env:TEMP
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_BUILD_JOBS = '2'
$env:CONTRIX_STATIC_DIR = "$taskBuild/web"
$metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or [IO.Path]::GetFullPath($metadata.target_directory) -ne [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)) { throw 'Cargo target verification failed' }
& cargo run --locked -p contrix-server
exit $LASTEXITCODE
