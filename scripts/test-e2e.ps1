param([string]$Project = '', [string]$Grep = '', [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/workspace.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
$server = $null
$savedTestEnvironment = @{}
foreach ($name in @('API_URL', 'E2E_EXTERNAL_SERVER', 'SYNTRA_E2E_ARTIFACTS')) {
    $savedTestEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    $metadata = Get-SyntraWorkspaceMetadata
    $extension = if ($IsWindows) { '.exe' } else { '' }
    if (-not $SkipBuild) {
        $messages = & cargo build --locked -p syntra-server --message-format=json
        if ($LASTEXITCODE -ne 0) { throw 'server compilation failed' }
        $artifact = $messages | ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'syntra-server' -and $_.executable } |
            Select-Object -Last 1
        if (-not $artifact) { throw 'Cargo did not report the server executable' }
        $binary = $artifact.executable
        & "$PSScriptRoot/build-frontend.ps1"
    }
    else {
        $binary = Join-Path $metadata.target_directory "debug/syntra-server$extension"
    }
    if (-not (Test-Path -LiteralPath $binary)) { throw "Missing server: $binary" }
    $web = Join-Path $metadata.target_directory 'syntra-web'
    if (-not (Test-Path -LiteralPath (Join-Path $web 'assets/syntra_frontend_bg.wasm'))) { throw 'Build browser assets before E2E' }
    $instance = Join-Path $metadata.target_directory ("e2e-instance-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $instance | Out-Null
    $instanceBinary = Join-Path $instance "syntra-server$extension"
    Copy-Item -LiteralPath $binary -Destination $instanceBinary
    Copy-Item -LiteralPath config,migrations -Destination $instance -Recurse
    @'
[rate_limit]
enabled = false
[ai]
enabled = false
[storage]
base_path = "data/files"
'@ | Set-Content -LiteralPath (Join-Path $instance 'config/development.toml') -Encoding utf8
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()
    $options = @{
        FilePath = $instanceBinary
        WorkingDirectory = $instance
        PassThru = $true
        RedirectStandardOutput = (Join-Path $instance 'server.stdout.log')
        RedirectStandardError = (Join-Path $instance 'server.stderr.log')
        Environment = @{
            SYNTRA_ENV = 'development'
            SYNTRA_SERVER_HOST = '127.0.0.1'
            SYNTRA_SERVER_PORT = [string]$port
            SYNTRA_STATIC_DIR = $web
            RUST_LOG = 'warn'
        }
    }
    # Keep compiler and temp settings inherited, but isolate application
    # configuration so a caller's production DB/provider cannot enter tests.
    foreach ($item in Get-ChildItem Env: | Where-Object { $_.Name -like 'SYNTRA_*' -or $_.Name -like ('CON' + 'TRIX_*') }) {
        if (-not $options.Environment.ContainsKey($item.Name)) { $options.Environment[$item.Name] = $null }
    }
    $options.Environment.SYNTRA_DATABASE_URL = 'sqlite://data/syntra.db'
    if ($IsWindows) { $options.WindowStyle = 'Hidden' }
    $server = Start-Process @options
    $env:API_URL = "http://127.0.0.1:$port"
    $env:E2E_EXTERNAL_SERVER = '1'
    $env:SYNTRA_E2E_ARTIFACTS = Join-Path $instance 'playwright'
    $ready = $false
    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        if ($server.HasExited) { throw "E2E server exited. Inspect $instance/server.stderr.log" }
        try {
            $response = Invoke-WebRequest "$env:API_URL/api/v1/ready" -TimeoutSec 1 -SkipHttpErrorCheck
            if ([int]$response.StatusCode -eq 200) { $ready = $true; break }
        }
        catch {}
        Start-Sleep -Milliseconds 500
    }
    if (-not $ready) { throw "E2E server was not ready. Inspect $instance" }
    $playwrightCli = Join-Path $metadata.workspace_root 'e2e/node_modules/@playwright/test/cli.js'
    if (-not (Test-Path -LiteralPath $playwrightCli)) { throw 'Install E2E dependencies with npm ci in e2e/ first' }
    $arguments = @($playwrightCli, 'test')
    if ($Project) { $arguments += @('--project', $Project) }
    if ($Grep) { $arguments += @('--grep', $Grep) }
    Write-Host "Isolated E2E instance: $instance"
    Write-Host "API: $env:API_URL"
    Push-Location e2e
    try {
        & node @arguments
        $testExitCode = $LASTEXITCODE
    }
    finally { Pop-Location }
}
finally {
    # Only stop the process object created by this invocation; never search for
    # or kill another user's server on a fixed port.
    if ($server -and -not $server.HasExited) { Stop-Process -InputObject $server -Force }
    foreach ($name in $savedTestEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedTestEnvironment[$name], 'Process')
    }
    Pop-Location
}
exit $testExitCode
