#requires -Version 7.2
<#
.SYNOPSIS
Run an existing release ZIP or container image without using application sources.
.DESCRIPTION
All server processes start in a fresh extracted package. The test's config,
SQLite database, storage, logs and optional Playwright artifacts live beneath
Cargo metadata's target_directory/package-smoke/<GUID>. They are retained for
inspection. No caller environment variable, Cargo location or TEMP path changes.
REST content and a WebSocket ACK must survive a forced server process restart;
retrying that operation after restart must ACK without another version change.
An upload staged before the same restart must retain its first chunk, reject a
conflicting retry and complete exactly once with the assembled bytes.
Browser tests use repository test code, but the application still runs only from
its package; node receives its own API_URL/E2E_EXTERNAL_SERVER/artifacts values.
.EXAMPLE
./scripts/test-package.ps1 -ArchivePath ./target/dist/syntra-x86_64-pc-windows-msvc.zip -BrowserTests
.EXAMPLE
./scripts/test-package.ps1 -Image syntra-smoke:local -BrowserTests
#>
[CmdletBinding(DefaultParameterSetName = 'Archive')]
param(
    [Parameter(ParameterSetName = 'Archive')]
    [string]$ArchivePath = '',
    [Parameter(ParameterSetName = 'Container', Mandatory)]
    [ValidatePattern('^[a-zA-Z0-9][a-zA-Z0-9_./:@-]*$')]
    [string]$Image,
    [switch]$BrowserTests
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. "$PSScriptRoot/workspace.ps1"

function Assert-SyntraSmoke {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Expand-SyntraSmokeArchive {
    param([string]$Archive, [string]$Destination)
    # Extract ordinary files ourselves: reject traversal, absolute paths,
    # symlinks and duplicate destinations before creating any archive entry.
    $root = [IO.Path]::GetFullPath($Destination)
    $prefix = $root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $comparer = if ($IsWindows) { [StringComparer]::OrdinalIgnoreCase } else { [StringComparer]::Ordinal }
    $seen = [Collections.Generic.HashSet[string]]::new($comparer)
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        Assert-SyntraSmoke ($zip.Entries.Count -le 20000) 'Archive has too many entries.'
        $entries = [Collections.Generic.List[object]]::new()
        [long]$total = 0
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('\', '/')
            $segments = $name.Split('/', [StringSplitOptions]::RemoveEmptyEntries)
            Assert-SyntraSmoke (-not [string]::IsNullOrWhiteSpace($name)) 'Archive contains an empty path.'
            Assert-SyntraSmoke (-not $name.StartsWith('/') -and -not $name.Contains(':') -and -not $name.Contains([char]0)) "Unsafe archive path: $name"
            Assert-SyntraSmoke (-not ($segments | Where-Object { $_ -eq '.' -or $_ -eq '..' })) "Unsafe archive path: $name"
            $kind = ($entry.ExternalAttributes -shr 16) -band 0xF000
            Assert-SyntraSmoke ($kind -ne 0xA000) "Archive symlink is not permitted: $name"
            $target = [IO.Path]::GetFullPath((Join-Path $root $name))
            Assert-SyntraSmoke ($target.StartsWith($prefix, $comparison)) "Archive path escapes extraction root: $name"
            Assert-SyntraSmoke ($seen.Add($target)) "Duplicate archive path: $name"
            $total += $entry.Length
            Assert-SyntraSmoke ($total -le 2GB) 'Expanded archive exceeds the smoke-test size limit.'
            $entries.Add([pscustomobject]@{ Entry = $entry; Target = $target; Directory = $name.EndsWith('/') })
        }
        [IO.Directory]::CreateDirectory($root) | Out-Null
        foreach ($item in $entries) {
            if ($item.Directory) { [IO.Directory]::CreateDirectory($item.Target) | Out-Null }
            else {
                [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($item.Target)) | Out-Null
                [IO.Compression.ZipFileExtensions]::ExtractToFile($item.Entry, $item.Target, $false)
            }
        }
    }
    finally { $zip.Dispose() }
}

function Start-SyntraSmokeProcess {
    param([string]$Executable, [string]$Directory, [string[]]$Arguments, [hashtable]$Environment, [string]$Label)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.WorkingDirectory = $Directory
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    # Never inherit a caller's production database/storage/AI configuration.
    # These removals affect only the child; CARGO_*, TEMP and TMP stay inherited.
    foreach ($key in @($start.Environment.Keys)) {
        if ($key -like 'SYNTRA_*' -or $key -like ('CON' + 'TRIX_*') -or $key -in @('APP_ENV', 'JWT_SECRET')) {
            $start.Environment.Remove($key) | Out-Null
        }
    }
    foreach ($key in $Environment.Keys) { $start.Environment[$key] = [string]$Environment[$key] }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    if (-not $process.Start()) { $process.Dispose(); throw "Could not start packaged process: $Label" }
    $record = [pscustomobject]@{
        Process = $process; Label = $Label; Stopped = $false
        Output = $null; Error = $null
    }
    $taskOwnedProcesses.Add($record)
    # Drain both streams immediately so a full pipe cannot block server startup.
    $record.Output = $process.StandardOutput.ReadToEndAsync()
    $record.Error = $process.StandardError.ReadToEndAsync()
    return $record
}

function Stop-SyntraSmokeProcess {
    param($Record)
    if ($Record.Stopped) { return }
    try {
        if (-not $Record.Process.HasExited) { $Record.Process.Kill($true) }
        if (-not $Record.Process.WaitForExit(10000)) { throw "Owned process did not exit: $($Record.Label)" }
        foreach ($stream in @(@{ Task = $Record.Output; Suffix = 'stdout' }, @{ Task = $Record.Error; Suffix = 'stderr' })) {
            $text = ''
            if ($null -ne $stream.Task -and $stream.Task.Wait(5000)) { $text = $stream.Task.GetAwaiter().GetResult() }
            [IO.File]::WriteAllText((Join-Path $taskLogs "$($Record.Label).$($stream.Suffix).log"), $text)
        }
        $Record.Stopped = $true
    }
    finally {
        if ($Record.Stopped) { $Record.Process.Dispose() }
    }
}

function Get-SyntraSmokePort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try { $listener.Start(); return $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
}

function Invoke-SyntraSmokeRequest {
    param([string]$Method, [string]$Path, $Payload = $null, [string]$Token = '')
    Assert-SyntraSmoke ($Path.StartsWith('/') -and -not $Path.StartsWith('//')) 'Only local server paths are allowed.'
    $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::new($Method), "$taskBaseUrl$Path")
    $response = $null
    try {
        if ($Token) { $request.Headers.Authorization = [Net.Http.Headers.AuthenticationHeaderValue]::new('Bearer', $Token) }
        if ($Payload -is [Net.Http.HttpContent]) {
            $request.Content = $Payload
        } elseif ($null -ne $Payload) {
            $json = ConvertTo-Json -InputObject $Payload -Depth 100 -Compress
            $request.Content = [Net.Http.StringContent]::new($json, [Text.Encoding]::UTF8, 'application/json')
        }
        $response = $taskClient.SendAsync($request).GetAwaiter().GetResult()
        $bytes = $response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
        $contentType = if ($response.Content.Headers.ContentType) { $response.Content.Headers.ContentType.ToString() } else { '' }
        return [pscustomobject]@{ Status = [int]$response.StatusCode; Bytes = $bytes; ContentType = $contentType }
    }
    finally {
        if ($null -ne $response) { $response.Dispose() }
        $request.Dispose()
    }
}

