[CmdletBinding()]
param(
    [ValidateRange(1024, 65535)]
    [int]$MySqlPort = 23306,

    [ValidateRange(1024, 65535)]
    [int]$RedisPort = 26379,

    [ValidateRange(1024, 65535)]
    [int]$RustFsPort = 29000,

    [ValidateRange(1024, 65535)]
    [int]$ApiPort = 28080,

    [ValidateRange(1024, 65535)]
    [int]$WorkerHealthPort = 29091
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:RuntimeMessages = ConvertFrom-Json @'
{
  "CommandFailed": "{0} \u5931\u8d25\uff0c\u9000\u51fa\u7801\uff1a{1}",
  "MissingCommand": "\u672a\u627e\u5230\u5fc5\u9700\u547d\u4ee4\uff1a{0}",
  "DockerContextRead": "\u65e0\u6cd5\u8bfb\u53d6 Docker context\uff1a{0}",
  "DockerContextEmpty": "Docker context \u4e3a\u7a7a\uff0c\u62d2\u7edd\u7ee7\u7eed",
  "DockerContextInspect": "\u65e0\u6cd5\u68c0\u67e5 Docker context\u201c{0}\u201d\uff1a{1}",
  "DockerRemote": "Docker context\u201c{0}\u201d\u6307\u5411\u975e\u672c\u673a endpoint\u201c{1}\u201d\uff0c\u62d2\u7edd\u8fd0\u884c\u9a8c\u6536",
  "DockerDaemonUnavailable": "Docker context\u201c{0}\u201d\u7684\u672c\u673a daemon \u4e0d\u53ef\u7528\uff0c\u8bf7\u5148\u542f\u52a8 Docker Desktop \u5e76\u7b49\u5f85 Linux Engine \u5c31\u7eea\uff1a{1}",
  "PortDuplicate": "\u7aef\u53e3 {0} \u540c\u65f6\u5206\u914d\u7ed9\u201c{1}\u201d\u548c\u201c{2}\u201d\uff0c\u62d2\u7edd\u7ee7\u7eed",
  "PortUnavailable": "\u56de\u73af\u7aef\u53e3 {0}\uff08{1}\uff09\u5df2\u88ab\u5360\u7528\u6216\u4e0d\u53ef\u7ed1\u5b9a",
  "ProcessExited": "{0} \u8fdb\u7a0b\u5df2\u63d0\u524d\u9000\u51fa\uff0cPID\uff1a{1}",
  "ProcessPathUnreadable": "\u65e0\u6cd5\u8bfb\u53d6 {0} \u8fdb\u7a0b PID {1} \u7684\u53ef\u6267\u884c\u6587\u4ef6\u8def\u5f84",
  "ProcessPathMismatch": "{0} \u8fdb\u7a0b PID {1} \u7684\u8def\u5f84\u4e3a\u201c{2}\u201d\uff0c\u9884\u671f\u4e3a\u201c{3}\u201d",
  "ReadinessUri": "{0} \u5c31\u7eea\u5730\u5740\u5fc5\u987b\u4f7f\u7528 http://127.0.0.1\uff0c\u5b9e\u9645\u4e3a\u201c{1}\u201d",
  "ExitedBeforeReady": "{0} \u8fdb\u7a0b\u5728\u5c31\u7eea\u524d\u9000\u51fa\uff1b\u65e5\u5fd7\uff1a\u201c{1}\u201d\u3001\u201c{2}\u201d",
  "NotReady": "{0} \u672a\u5728 {1} \u79d2\u5185\u5c31\u7eea\uff1b\u65e5\u5fd7\uff1a\u201c{2}\u201d\u3001\u201c{3}\u201d",
  "ScriptLocation": "\u811a\u672c\u5fc5\u987b\u4f4d\u4e8e\u4ed3\u5e93 scripts \u76ee\u5f55",
  "MissingFile": "\u7f3a\u5c11\u8fd0\u884c\u65f6\u9a8c\u6536\u6240\u9700\u6587\u4ef6\uff1a{0}",
  "ProjectName": "\u751f\u6210\u7684 Docker project name \u4e0d\u7b26\u5408\u9694\u79bb\u89c4\u5219\uff1a{0}",
  "LogEscaped": "\u8fd0\u884c\u65e5\u5fd7\u76ee\u5f55\u8d8a\u51fa\u9650\u5b9a\u8303\u56f4\uff1a{0}",
  "LogExists": "\u8fd0\u884c\u65e5\u5fd7\u76ee\u5f55\u5df2\u5b58\u5728\uff0c\u62d2\u7edd\u590d\u7528\uff1a{0}",
  "ValidateCompose": "\u6821\u9a8c\u6d4b\u8bd5 Compose \u914d\u7f6e",
  "StartDependencies": "\u542f\u52a8\u9694\u79bb\u7684 MySQL\u3001Redis \u4e0e RustFS",
  "BuildBinaries": "\u4e00\u6b21\u6784\u5efa\u5168\u90e8\u8fd0\u884c\u65f6\u4e8c\u8fdb\u5236",
  "MissingBinary": "\u6784\u5efa\u6210\u529f\u540e\u4ecd\u7f3a\u5c11\u8fd0\u884c\u65f6\u4e8c\u8fdb\u5236\uff1a{0}",
  "RunTest": "\u8fd0\u884c\u65f6\u4e13\u9879\u6d4b\u8bd5 {0}/{1}",
  "ResetDatabase": "\u91cd\u7f6e\u9694\u79bb\u6d4b\u8bd5\u6570\u636e\u5e93\u5e76\u5e94\u7528\u8fc1\u79fb",
  "MigrationStatus": "\u68c0\u67e5\u8fc1\u79fb\u8d26\u672c\u72b6\u6001",
  "MigrationVerify": "\u9a8c\u8bc1\u6700\u7ec8\u6570\u636e\u5e93\u7ed3\u6784",
  "RunSmoke": "\u6267\u884c API\u3001\u72ec\u7acb Worker \u4e0e RustFS \u8de8\u8fdb\u7a0b\u5192\u70df",
  "Success": "\u8fd0\u884c\u65f6\u9a8c\u6536\u5168\u90e8\u901a\u8fc7\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{0}",
  "ApiCleanup": "API \u6e05\u7406\u5931\u8d25\uff1a{0}",
  "WorkerCleanup": "Worker \u6e05\u7406\u5931\u8d25\uff1a{0}",
  "DockerCleanup": "\u6e05\u7406\u540c\u4e00\u9694\u79bb Docker project \u7684\u5bb9\u5668\u3001\u7f51\u7edc\u548c\u6570\u636e\u5377",
  "ComposeCleanup": "Docker Compose \u6e05\u7406\u5931\u8d25\uff1a{0}",
  "EnvironmentRestore": "\u73af\u5883\u53d8\u91cf\u6062\u590d\u5931\u8d25\uff1a{0}",
  "DirectoryRestore": "\u5de5\u4f5c\u76ee\u5f55\u6062\u590d\u5931\u8d25\uff1a{0}",
  "TranscriptCleanup": "\u9a8c\u6536\u65e5\u5fd7\u6536\u5c3e\u5931\u8d25\uff1a{0}",
  "MetadataWrite": "\u9a8c\u6536\u8bc1\u636e\u5199\u5165\u5931\u8d25\uff1a{0}",
  "MetadataArtifactCleanup": "\u9a8c\u6536\u8bc1\u636e\u5df2\u63d0\u4ea4\uff0c\u4f46\u4e34\u65f6\u6587\u4ef6\u6e05\u7406\u5931\u8d25\uff1a{0}",
  "CleanupFailed": "\u8fd0\u884c\u65f6\u9a8c\u6536\u901a\u8fc7\uff0c\u4f46\u6e05\u7406\u672a\u5b8c\u6210\uff1a{0}\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{1}"
}
'@

$script:RuntimeEnvironmentBackup = @{}
$script:RuntimeIsWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)

