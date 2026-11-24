param([int]$Workers = 2, [int]$RequestsPerWorker = 100, [int]$HotExports = 30,
      [int]$HotTitleBytes = 32768, [string]$Output = '')
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = Get-SyntraWorkspaceMetadata
    if (-not $Output) { $Output = Join-Path $metadata.target_directory ('performance/http-' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '.json') }
    $extension = if ($IsWindows) { '.exe' } else { '' }
    $binary = Join-Path $metadata.target_directory "release/syntra-server$extension"
    & python "$PSScriptRoot/benchmark-http.py" --binary $binary --output $Output --workers $Workers --requests-per-worker $RequestsPerWorker --hot-exports $HotExports --hot-title-bytes $HotTitleBytes
    if ($LASTEXITCODE -ne 0) { throw "Capacity workload reported errors; inspect $Output" }
}
finally { Pop-Location }