function Invoke-SyntraSmokeJson {
    param([string]$Method, [string]$Path, [int]$ExpectedStatus, $Payload = $null, [string]$Token = '')
    $result = Invoke-SyntraSmokeRequest $Method $Path $Payload $Token
    Assert-SyntraSmoke ($result.Status -eq $ExpectedStatus) "$Method $Path returned $($result.Status), expected $ExpectedStatus."
    $json = [Text.Encoding]::UTF8.GetString($result.Bytes) | ConvertFrom-Json -Depth 100
    Assert-SyntraSmoke ($json.success -eq $true) "$Method $Path reported an API failure."
    return $json.data
}

function Send-SyntraSmokeUploadChunk {
    param([string]$SessionPath, [int]$Index, [byte[]]$Bytes, [string]$Token, [int]$ExpectedStatus)
    $multipart = [Net.Http.MultipartFormDataContent]::new()
    try {
        $part = [Net.Http.ByteArrayContent]::new($Bytes)
        $part.Headers.ContentType = [Net.Http.Headers.MediaTypeHeaderValue]::new('application/octet-stream')
        $multipart.Add($part, 'chunk', 'part.bin')
        $path = "$SessionPath/chunks?chunk_index=$Index"
        $result = Invoke-SyntraSmokeRequest 'POST' $path $multipart $Token
        Assert-SyntraSmoke ($result.Status -eq $ExpectedStatus) "Chunk $Index returned $($result.Status), expected $ExpectedStatus."
        $json = [Text.Encoding]::UTF8.GetString($result.Bytes) | ConvertFrom-Json -Depth 100
        if ($ExpectedStatus -eq 200) {
            Assert-SyntraSmoke ($json.success -eq $true -and $json.data.chunk_number -eq $Index) 'Chunk response did not acknowledge the requested index.'
            return [long]$json.data.uploaded_size
        }
        Assert-SyntraSmoke ($json.success -eq $false -and $json.error.code -eq 409 -and $json.error.message.Contains('Chunk index already has different data')) 'Conflicting chunk bytes did not produce the overwrite conflict.'
    }
    finally { $multipart.Dispose() }
}

function Open-SyntraSmokeSocket {
    param([string]$DocumentId, [string]$Token)
    $socket = [Net.WebSockets.ClientWebSocket]::new()
    $taskOwnedSockets.Add($socket)
    $socket.Options.Proxy = $null
    $deadline = [Threading.CancellationTokenSource]::new(10000)
    try {
        $uri = [uri]($taskBaseUrl.Replace('http://', 'ws://') + "/api/v1/ws/collab/${DocumentId}?token=" + [uri]::EscapeDataString($Token))
        # PowerShell can surface a Task's internal VoidTaskResult; discard it
        # so the helper returns exactly the ClientWebSocket and no extra item.
        $null = $socket.ConnectAsync($uri, $deadline.Token).GetAwaiter().GetResult()
        return $socket
    }
    finally { $deadline.Dispose() }
}