function Set-RuntimeEnvironmentVariable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value
    )

    Save-RuntimeEnvironmentVariable -Name $Name
    [System.Environment]::SetEnvironmentVariable($Name, $Value, "Process")
}

function Save-RuntimeEnvironmentVariable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if ($script:RuntimeEnvironmentBackup.ContainsKey($Name)) {
        return
    }
    $currentValue = [System.Environment]::GetEnvironmentVariable($Name, "Process")
    $script:RuntimeEnvironmentBackup[$Name] = [pscustomobject]@{
        WasPresent = $null -ne $currentValue
        Value = $currentValue
    }
}

function Remove-RuntimeEnvironmentVariable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    Save-RuntimeEnvironmentVariable -Name $Name
    [System.Environment]::SetEnvironmentVariable($Name, $null, "Process")
}

function Restore-RuntimeEnvironment {
    foreach ($name in $script:RuntimeEnvironmentBackup.Keys) {
        $backup = $script:RuntimeEnvironmentBackup[$name]
        $value = if ($backup.WasPresent) { $backup.Value } else { $null }
        [System.Environment]::SetEnvironmentVariable($name, $value, "Process")
    }
}

function Invoke-CheckedNativeCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Executable,

        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    Write-Host "`n==> $Description"
    & $Executable @Arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:RuntimeMessages.CommandFailed -f $Description, $exitCode)
    }
}

