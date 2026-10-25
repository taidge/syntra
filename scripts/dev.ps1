param(
    [switch]$Release,
    [switch]$SkipFrontendBuild,
    [string]$BuildRoot = $(if ($IsWindows) { 'D:/BuildCache/office-glory' } elseif ($env:GITHUB_ACTIONS -eq 'true') { "$env:RUNNER_TEMP/office-glory" } else { '/mnt/d/BuildCache/office-glory' })
)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskBuild = [IO.Path]::GetFullPath($BuildRoot)
if ($IsWindows -and [IO.Path]::GetPathRoot($taskBuild) -ne 'D:\') { throw 'BuildRoot must be on D: (all compiler output and caches stay there).' }
if ($env:WSL_DISTRO_NAME) {
    if (-not $taskBuild.StartsWith('/mnt/d/BuildCache/')) { throw 'WSL BuildRoot must be under /mnt/d/BuildCache on the Windows D: drive.' }
    $mountSource = ([string](& findmnt --noheadings --output SOURCE --target /mnt/d)).Trim()
    if ($LASTEXITCODE -ne 0 -or $mountSource -notmatch '(?i)^d:([\\/]|$)') { throw '/mnt/d must be verified as the Windows D: drive before building.' }
}
$saved = @{}
foreach ($name in @('CARGO_HOME', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'TEMP', 'TMP', 'TMPDIR', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_DEV_DEBUG', 'CONTRIX_STATIC_DIR')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
Push-Location $taskRoot
try {
    if (-not $SkipFrontendBuild) { & "$PSScriptRoot/build-frontend.ps1" -Release:$Release -BuildRoot $taskBuild }
    $env:CARGO_HOME = "$taskBuild/cargo"
    $env:CARGO_TARGET_DIR = "$taskBuild/target"
    $env:TEMP = "$taskBuild/tmp"
    $env:TMP = $env:TEMP
    $env:TMPDIR = $env:TEMP
    $env:CARGO_INCREMENTAL = '0'
    $env:CARGO_PROFILE_DEV_DEBUG = '0'
    $env:CARGO_BUILD_JOBS = '2'
    $env:CONTRIX_STATIC_DIR = "$taskBuild/web"
    New-Item -ItemType Directory -Force -Path $env:TEMP, $env:CARGO_HOME | Out-Null
    $dependencyRoots = @('.dependencies/glory', '.dependencies/sapid')
    if ($dependencyRoots | Where-Object { -not (Test-Path -LiteralPath "$_/Cargo.toml") }) {
        & git submodule update --init --recursive -- @dependencyRoots
        if ($LASTEXITCODE -ne 0) { throw 'Dependency initialization failed. Run git submodule update --init --recursive from the repository root.' }
    }
    foreach ($dependencyRoot in $dependencyRoots) {
        if (-not (Test-Path -LiteralPath "$dependencyRoot/Cargo.toml")) { throw "Missing $dependencyRoot/Cargo.toml. Clone with --recurse-submodules or run git submodule update --init --recursive." }
    }
    $metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or [IO.Path]::GetFullPath($metadata.target_directory) -ne [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)) { throw 'Cargo target verification failed' }
    Write-Host "Verified Cargo target: $($metadata.target_directory)"
    $arguments = @('run', '--locked', '-p', 'contrix-server')
    if ($Release) { $arguments += '--release' }
    & cargo @arguments
    $runExitCode = $LASTEXITCODE
}
finally {
    Pop-Location
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
exit $runExitCode
