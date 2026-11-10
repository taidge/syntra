#requires -Version 7.2
<#
.SYNOPSIS
Run an existing release ZIP without building or using application source files.
.DESCRIPTION
All server processes start in a fresh extracted package. The test's config,
SQLite database, storage, logs and optional Playwright artifacts live beneath
Cargo metadata's target_directory/package-smoke/<GUID>. They are retained for
inspection. No caller environment variable, Cargo location or TEMP path changes.
REST content and a WebSocket ACK must survive a forced server process restart;
retrying that operation after restart must ACK without another version change.
Browser tests use repository test code, but the application still runs only from
its package; node receives its own API_URL/E2E_EXTERNAL_SERVER/artifacts values.
.EXAMPLE
./scripts/test-package.ps1 -ArchivePath ./target/dist/contrix-office-x86_64-pc-windows-msvc.zip -BrowserTests
#>
param(
    [string]$ArchivePath = '',
    [switch]$BrowserTests
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. "$PSScriptRoot/workspace.ps1"

function Assert-OfficeSmoke {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Expand-OfficeSmokeArchive {
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
        Assert-OfficeSmoke ($zip.Entries.Count -le 20000) 'Archive has too many entries.'
        $entries = [Collections.Generic.List[object]]::new()
        [long]$total = 0
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('\', '/')
            $segments = $name.Split('/', [StringSplitOptions]::RemoveEmptyEntries)
            Assert-OfficeSmoke (-not [string]::IsNullOrWhiteSpace($name)) 'Archive contains an empty path.'
            Assert-OfficeSmoke (-not $name.StartsWith('/') -and -not $name.Contains(':') -and -not $name.Contains([char]0)) "Unsafe archive path: $name"
            Assert-OfficeSmoke (-not ($segments | Where-Object { $_ -eq '.' -or $_ -eq '..' })) "Unsafe archive path: $name"
            $kind = ($entry.ExternalAttributes -shr 16) -band 0xF000
            Assert-OfficeSmoke ($kind -ne 0xA000) "Archive symlink is not permitted: $name"
            $target = [IO.Path]::GetFullPath((Join-Path $root $name))
            Assert-OfficeSmoke ($target.StartsWith($prefix, $comparison)) "Archive path escapes extraction root: $name"
            Assert-OfficeSmoke ($seen.Add($target)) "Duplicate archive path: $name"
            $total += $entry.Length
            Assert-OfficeSmoke ($total -le 2GB) 'Expanded archive exceeds the smoke-test size limit.'
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

function Start-OfficeSmokeProcess {
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
        if ($key.StartsWith('CONTRIX_', [StringComparison]::OrdinalIgnoreCase) -or $key -in @('APP_ENV', 'JWT_SECRET')) {
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

function Stop-OfficeSmokeProcess {
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

function Get-OfficeSmokePort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try { $listener.Start(); return $listener.LocalEndpoint.Port }
    finally { $listener.Stop() }
}

function Invoke-OfficeSmokeRequest {
    param([string]$Method, [string]$Path, $Payload = $null, [string]$Token = '')
    Assert-OfficeSmoke ($Path.StartsWith('/') -and -not $Path.StartsWith('//')) 'Only local server paths are allowed.'
    $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::new($Method), "$taskBaseUrl$Path")
    $response = $null
    try {
        if ($Token) { $request.Headers.Authorization = [Net.Http.Headers.AuthenticationHeaderValue]::new('Bearer', $Token) }
        if ($null -ne $Payload) {
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

function Invoke-OfficeSmokeJson {
    param([string]$Method, [string]$Path, [int]$ExpectedStatus, $Payload = $null, [string]$Token = '')
    $result = Invoke-OfficeSmokeRequest $Method $Path $Payload $Token
    Assert-OfficeSmoke ($result.Status -eq $ExpectedStatus) "$Method $Path returned $($result.Status), expected $ExpectedStatus."
    $json = [Text.Encoding]::UTF8.GetString($result.Bytes) | ConvertFrom-Json -Depth 100
    Assert-OfficeSmoke ($json.success -eq $true) "$Method $Path reported an API failure."
    return $json.data
}

function Open-OfficeSmokeSocket {
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

function Send-OfficeSmokeSocket {
    param([Net.WebSockets.ClientWebSocket]$Socket, [string]$Json)
    $deadline = [Threading.CancellationTokenSource]::new(10000)
    try {
        $segment = [ArraySegment[byte]]::new([Text.Encoding]::UTF8.GetBytes($Json))
        $null = $Socket.SendAsync($segment, [Net.WebSockets.WebSocketMessageType]::Text, $true, $deadline.Token).GetAwaiter().GetResult()
    }
    finally { $deadline.Dispose() }
}

function Wait-OfficeSmokeSocket {
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
                    Assert-OfficeSmoke ($received.MessageType -eq [Net.WebSockets.WebSocketMessageType]::Text) 'Collaboration socket closed or returned a non-text message.'
                    Assert-OfficeSmoke ($message.Length + $received.Count -le 1MB) 'Collaboration response exceeds the smoke-test limit.'
                    $message.Write($buffer, 0, $received.Count)
                } until ($received.EndOfMessage)
                $json = [Text.Encoding]::UTF8.GetString($message.ToArray()) | ConvertFrom-Json -Depth 100
            }
            finally { $message.Dispose() }
            if ($json.type -eq 'Error') { throw "Collaboration returned an error: $($json.message)" }
            if ($json.type -eq $ExpectedType) { return $json }
            Assert-OfficeSmoke (-not $RejectSnapshots -or $json.type -ne 'DocumentState') 'Replayed operation published another document snapshot before its ACK.'
            Assert-OfficeSmoke ($json.type -in @('UserJoined', 'UserLeft', 'CursorUpdate', 'DocumentState')) "Unexpected collaboration response while waiting for ${ExpectedType}: $($json.type)"
        }
    }
    finally { $deadline.Dispose() }
}

function Start-OfficeSmokeServer {
    param([int]$Round)
    $port = Get-OfficeSmokePort
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
    $record = Start-OfficeSmokeProcess -Executable $taskExecutable -Directory $taskPackage -Arguments @() -Environment @{ CONTRIX_ENV = 'package-smoke' } -Label "server-$Round"
    $script:taskBaseUrl = "http://127.0.0.1:$port"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt 45) {
        Assert-OfficeSmoke (-not $record.Process.HasExited) "Packaged server exited during startup; inspect $taskLogs."
        try {
            $ready = Invoke-OfficeSmokeJson 'GET' '/api/v1/ready' 200
            $probe = Invoke-OfficeSmokeRequest 'GET' "/$taskProbeName"
            if ($ready.status -eq 'ready' -and $probe.Status -eq 200 -and [Text.Encoding]::UTF8.GetString($probe.Bytes) -eq $taskNonce) {
                # The unpredictable static probe proves this is our extracted
                # package, rather than a service that won a free-port race.
                Assert-OfficeSmoke (-not $record.Process.HasExited) 'Packaged server exited after readiness.'
                return $record
            }
        }
        catch { if ($record.Process.HasExited) { throw } }
        Start-Sleep -Milliseconds 200
    }
    throw "Packaged server did not become ready within 45 seconds; inspect $taskLogs."
}

function Assert-OfficeSmokeAsset {
    param([string]$Path, [string]$Mime)
    Assert-OfficeSmoke (-not $Path.Contains('..') -and -not $Path.Contains('\') -and -not $Path.Contains(':')) "Unsafe asset URL: $Path"
    $relative = if ($Path -eq '/') { 'index.html' } else { $Path.TrimStart('/') }
    $file = Join-Path (Join-Path $taskPackage 'static') $relative
    Assert-OfficeSmoke (Test-Path -LiteralPath $file -PathType Leaf) "Asset missing from ZIP: $Path"
    $result = Invoke-OfficeSmokeRequest 'GET' $Path
    Assert-OfficeSmoke ($result.Status -eq 200 -and $result.ContentType.Contains($Mime) -and $result.Bytes.Length -gt 0) "Packaged asset failed HTTP validation: $Path"
    $expectedHash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
    $actualHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([byte[]]$result.Bytes))
    Assert-OfficeSmoke ($actualHash -eq $expectedHash) "HTTP asset differs from its ZIP contents: $Path"
    $taskAssets.Add($Path)
    return $result
}

$taskWorkspace = Split-Path -Parent $PSScriptRoot
Push-Location $taskWorkspace
try { $taskMetadata = Get-OfficeWorkspaceMetadata }
finally { Pop-Location }
if (-not $ArchivePath) {
    $hostDescription = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw 'rustc host discovery failed; provide -ArchivePath.' }
    $hostLine = $hostDescription | Select-String '^host: '
    if (-not $hostLine) { throw 'rustc did not report its host target; provide -ArchivePath.' }
    $taskHost = $hostLine.ToString().Substring(6).Trim()
    $ArchivePath = Join-Path $taskMetadata.target_directory "dist/contrix-office-$taskHost.zip"
}
$taskArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
Assert-OfficeSmoke ((Get-Item -LiteralPath $taskArchive).PSIsContainer -eq $false -and [IO.Path]::GetExtension($taskArchive) -eq '.zip') 'ArchivePath must identify an existing ZIP.'
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
$taskProbeName = "package-smoke-$taskNonce.txt"
$taskSecret = [Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(48))
[IO.Directory]::CreateDirectory($taskRuntime) | Out-Null
[IO.Directory]::CreateDirectory($taskLogs) | Out-Null
Write-Host "Package smoke artifacts: $taskRoot"
try {
    Expand-OfficeSmokeArchive $taskArchive $taskPackage
    Assert-OfficeSmoke (-not (Test-Path -LiteralPath (Join-Path $taskPackage 'Cargo.toml')) -and -not (Test-Path -LiteralPath (Join-Path $taskPackage 'crates'))) 'Release ZIP unexpectedly contains an application source workspace.'
    $taskExecutable = Join-Path $taskPackage $(if ($IsWindows) { 'contrix-server.exe' } else { 'contrix-server' })
    foreach ($required in @($taskExecutable, (Join-Path $taskPackage 'config/default.toml'), (Join-Path $taskPackage 'migrations/025_atomic_persistence.sql'), (Join-Path $taskPackage 'static/index.html'))) {
        Assert-OfficeSmoke (Test-Path -LiteralPath $required -PathType Leaf) "Required package file is missing: $required"
    }
    if (-not $IsWindows) {
        [IO.File]::SetUnixFileMode($taskExecutable, [IO.UnixFileMode]::UserRead -bor [IO.UnixFileMode]::UserWrite -bor [IO.UnixFileMode]::UserExecute -bor [IO.UnixFileMode]::GroupRead -bor [IO.UnixFileMode]::GroupExecute -bor [IO.UnixFileMode]::OtherRead -bor [IO.UnixFileMode]::OtherExecute)
    }
    [IO.File]::WriteAllText((Join-Path (Join-Path $taskPackage 'static') $taskProbeName), $taskNonce)
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $handler.AllowAutoRedirect = $false
    $taskClient = [Net.Http.HttpClient]::new($handler)
    $taskClient.Timeout = [TimeSpan]::FromSeconds(10)
    $first = Start-OfficeSmokeServer 1
    $html = Assert-OfficeSmokeAsset '/' 'text/html'
    # '/' must match the index.html bytes shipped in this ZIP.
    $htmlText = [Text.Encoding]::UTF8.GetString($html.Bytes)
    Assert-OfficeSmoke ($htmlText.Contains('/bootstrap.js')) 'HTML does not reference the frontend bootstrap.'
    $null = Assert-OfficeSmokeAsset '/bootstrap.js' 'javascript'
    $glue = Assert-OfficeSmokeAsset '/assets/contrix_frontend.js' 'javascript'
    Assert-OfficeSmoke ([Text.Encoding]::UTF8.GetString($glue.Bytes).Contains('contrix_frontend_bg.wasm')) 'JavaScript glue does not reference the packaged WASM.'
    $wasm = Assert-OfficeSmokeAsset '/assets/contrix_frontend_bg.wasm' 'application/wasm'
    Assert-OfficeSmoke ($wasm.Bytes.Length -ge 8 -and [BitConverter]::ToString($wasm.Bytes, 0, 8) -eq '00-61-73-6D-01-00-00-00') 'Packaged frontend is not a WASM v1 module.'
    $styles = @([regex]::Matches($htmlText, '<link\b[^>]*href="([^"]+\.css)"') | ForEach-Object { $_.Groups[1].Value })
    Assert-OfficeSmoke ($styles -contains '/assets/main.css' -and $styles -contains '/assets/sapid.css') 'HTML must include Office and Sapid styles.'
    foreach ($style in $styles) { $null = Assert-OfficeSmokeAsset $style 'text/css' }

    $email = "package_$taskNonce@example.test"
    $password = "Package!9-$taskNonce"
    $registered = Invoke-OfficeSmokeJson 'POST' '/api/v1/auth/register' 201 @{ username = "package_$taskNonce"; email = $email; password = $password }
    Assert-OfficeSmoke (-not [string]::IsNullOrWhiteSpace($registered.token)) 'Registration did not issue a token.'
    $loggedIn = Invoke-OfficeSmokeJson 'POST' '/api/v1/auth/login' 200 @{ email = $email; password = $password }
    $token = [string]$loggedIn.token
    $document = Invoke-OfficeSmokeJson 'POST' '/api/v1/documents' 201 @{ name = 'Packaged writer'; document_type = 'writer' } $token
    $marker = "Packaged persistence $taskNonce 世界"
    $content = $document.content
    $content.title = $marker
    $saved = Invoke-OfficeSmokeJson 'PUT' "/api/v1/documents/$($document.id)" 200 @{ expected_version = [int]$document.version; content = $content } $token
    Assert-OfficeSmoke ($saved.version -eq ([int]$document.version + 1)) 'Save did not advance the version.'
    $download = Invoke-OfficeSmokeRequest 'GET' "/api/v1/files/$($document.file_id)/download" $null $token
    Assert-OfficeSmoke ($download.Status -eq 200) 'Saved document download failed.'
    $downloaded = [Text.Encoding]::UTF8.GetString($download.Bytes) | ConvertFrom-Json -Depth 100
    Assert-OfficeSmoke ($downloaded.title -eq $marker) 'Downloaded blob does not contain the saved document.'

    # Keep the REST document independent so both persistence paths are checked.
    # Kill the actual server immediately after its WebSocket ACK, with the
    # connection still open: an in-memory ACK cannot pass the second startup.
    $collaborative = Invoke-OfficeSmokeJson 'POST' '/api/v1/documents' 201 @{ name = 'Packaged collaboration'; document_type = 'writer' } $token
    $sender = Open-OfficeSmokeSocket ([string]$collaborative.id) $token
    $initial = Wait-OfficeSmokeSocket $sender 'DocumentState'
    Assert-OfficeSmoke ($initial.version -eq $collaborative.version -and $initial.content.title -eq $collaborative.content.title) 'Initial WebSocket snapshot differs from the durable document.'
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
    Send-OfficeSmokeSocket $sender $operation
    $ack = Wait-OfficeSmokeSocket $sender 'Ack'
    Assert-OfficeSmoke ($ack.op_id.client_id -eq $opId.client_id -and $ack.op_id.counter -eq $opId.counter) 'ACK identifies a different operation.'
    Stop-OfficeSmokeProcess $first
    $sender.Abort()

    $second = Start-OfficeSmokeServer 2
    $again = Invoke-OfficeSmokeJson 'POST' '/api/v1/auth/login' 200 @{ email = $email; password = $password }
    $persisted = Invoke-OfficeSmokeJson 'GET' "/api/v1/documents/$($document.id)" 200 $null ([string]$again.token)
    Assert-OfficeSmoke ($persisted.version -eq $saved.version -and $persisted.content.title -eq $marker) 'Document content/version did not survive a packaged server restart.'
    $download = Invoke-OfficeSmokeRequest 'GET' "/api/v1/files/$($document.file_id)/download" $null ([string]$again.token)
    Assert-OfficeSmoke ($download.Status -eq 200 -and ([Text.Encoding]::UTF8.GetString($download.Bytes) | ConvertFrom-Json -Depth 100).title -eq $marker) 'Backing file did not survive restart.'

    $reconnected = Open-OfficeSmokeSocket ([string]$collaborative.id) ([string]$again.token)
    $recovered = Wait-OfficeSmokeSocket $reconnected 'DocumentState'
    $wsVersion = [long]$initial.version + 1
    Assert-OfficeSmoke ($recovered.version -eq $wsVersion -and $recovered.content.title -eq $wsMarker) 'WebSocket ACK content/version did not survive the process restart.'
    $recovered | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath (Join-Path $taskRoot 'websocket-restarted-snapshot.json') -Encoding utf8NoBOM
    # Send identical bytes, including the original base version and operation
    # id. A lost receipt would execute Replace again and increment the version.
    Send-OfficeSmokeSocket $reconnected $operation
    $retried = Wait-OfficeSmokeSocket $reconnected 'Ack' -RejectSnapshots
    Assert-OfficeSmoke ($retried.op_id.client_id -eq $opId.client_id -and $retried.op_id.counter -eq $opId.counter) 'Retry ACK identifies a different operation.'
    $afterRetry = Invoke-OfficeSmokeJson 'GET' "/api/v1/documents/$($collaborative.id)" 200 $null ([string]$again.token)
    Assert-OfficeSmoke ($afterRetry.version -eq $wsVersion -and $afterRetry.content.title -eq $wsMarker) 'Replayed WebSocket operation changed durable content/version after restart.'
    Send-OfficeSmokeSocket $reconnected (@{ type = 'SyncRequest'; from_version = 0 } | ConvertTo-Json -Compress)
    $synced = Wait-OfficeSmokeSocket $reconnected 'DocumentState'
    Assert-OfficeSmoke ($synced.version -eq $wsVersion -and $synced.content.title -eq $wsMarker) 'WebSocket retry changed the recovered collaboration snapshot.'
    $wsDownload = Invoke-OfficeSmokeRequest 'GET' "/api/v1/files/$($collaborative.file_id)/download" $null ([string]$again.token)
    Assert-OfficeSmoke ($wsDownload.Status -eq 200 -and ([Text.Encoding]::UTF8.GetString($wsDownload.Bytes) | ConvertFrom-Json -Depth 100).title -eq $wsMarker) 'Acknowledged WebSocket blob did not survive restart and retry.'

    if ($BrowserTests) {
        $cli = Join-Path $taskWorkspace 'e2e/node_modules/@playwright/test/cli.js'
        $testFile = Join-Path $taskWorkspace 'e2e/tests/package.ui.spec.ts'
        Assert-OfficeSmoke (Test-Path -LiteralPath $cli -PathType Leaf) 'Install existing e2e dependencies before requesting -BrowserTests.'
        Assert-OfficeSmoke (Test-Path -LiteralPath $testFile -PathType Leaf) 'Dedicated package UI smoke test is missing.'
        $node = (Get-Command node -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $browser = Start-OfficeSmokeProcess -Executable $node -Directory (Join-Path $taskWorkspace 'e2e') -Arguments @($cli, 'test', 'package.ui.spec.ts', '--project=ui-tests', '--workers=1', '--retries=0') -Environment @{
            API_URL = $taskBaseUrl; E2E_EXTERNAL_SERVER = '1'; CONTRIX_E2E_ARTIFACTS = (Join-Path $taskRoot 'browser')
        } -Label 'browser'
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while (-not $browser.Process.WaitForExit(1000)) {
            Assert-OfficeSmoke (-not $second.Process.HasExited) 'Packaged server exited during the browser smoke test.'
            Assert-OfficeSmoke ($watch.Elapsed.TotalSeconds -lt 600) 'Browser smoke test exceeded 10 minutes.'
        }
        $browserExit = $browser.Process.ExitCode
        Stop-OfficeSmokeProcess $browser
        Assert-OfficeSmoke ($browserExit -eq 0) "Browser smoke failed (exit $browserExit); inspect $taskLogs and browser artifacts."
    }
    Stop-OfficeSmokeProcess $second
    $report = [ordered]@{
        success = $true; archive = $taskArchive; package_directory = $taskPackage
        source_free_server_start = $true; restart_persistence = $true; browser_tests = [bool]$BrowserTests
        websocket_ack_restart = $true; websocket_receipt_retry = $true
        websocket_document_id = $collaborative.id; websocket_version = $wsVersion
        document_id = $document.id; version = $saved.version; assets = @($taskAssets)
    }
    $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $taskRoot 'result.json') -Encoding utf8NoBOM
    Write-Host "Package smoke passed. Browser tests: $([bool]$BrowserTests). Artifacts: $taskRoot"
}
finally {
    foreach ($socket in $taskOwnedSockets) {
        try { $socket.Abort(); $socket.Dispose() }
        catch { Write-Warning "Could not finish cleanup for an owned WebSocket: $_" }
    }
    # Only retained process objects created by this script are stopped. Never
    # search by executable name, port or a caller-supplied PID; keep artifacts.
    foreach ($owned in $taskOwnedProcesses) {
        try { Stop-OfficeSmokeProcess $owned }
        catch { Write-Warning "Could not finish cleanup for owned process $($owned.Label): $_" }
    }
    if ($null -ne $taskClient) { $taskClient.Dispose() }
}