function Write-RuntimeMetadataAtomically {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.IDictionary]$Metadata,
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [scriptblock]$ArtifactDeleter = {
            param([string]$ArtifactPath)
            [System.IO.File]::Delete($ArtifactPath)
        }
    )

    $destinationPath = [System.IO.Path]::GetFullPath($Path)
    $artifactSuffix = "{0}.{1}" -f $PID, [guid]::NewGuid().ToString("N")
    $temporaryPath = "{0}.{1}.tmp" -f $destinationPath, $artifactSuffix
    $backupPath = "{0}.{1}.bak" -f $destinationPath, $artifactSuffix
    $encoding = [System.Text.UTF8Encoding]::new($false)
    $primaryError = $null
    $cleanupError = $null
    $committed = $false
    try {
        $json = ($Metadata | ConvertTo-Json -Depth 4) + "`n"
        $bytes = $encoding.GetBytes($json)
        $stream = [System.IO.FileStream]::new(
            $temporaryPath,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        try {
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        }
        finally {
            $stream.Dispose()
        }
        if ([System.IO.File]::Exists($destinationPath)) {
            [System.IO.File]::Replace($temporaryPath, $destinationPath, $backupPath, $true)
        }
        else {
            [System.IO.File]::Move($temporaryPath, $destinationPath)
        }
        $committed = $true
    }
    catch {
        $primaryError = $_
    }
    finally {
        foreach ($cleanupPath in @($temporaryPath, $backupPath)) {
            try {
                if ([System.IO.File]::Exists($cleanupPath)) {
                    & $ArtifactDeleter $cleanupPath
                }
            }
            catch {
                if ($null -eq $cleanupError) {
                    $cleanupError = $_
                }
            }
        }
    }

    if ($null -ne $primaryError) {
        throw $primaryError
    }
    if ($null -ne $cleanupError -and -not $committed) {
        throw $cleanupError
    }
    if ($null -ne $cleanupError) {
        Write-Warning ($script:RuntimeMessages.MetadataArtifactCleanup -f $cleanupError.Exception.Message)
    }
}

function Resolve-RuntimeTerminalStatus {
    param(
        [bool]$RunSucceeded,
        [bool]$HasRunError,
        [int]$CleanupErrorCount
    )

    if ($HasRunError) {
        return "failed"
    }
    if ($CleanupErrorCount -gt 0) {
        return "cleanup_failed"
    }
    if ($RunSucceeded) {
        return "passed"
    }
    return "failed"
}

function Get-RequiredExecutable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $command = Get-Command $Name -CommandType Application -ErrorAction Stop | Select-Object -First 1
    if ($null -eq $command -or [string]::IsNullOrWhiteSpace($command.Source)) {
        throw ($script:RuntimeMessages.MissingCommand -f $Name)
    }
    return $command.Source
}

function Assert-LocalDockerContext {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable
    )

    $contextOutput = & $DockerExecutable context show 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw ($script:RuntimeMessages.DockerContextRead -f ($contextOutput -join [Environment]::NewLine))
    }
    $context = ($contextOutput | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($context)) {
        throw $script:RuntimeMessages.DockerContextEmpty
    }

    $endpointOutput = & $DockerExecutable context inspect --format "{{ .Endpoints.docker.Host }}" $context 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw ($script:RuntimeMessages.DockerContextInspect -f $context, ($endpointOutput -join [Environment]::NewLine))
    }
    $endpoint = ($endpointOutput | Out-String).Trim()
    if ($endpoint -notmatch "^(npipe|unix)://") {
        throw ($script:RuntimeMessages.DockerRemote -f $context, $endpoint)
    }

    return [pscustomobject]@{
        Name = $context
        Endpoint = $endpoint
    }
}

