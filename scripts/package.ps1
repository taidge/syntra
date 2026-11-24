param([string]$Target = '', [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $metadata = Get-SyntraWorkspaceMetadata
    $targetInfo = (& rustc -vV | Select-String '^host: ').ToString().Substring(6).Trim()
    if (-not $Target) { $Target = $targetInfo; $binaryDirectory = Join-Path $metadata.target_directory 'release' }
    else { $binaryDirectory = Join-Path $metadata.target_directory "$Target/release" }
    if (-not $SkipBuild) {
        $build = @('build', '--locked', '--release', '-p', 'syntra-server')
        if ($Target -ne $targetInfo -or $binaryDirectory -ne (Join-Path $metadata.target_directory 'release')) { $build += @('--target', $Target) }
        & cargo @build
        if ($LASTEXITCODE -ne 0) { throw 'server compilation failed' }
        & "$PSScriptRoot/build-frontend.ps1" -Release
    }
    $web = Join-Path $metadata.target_directory 'syntra-web'
    foreach ($asset in @('assets/syntra_frontend.js', 'assets/syntra_frontend_bg.wasm', 'assets/calc-worker.js', 'assets/main.css', 'assets/sapid.css')) {
        if (-not (Test-Path -LiteralPath (Join-Path $web $asset))) { throw "Missing frontend asset: $asset" }
    }
    $extension = if ($Target -like '*windows*') { '.exe' } else { '' }
    $binary = Join-Path $binaryDirectory "syntra-server$extension"
    if (-not (Test-Path -LiteralPath $binary)) { throw "Missing server binary: $binary" }
    $distribution = Join-Path $metadata.target_directory 'dist'
    $staging = Join-Path $distribution ("staging-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Copy-Item -LiteralPath $binary -Destination $staging
    Copy-Item -LiteralPath config,migrations -Destination $staging -Recurse
    Copy-Item -LiteralPath $web -Destination (Join-Path $staging 'static') -Recurse
    Copy-Item -LiteralPath README.md,README_ZH.md,LICENSE-APACHE,LICENSE-MIT -Destination $staging
    @'
Run from this directory so config/, migrations/ and static/ are available.
For production set SYNTRA_ENV=production and JWT_SECRET to a random secret
of at least 32 bytes; configure writable database and storage paths in config/.
Start ./syntra-server (syntra-server.exe on Windows), then open its bind address.
PDF export needs an installed Chrome, Chromium or Edge browser. If it is not
discovered automatically, set SYNTRA_PDF_BROWSER to the browser executable.
Keep browser sandboxing available. PDF reading and OCR assets are bundled in static/.
'@ | Set-Content -LiteralPath (Join-Path $staging 'RUNNING.txt') -Encoding utf8
    $archive = Join-Path $distribution "syntra-$Target.zip"
    Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $archive -Force
    if ($Target -notlike '*windows*') {
        $zip=[IO.Compression.ZipFile]::Open($archive,[IO.Compression.ZipArchiveMode]::Update)
        try {
            $entry=$zip.GetEntry('syntra-server')
            if (-not $entry) { throw 'Distribution is missing the server executable' }
            $entry.ExternalAttributes = 0x81ED -shl 16
        }
        finally { $zip.Dispose() }
    }
    Write-Host "Distribution: $archive"
}
finally { Pop-Location }
