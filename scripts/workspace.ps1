# Shared read-only discovery. Never redirect Cargo or temporary directories.
function Get-SyntraWorkspaceMetadata {
    foreach ($path in @($env:CARGO_HOME, $env:CARGO_TARGET_DIR, $env:TEMP, $env:TMP, $env:TMPDIR)) {
        if ($path -and ($path.Replace('\', '/') -match '(?i)^(D:/BuildCache|/mnt/d/BuildCache)(/|$)|(^|/)\.codex/build-targets(/|$)')) {
            throw 'A prohibited legacy build/cache path is configured. Remove that setting before building.'
        }
    }
    $metadataText = & cargo metadata --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $metadata = $metadataText | ConvertFrom-Json
    if ($metadata.target_directory.Replace('\', '/') -match '(?i)^(D:/BuildCache|/mnt/d/BuildCache)(/|$)|(^|/)\.codex/build-targets(/|$)') {
        throw 'Cargo metadata reports a prohibited legacy build/cache path. Remove that configuration before building.'
    }
    return $metadata
}