function Assert-DockerDaemonAvailable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    $ErrorActionPreference = "Continue"
    $serverOutput = & $DockerExecutable `
        --context $Context `
        info `
        --format "{{ .ServerVersion }}" 2>&1
    $serverExitCode = $LASTEXITCODE
    $serverVersion = ($serverOutput | Out-String).Trim()
    if ($serverExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($serverVersion)) {
        throw ($script:RuntimeMessages.DockerDaemonUnavailable -f (
            $Context,
            ($serverOutput -join [Environment]::NewLine)
        ))
    }

    return $serverVersion
}

function Assert-LoopbackPortsAvailable {
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.IDictionary]$Ports
    )

    $seen = @{}
    foreach ($entry in $Ports.GetEnumerator()) {
        $port = [int]$entry.Value
        if ($seen.ContainsKey($port)) {
            throw ($script:RuntimeMessages.PortDuplicate -f $port, $seen[$port], $entry.Key)
        }
        $seen[$port] = $entry.Key
    }

    foreach ($entry in $Ports.GetEnumerator()) {
        $listener = [System.Net.Sockets.TcpListener]::new(
            [System.Net.IPAddress]::Loopback,
            [int]$entry.Value
        )
        try {
            $listener.Start()
        }
        catch {
            throw ($script:RuntimeMessages.PortUnavailable -f $entry.Value, $entry.Key)
        }
        finally {
            $listener.Stop()
        }
    }
}

function Test-SameExecutablePath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Actual,

        [Parameter(Mandatory = $true)]
        [string]$Expected
    )

    $actualFullPath = [System.IO.Path]::GetFullPath($Actual)
    $expectedFullPath = [System.IO.Path]::GetFullPath($Expected)
    $comparison = if ($script:RuntimeIsWindows) {
        [System.StringComparison]::OrdinalIgnoreCase
    }
    else {
        [System.StringComparison]::Ordinal
    }
    return [string]::Equals($actualFullPath, $expectedFullPath, $comparison)
}

function Assert-RecordedProcessIdentity {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$RecordedProcess,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    $current = Get-Process -Id $RecordedProcess.Id -ErrorAction SilentlyContinue
    if ($null -eq $current) {
        throw ($script:RuntimeMessages.ProcessExited -f $Label, $RecordedProcess.Id)
    }
    $actualPath = $current.Path
    if ([string]::IsNullOrWhiteSpace($actualPath)) {
        throw ($script:RuntimeMessages.ProcessPathUnreadable -f $Label, $RecordedProcess.Id)
    }
    if (-not (Test-SameExecutablePath -Actual $actualPath -Expected $ExpectedExecutable)) {
        throw ($script:RuntimeMessages.ProcessPathMismatch -f $Label, $RecordedProcess.Id, $actualPath, $ExpectedExecutable)
    }
}

function Stop-RecordedProcess {
    param(
        [AllowNull()]
        [System.Diagnostics.Process]$RecordedProcess,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if ($null -eq $RecordedProcess) {
        return
    }

    $current = Get-Process -Id $RecordedProcess.Id -ErrorAction SilentlyContinue
    if ($null -eq $current) {
        return
    }
    Assert-RecordedProcessIdentity `
        -RecordedProcess $RecordedProcess `
        -ExpectedExecutable $ExpectedExecutable `
        -Label $Label

    Stop-Process -Id $RecordedProcess.Id -ErrorAction Stop
    try {
        Wait-Process -Id $RecordedProcess.Id -Timeout 10 -ErrorAction Stop
        return
    }
    catch {
        $current = Get-Process -Id $RecordedProcess.Id -ErrorAction SilentlyContinue
        if ($null -eq $current) {
            return
        }
    }

    Assert-RecordedProcessIdentity `
        -RecordedProcess $RecordedProcess `
        -ExpectedExecutable $ExpectedExecutable `
        -Label $Label
    Stop-Process -Id $RecordedProcess.Id -Force -ErrorAction Stop
    Wait-Process -Id $RecordedProcess.Id -Timeout 10 -ErrorAction Stop
}

function Wait-LocalReadiness {
    param(
        [Parameter(Mandatory = $true)]
        [uri]$Uri,

        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$RecordedProcess,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Label,

        [Parameter(Mandatory = $true)]
        [string]$StandardOutputLog,

        [Parameter(Mandatory = $true)]
        [string]$StandardErrorLog,

        [int]$TimeoutSeconds = 120
    )

    if ($Uri.Scheme -ne "http" -or $Uri.Host -ne "127.0.0.1") {
        throw ($script:RuntimeMessages.ReadinessUri -f $Label, $Uri)
    }

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $current = Get-Process -Id $RecordedProcess.Id -ErrorAction SilentlyContinue
        if ($null -eq $current) {
            throw ($script:RuntimeMessages.ExitedBeforeReady -f $Label, $StandardOutputLog, $StandardErrorLog)
        }
        Assert-RecordedProcessIdentity `
            -RecordedProcess $RecordedProcess `
            -ExpectedExecutable $ExpectedExecutable `
            -Label $Label

        try {
            $response = Invoke-WebRequest -Uri $Uri.AbsoluteUri -TimeoutSec 2 -UseBasicParsing
            if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 300) {
                return
            }
        }
        catch {
        }
        Start-Sleep -Milliseconds 500
    }

    throw ($script:RuntimeMessages.NotReady -f $Label, $TimeoutSeconds, $StandardOutputLog, $StandardErrorLog)
}

function Start-RuntimeProcess {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Executable,

        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory,

        [Parameter(Mandatory = $true)]
        [string]$StandardOutputLog,

        [Parameter(Mandatory = $true)]
        [string]$StandardErrorLog
    )

    $startArguments = @{
        FilePath = $Executable
        WorkingDirectory = $WorkingDirectory
        RedirectStandardOutput = $StandardOutputLog
        RedirectStandardError = $StandardErrorLog
        PassThru = $true
    }
    if ($script:RuntimeIsWindows) {
        $startArguments.WindowStyle = "Hidden"
    }
    return Start-Process @startArguments
}

$scriptFile = (Resolve-Path -LiteralPath $PSCommandPath).Path
$scriptsDirectory = Split-Path -Parent $scriptFile
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDirectory "..")).Path
$expectedScriptsDirectory = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "scripts"))
if (-not (Test-SameExecutablePath -Actual $scriptsDirectory -Expected $expectedScriptsDirectory)) {
    throw $script:RuntimeMessages.ScriptLocation
}