function Send-SyntraSmokeSocket {
    param([Net.WebSockets.ClientWebSocket]$Socket, [string]$Json)
    $deadline = [Threading.CancellationTokenSource]::new(10000)
    try {
        $segment = [ArraySegment[byte]]::new([Text.Encoding]::UTF8.GetBytes($Json))
        $null = $Socket.SendAsync($segment, [Net.WebSockets.WebSocketMessageType]::Text, $true, $deadline.Token).GetAwaiter().GetResult()
    }
    finally { $deadline.Dispose() }
}

function Wait-SyntraSmokeSocket {
    param([Net.WebSockets.ClientWebSocket]$Socket, [string]$ExpectedType, [switch]$RejectSnapshots)
    # One deadline covers all fragments and intervening participant messages.
    # Bound the response size rather than accumulating arbitrary server data.
    $deadline = [Threading.CancellationTokenSource]::new(10000)
    try {
        while ($true) {
            $buffer = [byte[]]::new(8192)
            $message = [IO.MemoryStream]::new()
            try {
                do {
                    $received = $Socket.ReceiveAsync([ArraySegment[byte]]::new($buffer), $deadline.Token).GetAwaiter().GetResult()
                    Assert-SyntraSmoke ($received.MessageType -eq [Net.WebSockets.WebSocketMessageType]::Text) 'Collaboration socket closed or returned a non-text message.'
                    Assert-SyntraSmoke ($message.Length + $received.Count -le 1MB) 'Collaboration response exceeds the smoke-test limit.'
                    $message.Write($buffer, 0, $received.Count)
                } until ($received.EndOfMessage)
                $json = [Text.Encoding]::UTF8.GetString($message.ToArray()) | ConvertFrom-Json -Depth 100
            }
            finally { $message.Dispose() }
            if ($json.type -eq 'Error') { throw "Collaboration returned an error: $($json.message)" }
            if ($json.type -eq $ExpectedType) { return $json }
            Assert-SyntraSmoke (-not $RejectSnapshots -or $json.type -ne 'DocumentState') 'Replayed operation published another document snapshot before its ACK.'
            Assert-SyntraSmoke ($json.type -in @('UserJoined', 'UserLeft', 'CursorUpdate', 'DocumentState')) "Unexpected collaboration response while waiting for ${ExpectedType}: $($json.type)"
        }
    }
    finally { $deadline.Dispose() }
}