$composeFile = Join-Path $repositoryRoot "docker-compose.test.yml"
$configDirectory = Join-Path $repositoryRoot "config"
$testConfigFile = Join-Path $configDirectory "app.test.toml"
$deployDirectory = Join-Path $repositoryRoot "deploy"
$deployTestsDirectory = Join-Path $deployDirectory "tests"
$smokeTestFile = Join-Path $deployTestsDirectory "smoke-test.js"
$cratesDirectory = Join-Path $repositoryRoot "crates"
$serviceCrateDirectory = Join-Path $cratesDirectory "ryframe-service"
$serviceTestsDirectory = Join-Path $serviceCrateDirectory "tests"
$exportAcceptanceTestFile = Join-Path $serviceTestsDirectory "export_runtime_acceptance_test.rs"
foreach ($requiredPath in @(
    $composeFile,
    $testConfigFile,
    $smokeTestFile,
    $exportAcceptanceTestFile,
    (Join-Path $repositoryRoot "Cargo.toml")
)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw ($script:RuntimeMessages.MissingFile -f $requiredPath)
    }
}

$runId = "{0}-{1}-{2}" -f (
    Get-Date -Format "yyyyMMddHHmmss"
), $PID, ([guid]::NewGuid().ToString("N").Substring(0, 12))
$projectName = "ryframe-runtime-$runId".ToLowerInvariant()
if ($projectName -notmatch "^ryframe-runtime-[a-z0-9-]+$") {
    throw ($script:RuntimeMessages.ProjectName -f $projectName)
}

$targetDirectory = Join-Path $repositoryRoot "target"
$runtimeLogRoot = [System.IO.Path]::GetFullPath((Join-Path $targetDirectory "runtime-acceptance"))
$runDirectory = [System.IO.Path]::GetFullPath((Join-Path $runtimeLogRoot $runId))
$runtimePrefix = $runtimeLogRoot.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
) + [System.IO.Path]::DirectorySeparatorChar
if (-not $runDirectory.StartsWith($runtimePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw ($script:RuntimeMessages.LogEscaped -f $runDirectory)
}
if (Test-Path -LiteralPath $runDirectory) {
    throw ($script:RuntimeMessages.LogExists -f $runDirectory)
}
New-Item -ItemType Directory -Path $runDirectory | Out-Null

$transcriptPath = Join-Path $runDirectory "acceptance-transcript.log"
$workerStandardOutput = Join-Path $runDirectory "worker.stdout.log"
$workerStandardError = Join-Path $runDirectory "worker.stderr.log"
$apiStandardOutput = Join-Path $runDirectory "api.stdout.log"
$apiStandardError = Join-Path $runDirectory "api.stderr.log"
$metadataPath = Join-Path $runDirectory "run.json"

$binarySuffix = if ($script:RuntimeIsWindows) { ".exe" } else { "" }
$debugDirectory = Join-Path $targetDirectory "debug"
$resetBinary = Join-Path $debugDirectory "ryframe-db-reset$binarySuffix"
$migrateBinary = Join-Path $debugDirectory "ryframe-migrate$binarySuffix"
$workerBinary = Join-Path $debugDirectory "ryframe-worker$binarySuffix"
$apiBinary = Join-Path $debugDirectory "ryframe$binarySuffix"

$apiProcess = $null
$workerProcess = $null
$composeOwned = $false
$transcriptStarted = $false
$dockerExecutable = $null
$dockerContext = $null
$originalLocation = (Get-Location).Path
$runError = $null
$runSucceeded = $false
$cleanupErrors = [System.Collections.Generic.List[string]]::new()
$ports = [ordered]@{
    MySQL = $MySqlPort
    Redis = $RedisPort
    RustFS = $RustFsPort
    API = $ApiPort
    WorkerHealth = $WorkerHealthPort
}
$rustFsEndpoint = "http://127.0.0.1:$RustFsPort"
$rustFsAccessKey = "ryframe-test-access"
$rustFsSecretKey = "ryframe-test-secret-2026"
$rustFsRegion = "us-east-1"
$loopbackNoProxy = "127.0.0.1,localhost"
$metadata = [ordered]@{
    run_id = $runId
    docker_project = $projectName
    docker_context = $null
    docker_endpoint = $null
    docker_server_version = $null
    repository = $repositoryRoot
    log_directory = $runDirectory
    ports = $ports
    status = "starting"
    started_at = [DateTimeOffset]::Now.ToString("O")
    completed_at = $null
    error = $null
    cleanup_errors = @()
}
Write-RuntimeMetadataAtomically -Metadata $metadata -Path $metadataPath

try {
    Start-Transcript -Path $transcriptPath | Out-Null
    $transcriptStarted = $true
    Set-Location -LiteralPath $repositoryRoot

    Assert-LoopbackPortsAvailable -Ports $ports

    $cargoExecutable = Get-RequiredExecutable -Name "cargo"
    $dockerExecutable = Get-RequiredExecutable -Name "docker"
    $nodeExecutable = Get-RequiredExecutable -Name "node"
    $dockerContextInfo = Assert-LocalDockerContext -DockerExecutable $dockerExecutable
    $dockerContext = $dockerContextInfo.Name

    $existingAppVariables = @(
        [System.Environment]::GetEnvironmentVariables("Process").Keys |
            Where-Object { $_ -is [string] -and $_.StartsWith("APP_", [System.StringComparison]::Ordinal) }
    )
    foreach ($name in $existingAppVariables) {
        Remove-RuntimeEnvironmentVariable -Name $name
    }
    foreach ($name in @("SNOWFLAKE_WORKER_ID", "ADMIN_USER", "ADMIN_PASS", "TENANT_ID")) {
        Remove-RuntimeEnvironmentVariable -Name $name
    }

    Set-RuntimeEnvironmentVariable -Name "CARGO_TARGET_DIR" -Value $targetDirectory
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_MYSQL_PORT" -Value $MySqlPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_MYSQL_ADMIN_URL" `
        -Value "mysql://root:ryframe_test_password@127.0.0.1:$MySqlPort/mysql"
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_REDIS_PORT" -Value $RedisPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_RUSTFS_PORT" -Value $RustFsPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_RUSTFS_ENDPOINT" -Value $rustFsEndpoint
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_RUSTFS_ACCESS_KEY" -Value $rustFsAccessKey
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_RUSTFS_SECRET_KEY" -Value $rustFsSecretKey
    Set-RuntimeEnvironmentVariable -Name "RYFRAME_TEST_RUSTFS_REGION" -Value $rustFsRegion
    Set-RuntimeEnvironmentVariable -Name "NO_PROXY" -Value $loopbackNoProxy

    $metadata["docker_context"] = $dockerContextInfo.Name
    $metadata["docker_endpoint"] = $dockerContextInfo.Endpoint
    $dockerServerVersion = Assert-DockerDaemonAvailable `
        -DockerExecutable $dockerExecutable `
        -Context $dockerContext
    $metadata["docker_server_version"] = $dockerServerVersion
    $metadata["status"] = "running"
    Write-RuntimeMetadataAtomically -Metadata $metadata -Path $metadataPath

    $composeArguments = @(
        "--context", $dockerContext,
        "compose",
        "--project-name", $projectName,
        "--file", $composeFile
    )
    Invoke-CheckedNativeCommand `
        -Executable $dockerExecutable `
        -Arguments ($composeArguments + @("config", "--quiet")) `
        -Description $script:RuntimeMessages.ValidateCompose

    $composeOwned = $true
    Invoke-CheckedNativeCommand `
        -Executable $dockerExecutable `
        -Arguments ($composeArguments + @("up", "-d", "--wait")) `
        -Description $script:RuntimeMessages.StartDependencies

    Invoke-CheckedNativeCommand `
        -Executable $cargoExecutable `
        -Arguments @("build", "--locked", "-p", "ryframe", "--bins", "--features", "file-maintenance") `
        -Description $script:RuntimeMessages.BuildBinaries

    foreach ($binary in @($resetBinary, $migrateBinary, $workerBinary, $apiBinary)) {
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
            throw ($script:RuntimeMessages.MissingBinary -f $binary)
        }
    }
    $resetBinary = (Resolve-Path -LiteralPath $resetBinary).Path
    $migrateBinary = (Resolve-Path -LiteralPath $migrateBinary).Path
    $workerBinary = (Resolve-Path -LiteralPath $workerBinary).Path
    $apiBinary = (Resolve-Path -LiteralPath $apiBinary).Path

    $testCommands = @(
        @("test", "--locked", "--workspace", "--no-fail-fast", "--", "--test-threads=1"),
        @("test", "--locked", "-p", "ryframe-core", "--test", "refresh_session_redis_test", "--", "--ignored", "--test-threads=1"),
        @(
            "test", "--locked", "-p", "ryframe-service", "--lib",
            "system::online_user_service::redis_backend::tests::stale_touch_cannot_resurrect_or_overwrite_online_user_index",
            "--", "--exact", "--ignored", "--test-threads=1"
        ),
        @("test", "--locked", "-p", "ryframe-api", "--test", "integration_test", "--", "--ignored", "--test-threads=1")
    )
    $rustFsTestCommands = @(
        @(
            "test", "--locked", "-p", "ryframe-storage", "--test", "object_storage_test",
            "test_s3_integration_put_get_delete", "--", "--exact", "--ignored", "--test-threads=1"
        ),
        @(
            "test", "--locked", "-p", "ryframe-service", "--test", "export_runtime_acceptance_test",
            "export_runtime_acceptance_covers_scale_takeover_storage_recovery_and_cleanup",
            "--", "--exact", "--ignored", "--test-threads=1"
        )
    )
    $totalTestCount = $testCommands.Count + $rustFsTestCommands.Count
    $testIndex = 0
    foreach ($testArguments in $testCommands) {
        $testIndex += 1
        Invoke-CheckedNativeCommand `
            -Executable $cargoExecutable `
            -Arguments $testArguments `
            -Description ($script:RuntimeMessages.RunTest -f $testIndex, $totalTestCount)
    }

    Set-RuntimeEnvironmentVariable -Name "APP_OBJECT_STORAGE_ENDPOINT" -Value $rustFsEndpoint
    Set-RuntimeEnvironmentVariable -Name "APP_OBJECT_STORAGE_ACCESS_KEY" -Value $rustFsAccessKey
    Set-RuntimeEnvironmentVariable -Name "APP_OBJECT_STORAGE_SECRET_KEY" -Value $rustFsSecretKey
    Set-RuntimeEnvironmentVariable -Name "APP_OBJECT_STORAGE_USE_SSL" -Value "false"
    Set-RuntimeEnvironmentVariable -Name "APP_OBJECT_STORAGE_REGION" -Value $rustFsRegion
    foreach ($testArguments in $rustFsTestCommands) {
        $testIndex += 1
        Invoke-CheckedNativeCommand `
            -Executable $cargoExecutable `
            -Arguments $testArguments `
            -Description ($script:RuntimeMessages.RunTest -f $testIndex, $totalTestCount)
    }

    Set-RuntimeEnvironmentVariable -Name "APP_CONFIG_DIR" -Value $configDirectory
    Set-RuntimeEnvironmentVariable -Name "APP_ENV" -Value "test"
    Set-RuntimeEnvironmentVariable -Name "APP_APP_HOST" -Value "127.0.0.1"
    Set-RuntimeEnvironmentVariable -Name "APP_APP_PORT" -Value $ApiPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "APP_API_DOCS_ENABLED" -Value "true"
    Set-RuntimeEnvironmentVariable -Name "APP_MONITOR_METRICS_BEARER_TOKEN" -Value ""
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_HOST" -Value "127.0.0.1"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_PORT" -Value $MySqlPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_NAME" -Value "ryframe_test"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_USERNAME" -Value "root"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_PASSWORD" -Value "ryframe_test_password"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_TLS_MODE" -Value "disabled"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_MIGRATION_MODE" -Value "verify"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_REPLICAS" -Value "[]"
    Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_SOURCES" -Value "[]"
    Set-RuntimeEnvironmentVariable -Name "APP_REDIS_MODE" -Value "required"
    Set-RuntimeEnvironmentVariable -Name "APP_REDIS_HOST" -Value "127.0.0.1"
    Set-RuntimeEnvironmentVariable -Name "APP_REDIS_PORT" -Value $RedisPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "APP_REDIS_PASSWORD" -Value ""
    Set-RuntimeEnvironmentVariable -Name "APP_REDIS_DATABASE" -Value "0"
    Set-RuntimeEnvironmentVariable -Name "APP_REDIS_TLS" -Value "false"
    Set-RuntimeEnvironmentVariable -Name "APP_OBJECT_STORAGE_BACKEND" -Value "rustfs"
    Set-RuntimeEnvironmentVariable -Name "APP_JOBS_MODE" -Value "external"
    Set-RuntimeEnvironmentVariable -Name "APP_JOBS_POLL_INTERVAL_MS" -Value "100"
    Set-RuntimeEnvironmentVariable -Name "APP_JOBS_WORKER_ID" -Value "runtime-$runId"
    Set-RuntimeEnvironmentVariable -Name "APP_JOBS_HEALTH_HOST" -Value "127.0.0.1"
    Set-RuntimeEnvironmentVariable -Name "APP_JOBS_HEALTH_PORT" -Value $WorkerHealthPort.ToString()
    Set-RuntimeEnvironmentVariable -Name "APP_AUTH_JWT_SECRET" `
        -Value "ryframe-runtime-acceptance-jwt-secret-2026"
    Set-RuntimeEnvironmentVariable -Name "APP_MESSAGING_ENABLED" -Value "true"
    Set-RuntimeEnvironmentVariable -Name "APP_TELEMETRY_ENABLED" -Value "false"
    Set-RuntimeEnvironmentVariable -Name "APP_LOGGER_OUTPUT" -Value "stdout"
    Set-RuntimeEnvironmentVariable -Name "APP_LOGGER_FORMAT" -Value "text"
    Set-RuntimeEnvironmentVariable -Name "ADMIN_USER" -Value "admin"
    Set-RuntimeEnvironmentVariable -Name "ADMIN_PASS" -Value "123456"
    Set-RuntimeEnvironmentVariable -Name "TENANT_ID" -Value "system"

    Set-RuntimeEnvironmentVariable -Name "SNOWFLAKE_WORKER_ID" -Value "0"
    Invoke-CheckedNativeCommand `
        -Executable $resetBinary `
        -Arguments @("--database", "ryframe_test", "--confirm-reset", "RESET-RYFRAME-DATABASE") `
        -Description $script:RuntimeMessages.ResetDatabase
    Invoke-CheckedNativeCommand `
        -Executable $migrateBinary `
        -Arguments @("status") `
        -Description $script:RuntimeMessages.MigrationStatus
    Invoke-CheckedNativeCommand `
        -Executable $migrateBinary `
        -Arguments @("verify") `
        -Description $script:RuntimeMessages.MigrationVerify

    Assert-LoopbackPortsAvailable -Ports ([ordered]@{
        API = $ApiPort
        WorkerHealth = $WorkerHealthPort
    })

    $snowflakeBase = Get-Random -Minimum 1 -Maximum 1022
    $workerSnowflakeId = $snowflakeBase
    $apiSnowflakeId = $snowflakeBase + 1

    Set-RuntimeEnvironmentVariable -Name "SNOWFLAKE_WORKER_ID" -Value $workerSnowflakeId.ToString()
    $workerProcess = Start-RuntimeProcess `
        -Executable $workerBinary `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $workerStandardOutput `
        -StandardErrorLog $workerStandardError
    Assert-RecordedProcessIdentity `
        -RecordedProcess $workerProcess `
        -ExpectedExecutable $workerBinary `
        -Label "Worker"
    Wait-LocalReadiness `
        -Uri "http://127.0.0.1:$WorkerHealthPort/readyz" `
        -RecordedProcess $workerProcess `
        -ExpectedExecutable $workerBinary `
        -Label "Worker" `
        -StandardOutputLog $workerStandardOutput `
        -StandardErrorLog $workerStandardError

    Set-RuntimeEnvironmentVariable -Name "SNOWFLAKE_WORKER_ID" -Value $apiSnowflakeId.ToString()
    $apiProcess = Start-RuntimeProcess `
        -Executable $apiBinary `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $apiStandardOutput `
        -StandardErrorLog $apiStandardError
    Assert-RecordedProcessIdentity `
        -RecordedProcess $apiProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API"
    Wait-LocalReadiness `
        -Uri "http://127.0.0.1:$ApiPort/readyz" `
        -RecordedProcess $apiProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API" `
        -StandardOutputLog $apiStandardOutput `
        -StandardErrorLog $apiStandardError

    Invoke-CheckedNativeCommand `
        -Executable $nodeExecutable `
        -Arguments @($smokeTestFile, "http://127.0.0.1:$ApiPort") `
        -Description $script:RuntimeMessages.RunSmoke

    $runSucceeded = $true
}
catch {
    $runError = $_
}
finally {
    try {
        Stop-RecordedProcess `
            -RecordedProcess $apiProcess `
            -ExpectedExecutable $apiBinary `
            -Label "API"
    }
    catch {
        $cleanupErrors.Add(($script:RuntimeMessages.ApiCleanup -f $_.Exception.Message))
    }

    try {
        Stop-RecordedProcess `
            -RecordedProcess $workerProcess `
            -ExpectedExecutable $workerBinary `
            -Label "Worker"
    }
    catch {
        $cleanupErrors.Add(($script:RuntimeMessages.WorkerCleanup -f $_.Exception.Message))
    }

    if ($composeOwned -and $null -ne $dockerExecutable -and $null -ne $dockerContext) {
        try {
            Invoke-CheckedNativeCommand `
                -Executable $dockerExecutable `
                -Arguments @(
                    "--context", $dockerContext,
                    "compose",
                    "--project-name", $projectName,
                    "--file", $composeFile,
                    "down", "--volumes", "--remove-orphans", "--timeout", "15"
                ) `
                -Description $script:RuntimeMessages.DockerCleanup
        }
        catch {
            $cleanupErrors.Add(($script:RuntimeMessages.ComposeCleanup -f $_.Exception.Message))
        }
    }

    try {
        Restore-RuntimeEnvironment
    }
    catch {
        $cleanupErrors.Add(($script:RuntimeMessages.EnvironmentRestore -f $_.Exception.Message))
    }

    try {
        Set-Location -LiteralPath $originalLocation
    }
    catch {
        $cleanupErrors.Add(($script:RuntimeMessages.DirectoryRestore -f $_.Exception.Message))
    }

    if ($transcriptStarted) {
        try {
            Stop-Transcript | Out-Null
        }
        catch {
            $cleanupErrors.Add(($script:RuntimeMessages.TranscriptCleanup -f $_.Exception.Message))
        }
    }

    $metadata["completed_at"] = [DateTimeOffset]::Now.ToString("O")
    $metadata["cleanup_errors"] = @($cleanupErrors)
    $metadata["error"] = if ($null -ne $runError) {
        $runError.Exception.Message
    }
    elseif ($cleanupErrors.Count -gt 0) {
        $cleanupErrors -join "; "
    }
    else {
        $null
    }
    $metadata["status"] = Resolve-RuntimeTerminalStatus `
        -RunSucceeded $runSucceeded `
        -HasRunError ($null -ne $runError) `
        -CleanupErrorCount $cleanupErrors.Count
    try {
        Write-RuntimeMetadataAtomically -Metadata $metadata -Path $metadataPath
    }
    catch {
        $metadataWriteError = $script:RuntimeMessages.MetadataWrite -f $_.Exception.Message
        Write-Warning $metadataWriteError
        if ($null -eq $runError) {
            $cleanupErrors.Add($metadataWriteError)
        }
    }
}

if ($null -ne $runError) {
    foreach ($cleanupError in $cleanupErrors) {
        Write-Warning $cleanupError
    }
    throw $runError
}
if ($cleanupErrors.Count -gt 0) {
    throw ($script:RuntimeMessages.CleanupFailed -f ($cleanupErrors -join "; "), $runDirectory)
}
Write-Host ("`n" + ($script:RuntimeMessages.Success -f $runDirectory)) -ForegroundColor Green