function Start-SyntraSmokeServer {
    param([int]$Round)
    if ($Image) {
        if ($Round -eq 1) {
            $container = Invoke-SyntraSmokeDocker create --pull never --name "syntra-smoke-$taskNonce" `
                --publish '127.0.0.1::8080' --mount "type=volume,src=$taskVolume,dst=/data" `
                --env "JWT_SECRET=$taskSecret" $taskImageId
            Assert-SyntraSmoke ($container -match '^[0-9a-f]{64}$') 'Docker did not return the owned container ID.'
            $script:taskContainer = $container
        }
        $null = Invoke-SyntraSmokeDocker start $taskContainer
        $portMapping = Invoke-SyntraSmokeDocker port $taskContainer '8080/tcp'
        Assert-SyntraSmoke ($portMapping -match '^127\.0\.0\.1:(\d+)$') 'Container requires exactly one loopback port mapping.'
        $script:taskBaseUrl = "http://$portMapping"
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while ($watch.Elapsed.TotalSeconds -lt 45) {
            Assert-SyntraSmoke (Test-SyntraSmokeServerRunning $null) 'Container exited during startup.'
            try {
                $ready = Invoke-SyntraSmokeJson 'GET' '/api/v1/ready' 200
                if ($ready.status -eq 'ready') { return }
            }
            catch { if (-not (Test-SyntraSmokeServerRunning $null)) { throw } }
            Start-Sleep -Milliseconds 200
        }
        throw 'Container did not become ready within 45 seconds.'
    }
    $port = Get-SyntraSmokePort
    # Write an isolated overlay in the copied package, using stable paths and
    # secret across both starts. The caller's config files are never modified.
    $database = (Join-Path $taskRuntime 'office.sqlite').Replace('\', '/') | ConvertTo-Json -Compress
    $storage = (Join-Path $taskRuntime 'files').Replace('\', '/') | ConvertTo-Json -Compress
    @"
[server]
host = "127.0.0.1"
port = $port
cors_origins = []
[database]
url = $database
max_connections = 5
[auth]
jwt_secret = "$taskSecret"
[storage]
backend = "local"
base_path = $storage
max_upload_size_mb = 10
default_quota_mb = 100
[ai]
enabled = false
[trash]
auto_delete_enabled = false
[rate_limit]
enabled = false
"@ | Set-Content -LiteralPath (Join-Path $taskPackage 'config/package-smoke.toml') -Encoding utf8NoBOM
    $record = Start-SyntraSmokeProcess -Executable $taskExecutable -Directory $taskPackage -Arguments @() -Environment @{ SYNTRA_ENV = 'package-smoke' } -Label "server-$Round"
    $script:taskBaseUrl = "http://127.0.0.1:$port"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt 45) {
        Assert-SyntraSmoke (-not $record.Process.HasExited) "Packaged server exited during startup; inspect $taskLogs."
        try {
            $ready = Invoke-SyntraSmokeJson 'GET' '/api/v1/ready' 200
            $probe = Invoke-SyntraSmokeRequest 'GET' "/$taskProbeName"
            if ($ready.status -eq 'ready' -and $probe.Status -eq 200 -and [Text.Encoding]::UTF8.GetString($probe.Bytes) -eq $taskNonce) {
                # The unpredictable static probe proves this is our extracted
                # package, rather than a service that won a free-port race.
                Assert-SyntraSmoke (-not $record.Process.HasExited) 'Packaged server exited after readiness.'
                return $record
            }
        }
        catch { if ($record.Process.HasExited) { throw } }
        Start-Sleep -Milliseconds 200
    }
    throw "Packaged server did not become ready within 45 seconds; inspect $taskLogs."
}

function Invoke-SyntraSmokeDocker {
    $output = & docker @args
    if ($LASTEXITCODE -ne 0) { throw "Docker $($args[0]) failed (exit $LASTEXITCODE)." }
    return ($output -join "`n").Trim()
}

function Test-SyntraSmokeServerRunning {
    param($Record)
    if ($Image) { return (Invoke-SyntraSmokeDocker inspect --format '{{.State.Running}}' $taskContainer) -eq 'true' }
    return -not $Record.Process.HasExited
}

function Stop-SyntraSmokeServer {
    param($Record)
    if ($Image) {
        $null = Invoke-SyntraSmokeDocker kill --signal KILL $taskContainer
        Assert-SyntraSmoke ((Invoke-SyntraSmokeDocker inspect --format '{{.State.ExitCode}}' $taskContainer) -eq '137') 'Container did not exit after SIGKILL.'
    } else { Stop-SyntraSmokeProcess $Record }
}

function Assert-SyntraSmokeAsset {
    param([string]$Path, [string]$Mime)
    Assert-SyntraSmoke (-not $Path.Contains('..') -and -not $Path.Contains('\') -and -not $Path.Contains(':')) "Unsafe asset URL: $Path"
    $relative = if ($Path -eq '/') { 'index.html' } else { $Path.TrimStart('/') }
    if ($Image) {
        $digest = Invoke-SyntraSmokeDocker exec $taskContainer sha256sum "/app/static/$relative"
        Assert-SyntraSmoke ($digest -match '^[0-9a-f]{64}\s') "Image asset hash is missing: $Path"
        $expectedHash = ($digest -split '\s+')[0]
    } else {
        $file = Join-Path (Join-Path $taskPackage 'static') $relative
        Assert-SyntraSmoke (Test-Path -LiteralPath $file -PathType Leaf) "Asset missing from ZIP: $Path"
        $expectedHash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
    }
    $result = Invoke-SyntraSmokeRequest 'GET' $Path
    Assert-SyntraSmoke ($result.Status -eq 200 -and $result.ContentType.Contains($Mime) -and $result.Bytes.Length -gt 0) "Packaged asset failed HTTP validation: $Path"
    $actualHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$result.Bytes))
    Assert-SyntraSmoke ($actualHash -eq $expectedHash) "HTTP asset differs from its distribution contents: $Path"
    $taskAssets.Add($Path)
    return $result
}

$taskWorkspace = Split-Path -Parent $PSScriptRoot
Push-Location $taskWorkspace
try { $taskMetadata = Get-SyntraWorkspaceMetadata }
finally { Pop-Location }
$taskArchive = $null
$taskImageId = $null
if ($Image) {
    $taskImageId = Invoke-SyntraSmokeDocker image inspect --format '{{.Id}}' $Image
    Assert-SyntraSmoke ($taskImageId -match '^sha256:[0-9a-f]{64}$') 'Image must identify an existing local image.'
} elseif (-not $ArchivePath) {
    $hostDescription = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw 'rustc host discovery failed; provide -ArchivePath.' }
    $hostLine = $hostDescription | Select-String '^host: '
    if (-not $hostLine) { throw 'rustc did not report its host target; provide -ArchivePath.' }
    $taskHost = $hostLine.ToString().Substring(6).Trim()
    $ArchivePath = Join-Path $taskMetadata.target_directory "dist/syntra-$taskHost.zip"
}
if (-not $Image) {
    $taskArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
    Assert-SyntraSmoke ((Get-Item -LiteralPath $taskArchive).PSIsContainer -eq $false -and [IO.Path]::GetExtension($taskArchive) -eq '.zip') 'ArchivePath must identify an existing ZIP.'
}
$taskRoot = [IO.Path]::GetFullPath((Join-Path $taskMetadata.target_directory ("package-smoke/" + [guid]::NewGuid().ToString('N'))))
$taskPackage = Join-Path $taskRoot 'package'
$taskRuntime = Join-Path $taskRoot 'runtime'
$taskLogs = Join-Path $taskRoot 'logs'
$taskOwnedProcesses = [Collections.Generic.List[object]]::new()
$taskOwnedSockets = [Collections.Generic.List[Net.WebSockets.ClientWebSocket]]::new()
$taskAssets = [Collections.Generic.List[string]]::new()
$taskClient = $null
$taskBaseUrl = ''
$taskNonce = [guid]::NewGuid().ToString('N')
$taskVolume = "syntra-smoke-$taskNonce"
$taskVolumeCreated = $false
$taskContainer = $null
$taskProbeName = "package-smoke-$taskNonce.txt"
$taskSecret = [Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(48))
[IO.Directory]::CreateDirectory($taskRuntime) | Out-Null
[IO.Directory]::CreateDirectory($taskLogs) | Out-Null
Write-Host "Package smoke artifacts: $taskRoot"
try {
    if ($Image) {
        $volume = Invoke-SyntraSmokeDocker volume create $taskVolume
        Assert-SyntraSmoke ($volume -eq $taskVolume) 'Docker did not create the owned volume.'
        $taskVolumeCreated = $true
    } else {
        Expand-SyntraSmokeArchive $taskArchive $taskPackage
        Assert-SyntraSmoke (-not (Test-Path -LiteralPath (Join-Path $taskPackage 'Cargo.toml')) -and -not (Test-Path -LiteralPath (Join-Path $taskPackage 'crates'))) 'Release ZIP unexpectedly contains an application source workspace.'
        $taskExecutable = Join-Path $taskPackage $(if ($IsWindows) { 'syntra-server.exe' } else { 'syntra-server' })
        foreach ($required in @($taskExecutable, (Join-Path $taskPackage 'config/default.toml'), (Join-Path $taskPackage 'migrations/025_atomic_persistence.sql'), (Join-Path $taskPackage 'static/index.html'))) {
            Assert-SyntraSmoke (Test-Path -LiteralPath $required -PathType Leaf) "Required package file is missing: $required"
        }
        if (-not $IsWindows) {
            [IO.File]::SetUnixFileMode($taskExecutable, [IO.UnixFileMode]::UserRead -bor [IO.UnixFileMode]::UserWrite -bor [IO.UnixFileMode]::UserExecute -bor [IO.UnixFileMode]::GroupRead -bor [IO.UnixFileMode]::GroupExecute -bor [IO.UnixFileMode]::OtherRead -bor [IO.UnixFileMode]::OtherExecute)
        }
        [IO.File]::WriteAllText((Join-Path (Join-Path $taskPackage 'static') $taskProbeName), $taskNonce)
    }
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $handler.AllowAutoRedirect = $false
    $taskClient = [Net.Http.HttpClient]::new($handler)
    $taskClient.Timeout = [TimeSpan]::FromSeconds(10)
    $first = Start-SyntraSmokeServer 1
    if ($Image) {
        $uid = Invoke-SyntraSmokeDocker exec $taskContainer id -u
        Assert-SyntraSmoke ($uid -match '^[1-9][0-9]*$') 'The image must run as a non-root user.'
        $null = Invoke-SyntraSmokeDocker exec $taskContainer sh -ec 'test ! -e /app/Cargo.toml && test ! -d /app/crates && test -x /app/syntra-server && test -f /app/config/production.toml && test -f /app/migrations/025_atomic_persistence.sql'
    }
    $html = Assert-SyntraSmokeAsset '/' 'text/html'
    # '/' must match the index.html bytes shipped in this ZIP.
    $htmlText = [Text.Encoding]::UTF8.GetString($html.Bytes)
    Assert-SyntraSmoke ($htmlText.Contains('/bootstrap.js')) 'HTML does not reference the frontend bootstrap.'
    Assert-SyntraSmoke ($htmlText.Contains('<title>Syntra Office</title>')) 'HTML does not use the Syntra Office product name.'
    $manifest = Assert-SyntraSmokeAsset '/manifest.webmanifest' 'application/manifest+json'
    $branding = [Text.Encoding]::UTF8.GetString($manifest.Bytes) | ConvertFrom-Json
    Assert-SyntraSmoke ($branding.name -ceq 'Syntra Office' -and $branding.short_name -ceq 'Syntra') 'PWA manifest does not use the Syntra Office product name.'
    foreach ($icon in $branding.icons) { $null = Assert-SyntraSmokeAsset $icon.src $icon.type }
    $null = Assert-SyntraSmokeAsset '/icons/syntra-dark.svg' 'image/svg+xml'
    $null = Assert-SyntraSmokeAsset '/favicon.ico' 'image/x-icon'
    $null = Assert-SyntraSmokeAsset '/bootstrap.js' 'javascript'
    $null = Assert-SyntraSmokeAsset '/assets/calc-worker.js' 'javascript'
    $glue = Assert-SyntraSmokeAsset '/assets/syntra_frontend.js' 'javascript'
    Assert-SyntraSmoke ([Text.Encoding]::UTF8.GetString($glue.Bytes).Contains('syntra_frontend_bg.wasm')) 'JavaScript glue does not reference the packaged WASM.'
    $wasm = Assert-SyntraSmokeAsset '/assets/syntra_frontend_bg.wasm' 'application/wasm'
    Assert-SyntraSmoke ($wasm.Bytes.Length -ge 8 -and [BitConverter]::ToString($wasm.Bytes, 0, 8) -eq '00-61-73-6D-01-00-00-00') 'Packaged frontend is not a WASM v1 module.'
    $styles = @([regex]::Matches($htmlText, '<link\b[^>]*href="([^"]+\.css)"') | ForEach-Object { $_.Groups[1].Value })
    Assert-SyntraSmoke ($styles -contains '/assets/main.css' -and $styles -contains '/assets/sapid.css') 'HTML must include syntra and Sapid styles.'
    foreach ($style in $styles) { $null = Assert-SyntraSmokeAsset $style 'text/css' }

    $email = "package_$taskNonce@example.test"
    $password = "Package!9-$taskNonce"
    $registered = Invoke-SyntraSmokeJson 'POST' '/api/v1/auth/register' 201 @{ username = "package_$taskNonce"; email = $email; password = $password }
    Assert-SyntraSmoke (-not [string]::IsNullOrWhiteSpace($registered.token)) 'Registration did not issue a token.'
    $loggedIn = Invoke-SyntraSmokeJson 'POST' '/api/v1/auth/login' 200 @{ email = $email; password = $password }
    $token = [string]$loggedIn.token
    $document = Invoke-SyntraSmokeJson 'POST' '/api/v1/documents' 201 @{ name = 'Packaged writer'; document_type = 'writer' } $token
    $marker = "Packaged persistence $taskNonce 世界"
    $content = $document.content
    $content.title = $marker
    $saved = Invoke-SyntraSmokeJson 'PUT' "/api/v1/documents/$($document.id)" 200 @{ expected_version = [int]$document.version; content = $content } $token
    Assert-SyntraSmoke ($saved.version -eq ([int]$document.version + 1)) 'Save did not advance the version.'
    $download = Invoke-SyntraSmokeRequest 'GET' "/api/v1/files/$($document.file_id)/download" $null $token
    Assert-SyntraSmoke ($download.Status -eq 200) 'Saved document download failed.'
    $downloaded = [Text.Encoding]::UTF8.GetString($download.Bytes) | ConvertFrom-Json -Depth 100
    Assert-SyntraSmoke ($downloaded.title -eq $marker) 'Downloaded blob does not contain the saved document.'

    # A separate account isolates upload quota from the document and WS saves.
    # Stage just one chunk before the same forced restart used by the WS ACK.
    $uploadEmail = "upload_$taskNonce@example.test"
    $uploadPassword = "Upload!9-$taskNonce"
    $uploadAccount = Invoke-SyntraSmokeJson 'POST' '/api/v1/auth/register' 201 @{ username = "upload_$taskNonce"; email = $uploadEmail; password = $uploadPassword }
    $uploadToken = [string]$uploadAccount.token
    $quotaBaseline = [long]((Invoke-SyntraSmokeJson 'GET' '/api/v1/quota' 200 $null $uploadToken).storage_used)
    $firstChunk = [Security.Cryptography.RandomNumberGenerator]::GetBytes(8192)
    $secondChunk = [Security.Cryptography.RandomNumberGenerator]::GetBytes(12017)
    $uploadBytes = [byte[]]::new($firstChunk.Length + $secondChunk.Length)
    [Buffer]::BlockCopy($firstChunk, 0, $uploadBytes, 0, $firstChunk.Length)
    [Buffer]::BlockCopy($secondChunk, 0, $uploadBytes, $firstChunk.Length, $secondChunk.Length)
    $uploadSession = Invoke-SyntraSmokeJson 'POST' '/api/v1/upload/sessions' 201 @{ file_name = 'resume.bin'; total_size = $uploadBytes.Length; mime_type = 'application/octet-stream' } $uploadToken
    Assert-SyntraSmoke (-not [string]::IsNullOrWhiteSpace($uploadSession.id)) 'Upload creation did not return a session ID.'
    $uploadPath = "/api/v1/upload/sessions/$($uploadSession.id)"
    $beforeRestartProgress = Send-SyntraSmokeUploadChunk $uploadPath 1 $firstChunk $uploadToken 200
    $beforeRestartQuota = [long]((Invoke-SyntraSmokeJson 'GET' '/api/v1/quota' 200 $null $uploadToken).storage_used)
    Assert-SyntraSmoke ($beforeRestartProgress -eq $firstChunk.Length -and $beforeRestartQuota -eq $quotaBaseline) 'The first chunk reported incorrect progress or changed charged quota.'

    # Keep the REST document independent so both persistence paths are checked.
    # Kill the actual server immediately after its WebSocket ACK, with the
    # connection still open: an in-memory ACK cannot pass the second startup.
    $collaborative = Invoke-SyntraSmokeJson 'POST' '/api/v1/documents' 201 @{ name = 'Packaged collaboration'; document_type = 'writer' } $token
    $sender = Open-SyntraSmokeSocket ([string]$collaborative.id) $token
    $initial = Wait-SyntraSmokeSocket $sender 'DocumentState'
    Assert-SyntraSmoke ($initial.version -eq $collaborative.version -and $initial.content.title -eq $collaborative.content.title) 'Initial WebSocket snapshot differs from the durable document.'
    $wsMarker = "ACK survives process restart $taskNonce 世界"
    $replacement = $initial.content | ConvertTo-Json -Depth 100 -Compress | ConvertFrom-Json -Depth 100
    $replacement.title = $wsMarker
    $opId = @{ counter = 1; client_id = [guid]::NewGuid().ToString() }
    $operation = @{
        type = 'Operation'; id = $opId; timestamp = [DateTimeOffset]::UtcNow.ToString('o')
        user_id = [string]$registered.user.id; base_version = [long]$initial.version
        op_type = @{ type = 'Replace'; path = @(); old_value = $initial.content; new_value = $replacement }
    } | ConvertTo-Json -Depth 100 -Compress
    [IO.File]::WriteAllText((Join-Path $taskRoot 'websocket-operation.json'), $operation)
    Send-SyntraSmokeSocket $sender $operation
    $ack = Wait-SyntraSmokeSocket $sender 'Ack'
    Assert-SyntraSmoke ($ack.op_id.client_id -eq $opId.client_id -and $ack.op_id.counter -eq $opId.counter) 'ACK identifies a different operation.'
    Stop-SyntraSmokeServer $first
    $sender.Abort()

    $second = Start-SyntraSmokeServer 2
    $again = Invoke-SyntraSmokeJson 'POST' '/api/v1/auth/login' 200 @{ email = $email; password = $password }
    $persisted = Invoke-SyntraSmokeJson 'GET' "/api/v1/documents/$($document.id)" 200 $null ([string]$again.token)
    Assert-SyntraSmoke ($persisted.version -eq $saved.version -and $persisted.content.title -eq $marker) 'Document content/version did not survive a packaged server restart.'
    $download = Invoke-SyntraSmokeRequest 'GET' "/api/v1/files/$($document.file_id)/download" $null ([string]$again.token)
    Assert-SyntraSmoke ($download.Status -eq 200 -and ([Text.Encoding]::UTF8.GetString($download.Bytes) | ConvertFrom-Json -Depth 100).title -eq $marker) 'Backing file did not survive restart.'

    $reconnected = Open-SyntraSmokeSocket ([string]$collaborative.id) ([string]$again.token)
    $recovered = Wait-SyntraSmokeSocket $reconnected 'DocumentState'
    $wsVersion = [long]$initial.version + 1
    Assert-SyntraSmoke ($recovered.version -eq $wsVersion -and $recovered.content.title -eq $wsMarker) 'WebSocket ACK content/version did not survive the process restart.'
    $recovered | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath (Join-Path $taskRoot 'websocket-restarted-snapshot.json') -Encoding utf8NoBOM
    # Send identical bytes, including the original base version and operation
    # id. A lost receipt would execute Replace again and increment the version.
    Send-SyntraSmokeSocket $reconnected $operation
    $retried = Wait-SyntraSmokeSocket $reconnected 'Ack' -RejectSnapshots
    Assert-SyntraSmoke ($retried.op_id.client_id -eq $opId.client_id -and $retried.op_id.counter -eq $opId.counter) 'Retry ACK identifies a different operation.'
    $afterRetry = Invoke-SyntraSmokeJson 'GET' "/api/v1/documents/$($collaborative.id)" 200 $null ([string]$again.token)
    Assert-SyntraSmoke ($afterRetry.version -eq $wsVersion -and $afterRetry.content.title -eq $wsMarker) 'Replayed WebSocket operation changed durable content/version after restart.'
    Send-SyntraSmokeSocket $reconnected (@{ type = 'SyncRequest'; from_version = 0 } | ConvertTo-Json -Compress)
    $synced = Wait-SyntraSmokeSocket $reconnected 'DocumentState'
    Assert-SyntraSmoke ($synced.version -eq $wsVersion -and $synced.content.title -eq $wsMarker) 'WebSocket retry changed the recovered collaboration snapshot.'
    $wsDownload = Invoke-SyntraSmokeRequest 'GET' "/api/v1/files/$($collaborative.file_id)/download" $null ([string]$again.token)
    Assert-SyntraSmoke ($wsDownload.Status -eq 200 -and ([Text.Encoding]::UTF8.GetString($wsDownload.Bytes) | ConvertFrom-Json -Depth 100).title -eq $wsMarker) 'Acknowledged WebSocket blob did not survive restart and retry.'

    $resumedAccount = Invoke-SyntraSmokeJson 'POST' '/api/v1/auth/login' 200 @{ email = $uploadEmail; password = $uploadPassword }
    $resumedToken = [string]$resumedAccount.token
    # Never retransmit the original first chunk. The download must include its
    # durable pre-restart bytes even after a conflicting upload to that index.
    $replacementChunk = [byte[]]$firstChunk.Clone()
    $replacementChunk[0] = [byte]($replacementChunk[0] -bxor 1)
    Send-SyntraSmokeUploadChunk $uploadPath 1 $replacementChunk $resumedToken 409
    $resumedProgress = Send-SyntraSmokeUploadChunk $uploadPath 2 $secondChunk $resumedToken 200
    $repeatedProgress = Send-SyntraSmokeUploadChunk $uploadPath 2 $secondChunk $resumedToken 200
    $beforeCompleteQuota = [long]((Invoke-SyntraSmokeJson 'GET' '/api/v1/quota' 200 $null $resumedToken).storage_used)
    Assert-SyntraSmoke ($resumedProgress -eq $uploadBytes.Length -and $repeatedProgress -eq $resumedProgress -and $beforeCompleteQuota -eq $quotaBaseline) 'Resume or retry changed assembled progress or charged quota incorrectly.'
    $completedUpload = Invoke-SyntraSmokeJson 'POST' "$uploadPath/complete" 201 $null $resumedToken
    $completedQuota = [long]((Invoke-SyntraSmokeJson 'GET' '/api/v1/quota' 200 $null $resumedToken).storage_used)
    $replayedUpload = Invoke-SyntraSmokeJson 'POST' "$uploadPath/complete" 200 $null $resumedToken
    $replayedQuota = [long]((Invoke-SyntraSmokeJson 'GET' '/api/v1/quota' 200 $null $resumedToken).storage_used)
    Assert-SyntraSmoke (-not [string]::IsNullOrWhiteSpace($completedUpload.id) -and $replayedUpload.id -eq $completedUpload.id) 'Completion retry returned a different file ID.'
    Assert-SyntraSmoke ($completedQuota - $quotaBaseline -eq $uploadBytes.Length -and $replayedQuota -eq $completedQuota) 'Completed upload was not charged exactly once.'
    $uploadedFiles = @(Invoke-SyntraSmokeJson 'GET' '/api/v1/files' 200 $null $resumedToken)
    Assert-SyntraSmoke ($uploadedFiles.Count -eq 1 -and $uploadedFiles[0].id -eq $completedUpload.id) 'Upload completion did not leave exactly one file.'
    $uploadDownload = Invoke-SyntraSmokeRequest 'GET' "/api/v1/files/$($completedUpload.id)/download" $null $resumedToken
    Assert-SyntraSmoke ($uploadDownload.Status -eq 200 -and [Convert]::ToBase64String([byte[]]$uploadDownload.Bytes) -ceq [Convert]::ToBase64String($uploadBytes)) 'Downloaded upload differs from the ordered pre- and post-restart bytes.'

    if ($BrowserTests) {
        $cli = Join-Path $taskWorkspace 'e2e/node_modules/@playwright/test/cli.js'
        $testFile = Join-Path $taskWorkspace 'e2e/tests/package.ui.spec.ts'
        Assert-SyntraSmoke (Test-Path -LiteralPath $cli -PathType Leaf) 'Install existing e2e dependencies before requesting -BrowserTests.'
        Assert-SyntraSmoke (Test-Path -LiteralPath $testFile -PathType Leaf) 'Dedicated package UI smoke test is missing.'
        $node = (Get-Command node -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $browser = Start-SyntraSmokeProcess -Executable $node -Directory (Join-Path $taskWorkspace 'e2e') -Arguments @($cli, 'test', 'package.ui.spec.ts', '--project=ui-tests', '--workers=1', '--retries=0') -Environment @{
            API_URL = $taskBaseUrl; E2E_EXTERNAL_SERVER = '1'; SYNTRA_E2E_ARTIFACTS = (Join-Path $taskRoot 'browser')
        } -Label 'browser'
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while (-not $browser.Process.WaitForExit(1000)) {
            Assert-SyntraSmoke (Test-SyntraSmokeServerRunning $second) 'Server exited during the browser smoke test.'
            Assert-SyntraSmoke ($watch.Elapsed.TotalSeconds -lt 600) 'Browser smoke test exceeded 10 minutes.'
        }
        $browserExit = $browser.Process.ExitCode
        Stop-SyntraSmokeProcess $browser
        Assert-SyntraSmoke ($browserExit -eq 0) "Browser smoke failed (exit $browserExit); inspect $taskLogs and browser artifacts."
    }
    Stop-SyntraSmokeServer $second
    $report = [ordered]@{
        success = $true; archive = $taskArchive; package_directory = $taskPackage
        image_id = $taskImageId
        source_free_server_start = $true; restart_persistence = $true; browser_tests = [bool]$BrowserTests
        websocket_ack_restart = $true; websocket_receipt_retry = $true
        upload_chunk_restart = $true; upload_conflicting_chunk_rejected = $true; upload_completion_replay = $true
        upload_restart = [ordered]@{
            session_id = $uploadSession.id; file_id = $completedUpload.id
            first_chunk_bytes = $firstChunk.Length; second_chunk_bytes = $secondChunk.Length
            downloaded_sha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$uploadDownload.Bytes))
            quota_baseline_bytes = $quotaBaseline; quota_after_complete_bytes = $completedQuota; quota_after_replay_bytes = $replayedQuota
        }
        websocket_document_id = $collaborative.id; websocket_version = $wsVersion
        document_id = $document.id; version = $saved.version; assets = @($taskAssets)
    }
}
finally {
    foreach ($ownedSocket in $taskOwnedSockets) {
        try { $ownedSocket.Abort(); $ownedSocket.Dispose() }
        catch { Write-Warning "Could not finish cleanup for an owned WebSocket: $_" }
    }
    # Only retained process objects created by this script are stopped. Never
    # search by executable name, port or a caller-supplied PID; keep artifacts.
    foreach ($owned in $taskOwnedProcesses) {
        try { Stop-SyntraSmokeProcess $owned }
        catch { Write-Warning "Could not finish cleanup for owned process $($owned.Label): $_" }
    }
    if ($taskContainer) {
        try {
            $log = Invoke-SyntraSmokeDocker logs $taskContainer 2>&1
            [IO.File]::WriteAllLines((Join-Path $taskLogs 'container.log'), [string[]]$log)
        }
        finally { $null = Invoke-SyntraSmokeDocker rm --force $taskContainer }
    }
    if ($taskVolumeCreated) { $null = Invoke-SyntraSmokeDocker volume rm $taskVolume }
    if ($null -ne $taskClient) { $taskClient.Dispose() }
}
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $taskRoot 'result.json') -Encoding utf8NoBOM
Write-Host "Package smoke passed. Browser tests: $([bool]$BrowserTests). Artifacts: $taskRoot"
