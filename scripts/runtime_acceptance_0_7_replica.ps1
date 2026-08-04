[CmdletBinding()]
param(
    [string]$ConfirmRun = "",

    [Parameter(Mandatory = $true)]
    [string]$ProjectName,

    [Parameter(Mandatory = $true)]
    [string]$RunDirectory,

    [Parameter(Mandatory = $true)]
    [string]$DockerExecutable,

    [Parameter(Mandatory = $true)]
    [string]$DockerContext,

    [Parameter(Mandatory = $true)]
    [string]$OwnershipToken,

    [Parameter(Mandatory = $true)]
    [string]$DockerHelperPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$script:ReplicaClientInternalToken = "RUN-RYFRAME-V0-7-REPLICA-CLIENT"

$script:ReplicaAcceptanceMessages = ConvertFrom-Json @'
{
  "OptIn": "\u5fc5\u987b\u7531 v0.7 \u9a8c\u6536\u5165\u53e3\u4f20\u5165\u7cbe\u786e\u5b50\u9636\u6bb5\u786e\u8ba4\u4ee4\u724c",
  "PowerShellVersion": "\u526f\u672c\u8fd0\u884c\u9a8c\u6536\u9700\u8981 PowerShell 5.1 \u6216\u66f4\u9ad8\u7248\u672c",
  "ScriptLocation": "\u526f\u672c\u9a8c\u6536\u811a\u672c\u5fc5\u987b\u4f4d\u4e8e\u4ed3\u5e93 scripts \u76ee\u5f55",
  "HelperPath": "Docker \u652f\u6301\u811a\u672c\u8def\u5f84\u4e0e\u4ed3\u5e93\u56fa\u5b9a\u8def\u5f84\u4e0d\u4e00\u81f4\uff1a{0}",
  "RunDirectory": "\u526f\u672c\u9a8c\u6536\u8bc1\u636e\u76ee\u5f55\u5fc5\u987b\u4f4d\u4e8e v0.7 \u4e13\u7528 target \u6839\u76ee\u5f55\u5185\uff1a{0}",
  "EvidenceExists": "\u526f\u672c\u9a8c\u6536\u8bc1\u636e\u5df2\u5b58\u5728\uff0c\u62d2\u7edd\u8986\u76d6\uff1a{0}",
  "MissingFile": "\u526f\u672c\u9a8c\u6536\u7f3a\u5c11\u6587\u4ef6\uff1a{0}",
  "MissingCommand": "\u526f\u672c\u9a8c\u6536\u7f3a\u5c11\u547d\u4ee4\uff1a{0}",
  "CommandFailed": "{0}\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {1}",
  "PortUnavailable": "\u56de\u73af\u7aef\u53e3 {0} \u5df2\u88ab\u5360\u7528\u6216\u4e0d\u53ef\u7ed1\u5b9a",
  "ComposeValidate": "\u6821\u9a8c\u526f\u672c\u4e13\u7528 Compose \u914d\u7f6e",
  "ComposeStart": "\u542f\u52a8\u9694\u79bb\u4e3b\u5e93\u4e0e\u526f\u672c",
  "ResetPrimary": "\u91cd\u7f6e\u9694\u79bb\u4e3b\u5e93",
  "ResetReplica": "\u91cd\u7f6e\u9694\u79bb\u526f\u672c",
  "VerifyPrimary": "\u6821\u9a8c\u9694\u79bb\u4e3b\u5e93\u8fc1\u79fb\u8d26\u672c\u4e0e\u7ed3\u6784",
  "VerifyReplica": "\u6821\u9a8c\u9694\u79bb\u526f\u672c\u8fc1\u79fb\u8d26\u672c\u4e0e\u7ed3\u6784",
  "SeedReplica": "\u5199\u5165\u4ec5\u526f\u672c\u53ef\u89c1\u7684\u8def\u7531\u6807\u8bb0",
  "LedgerRemove": "\u6ce8\u5165\u4ec5\u8fc1\u79fb\u8d26\u672c\u6ede\u540e\u6545\u969c",
  "LedgerRestore": "\u4fee\u590d\u526f\u672c\u8fc1\u79fb\u8d26\u672c",
  "ProcessExited": "{0}\u8fdb\u7a0b\u5728\u9a8c\u6536\u5b8c\u6210\u524d\u9000\u51fa\uff0cPID \u4e3a {1}",
  "ProcessStopTimeout": "{0}\u8fdb\u7a0b PID {1}\u5728\u5f3a\u5236\u505c\u6b62\u540e\u4ecd\u672a\u9000\u51fa",
  "ContextMismatch": "\u5f53\u524d Docker context\u201c{0}\u201d\u4e0e\u4f20\u5165 context\u201c{1}\u201d\u4e0d\u4e00\u81f4",
  "ImageEvidence": "\u526f\u672c\u9a8c\u6536\u955c\u50cf\u8bc1\u636e\u5fc5\u987b\u7cbe\u786e\u5305\u542b mysql-primary \u4e0e mysql-replica\uff1a{0}",
  "ThresholdObserverLabel": "\u526f\u672c\u9608\u503c\u89c2\u5bdf\u5668",
  "Readiness": "{0}\u672a\u5728 {1} \u79d2\u5185\u5c31\u7eea",
  "SqlEvidence": "MySQL \u8bc1\u636e\u4e0d\u7b26\u5408\u9884\u671f\uff1a{0}",
  "ClientEvidence": "\u526f\u672c\u8def\u7531\u5ba2\u6237\u7aef\u8bc1\u636e\u4e0d\u7b26\u5408\u9884\u671f\uff1a{0}",
  "ClientFailed": "\u526f\u672c\u8def\u7531\u5ba2\u6237\u7aef\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {0}",
  "ReplicaRestore": "\u526f\u672c\u505c\u673a\u6545\u969c\u6062\u590d\u5931\u8d25\uff1a{0}",
  "LedgerCleanup": "\u526f\u672c\u8fc1\u79fb\u8d26\u672c\u6536\u5c3e\u4fee\u590d\u5931\u8d25\uff1a{0}",
  "ProcessCleanup": "{0}\u8fdb\u7a0b\u6e05\u7406\u5931\u8d25\uff1a{1}",
  "DockerCleanup": "\u526f\u672c\u9a8c\u6536 Docker \u8d44\u6e90\u6e05\u7406\u5931\u8d25\uff1a{0}",
  "TranscriptCleanup": "\u526f\u672c\u9a8c\u6536\u65e5\u5fd7\u6536\u5c3e\u5931\u8d25\uff1a{0}",
  "EnvironmentRestore": "\u526f\u672c\u9a8c\u6536\u8fdb\u7a0b\u73af\u5883\u6062\u590d\u5931\u8d25\uff1a{0}",
  "MetadataWrite": "\u526f\u672c\u9a8c\u6536\u8bc1\u636e\u5199\u5165\u5931\u8d25\uff1a{0}",
  "Success": "\u526f\u672c\u547d\u4e2d\u3001\u505c\u673a\u6458\u9664\u3001\u6062\u590d\u3001\u5f3a\u4e00\u81f4\u4e3b\u5e93\u8def\u7531\u548c\u8d26\u672c\u6ede\u540e\u62d2\u7edd\u9a8c\u6536\u901a\u8fc7\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{0}"
}
'@

if ($ConfirmRun -cne "RUN-RYFRAME-V0-7-STAGE") {
    throw $script:ReplicaAcceptanceMessages.OptIn
}
if ($PSVersionTable.PSVersion -lt [version]"5.1") {
    throw $script:ReplicaAcceptanceMessages.PowerShellVersion
}

function Test-ReplicaAcceptanceSamePath {
    param(
        [Parameter(Mandatory = $true)][string]$Actual,
        [Parameter(Mandatory = $true)][string]$Expected
    )

    $comparison = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        [System.StringComparison]::OrdinalIgnoreCase
    }
    else {
        [System.StringComparison]::Ordinal
    }
    return [string]::Equals(
        [System.IO.Path]::GetFullPath($Actual),
        [System.IO.Path]::GetFullPath($Expected),
        $comparison
    )
}

function Get-ReplicaAcceptanceCommand {
    param([Parameter(Mandatory = $true)][string]$Name)

    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command -or [string]::IsNullOrWhiteSpace($command.Source)) {
        throw ($script:ReplicaAcceptanceMessages.MissingCommand -f $Name)
    }
    return $command.Source
}

function Invoke-ReplicaAcceptanceCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Description
    )

    Write-Host ("`n==> {0}" -f $Description)
    & $Executable @Arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:ReplicaAcceptanceMessages.CommandFailed -f $Description, $exitCode)
    }
}

function Get-ReplicaAcceptanceFreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function Get-ReplicaAcceptancePorts {
    $ports = [ordered]@{}
    $used = New-Object System.Collections.Generic.HashSet[int]
    foreach ($name in @("primary", "replica", "api")) {
        do {
            $port = Get-ReplicaAcceptanceFreePort
        } while (-not $used.Add($port))
        $ports[$name] = $port
    }
    return $ports
}

function Assert-ReplicaAcceptancePortsAvailable {
    param([Parameter(Mandatory = $true)][System.Collections.IDictionary]$Ports)

    foreach ($port in $Ports.Values) {
        $listener = [System.Net.Sockets.TcpListener]::new(
            [System.Net.IPAddress]::Loopback,
            [int]$port
        )
        try {
            $listener.Start()
        }
        catch {
            throw ($script:ReplicaAcceptanceMessages.PortUnavailable -f $port)
        }
        finally {
            $listener.Stop()
        }
    }
}

function Set-ReplicaAcceptanceEnvironment {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Value
    )

    [System.Environment]::SetEnvironmentVariable($Name, $Value, "Process")
}

function Start-ReplicaAcceptanceProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][string]$StandardOutputLog,
        [Parameter(Mandatory = $true)][string]$StandardErrorLog
    )

    $startArguments = @{
        FilePath = $Executable
        WorkingDirectory = $WorkingDirectory
        RedirectStandardOutput = $StandardOutputLog
        RedirectStandardError = $StandardErrorLog
        PassThru = $true
    }
    if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        $startArguments["WindowStyle"] = "Hidden"
    }
    return Start-Process @startArguments
}

function Assert-ReplicaAcceptanceProcessIdentity {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $current = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $current) {
        throw ($script:ReplicaAcceptanceMessages.ProcessExited -f $Label, $Process.Id)
    }
    return $current
}

function Stop-ReplicaAcceptanceProcess {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $ownedProcess = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $ownedProcess) {
        return
    }
    Stop-Process -InputObject $ownedProcess -ErrorAction Stop
    if ($Process.WaitForExit(10000)) {
        return
    }
    $ownedProcess = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $ownedProcess) {
        [void]$Process.WaitForExit(10000)
        return
    }
    Stop-Process -InputObject $ownedProcess -Force -ErrorAction Stop
    if (-not $Process.WaitForExit(10000)) {
        throw ($script:ReplicaAcceptanceMessages.ProcessStopTimeout -f $Label, $Process.Id)
    }
}

function Wait-ReplicaAcceptanceReadiness {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 60
    )

    if ($Uri.Scheme -ne "http" -or $Uri.Host -ne "127.0.0.1") {
        throw ($script:ReplicaAcceptanceMessages.Readiness -f $Label, 0)
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        [void](Assert-ReplicaAcceptanceProcessIdentity `
            -Process $Process `
            -ExpectedExecutable $ExpectedExecutable `
            -Label $Label)
        try {
            $response = Invoke-WebRequest -Uri $Uri.AbsoluteUri -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 300) {
                return
            }
        }
        catch {
        }
        Start-Sleep -Milliseconds 250
    }
    throw ($script:ReplicaAcceptanceMessages.Readiness -f $Label, $TimeoutSeconds)
}

function Invoke-ReplicaAcceptanceMySqlLines {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string]$Sql,
        [Parameter(Mandatory = $true)][string]$ResolvedDockerExecutable
    )

    return @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $ResolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments @(
            "exec", "--env", "MYSQL_PWD=ryframe_test_password", $ContainerId,
            "mysql", "--batch", "--raw", "--skip-column-names", "-uroot",
            "ryframe_test", "--execute", $Sql
        ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

function Invoke-ReplicaAcceptanceMySqlScalar {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string]$Sql,
        [Parameter(Mandatory = $true)][string]$ResolvedDockerExecutable
    )

    $lines = @(Invoke-ReplicaAcceptanceMySqlLines `
        -ContainerId $ContainerId `
        -Sql $Sql `
        -ResolvedDockerExecutable $ResolvedDockerExecutable)
    if ($lines.Count -ne 1) {
        throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f ($lines -join " | "))
    }
    return $lines[0].Trim()
}

function Invoke-ReplicaAcceptanceClient {
    param(
        [Parameter(Mandatory = $true)][string]$NodeExecutable,
        [Parameter(Mandatory = $true)][string]$ClientScript,
        [Parameter(Mandatory = $true)][string]$ApiBase,
        [Parameter(Mandatory = $true)][string]$ExpectedState,
        [Parameter(Mandatory = $true)][string]$EvidencePath,
        [Parameter(Mandatory = $true)][string]$SentinelUser,
        [Parameter(Mandatory = $true)][string]$SentinelId,
        [Parameter(Mandatory = $true)][string]$ReplicaNickname,
        [int]$StabilitySeconds = 0
    )

    if (Test-Path -LiteralPath $EvidencePath) {
        throw ($script:ReplicaAcceptanceMessages.EvidenceExists -f $EvidencePath)
    }
    $evidenceRoot = Split-Path -Parent ([System.IO.Path]::GetFullPath($EvidencePath))
    & $NodeExecutable @(
        $ClientScript,
        "--api-base", $ApiBase,
        "--evidence", $EvidencePath,
        "--evidence-root", $evidenceRoot,
        "--expected-state", $ExpectedState,
        "--internal-token", $script:ReplicaClientInternalToken,
        "--sentinel-user", $SentinelUser,
        "--sentinel-id", $SentinelId,
        "--replica-nickname", $ReplicaNickname,
        "--stability-seconds", $StabilitySeconds.ToString()
    )
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:ReplicaAcceptanceMessages.ClientFailed -f $exitCode)
    }
    if (-not (Test-Path -LiteralPath $EvidencePath -PathType Leaf)) {
        throw ($script:ReplicaAcceptanceMessages.MissingFile -f $EvidencePath)
    }
    $evidence = Get-Content -LiteralPath $EvidencePath -Raw -Encoding utf8 | ConvertFrom-Json
    if ($evidence.status -ne "passed" -or $evidence.expected_state -cne $ExpectedState) {
        throw ($script:ReplicaAcceptanceMessages.ClientEvidence -f ($evidence | ConvertTo-Json -Depth 10 -Compress))
    }
    return $evidence
}

function Start-ReplicaAcceptanceThresholdObserver {
    param(
        [Parameter(Mandatory = $true)][string]$NodeExecutable,
        [Parameter(Mandatory = $true)][string]$ClientScript,
        [Parameter(Mandatory = $true)][string]$ApiBase,
        [Parameter(Mandatory = $true)][ValidateSet("failure-threshold", "recovery-threshold")]
        [string]$ExpectedState,
        [Parameter(Mandatory = $true)][string]$EvidencePath,
        [Parameter(Mandatory = $true)][string]$ReadyEvidencePath,
        [Parameter(Mandatory = $true)][string]$StandardOutputLog,
        [Parameter(Mandatory = $true)][string]$StandardErrorLog,
        [Parameter(Mandatory = $true)][string]$SentinelUser,
        [Parameter(Mandatory = $true)][string]$SentinelId,
        [Parameter(Mandatory = $true)][string]$ReplicaNickname
    )

    foreach ($path in @($EvidencePath, $ReadyEvidencePath, $StandardOutputLog, $StandardErrorLog)) {
        if (Test-Path -LiteralPath $path) {
            throw ($script:ReplicaAcceptanceMessages.EvidenceExists -f $path)
        }
    }
    $evidenceRoot = Split-Path -Parent ([System.IO.Path]::GetFullPath($EvidencePath))
    if (-not (Test-ReplicaAcceptanceSamePath `
        -Actual (Split-Path -Parent ([System.IO.Path]::GetFullPath($ReadyEvidencePath))) `
        -Expected $evidenceRoot)) {
        throw ($script:ReplicaAcceptanceMessages.ClientEvidence -f $ReadyEvidencePath)
    }
    $arguments = @(
        $ClientScript,
        "--api-base", $ApiBase,
        "--evidence", $EvidencePath,
        "--evidence-root", $evidenceRoot,
        "--expected-state", $ExpectedState,
        "--internal-token", $script:ReplicaClientInternalToken,
        "--ready-evidence", $ReadyEvidencePath,
        "--sentinel-user", $SentinelUser,
        "--sentinel-id", $SentinelId,
        "--replica-nickname", $ReplicaNickname,
        "--stability-seconds", "0"
    )
    $startArguments = @{
        FilePath = $NodeExecutable
        ArgumentList = (@($arguments | ForEach-Object {
            ConvertTo-RyFrameV07ProcessArgument -Value $_
        }) -join " ")
        WorkingDirectory = $evidenceRoot
        RedirectStandardOutput = $StandardOutputLog
        RedirectStandardError = $StandardErrorLog
        PassThru = $true
    }
    if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        $startArguments["WindowStyle"] = "Hidden"
    }
    return Start-Process @startArguments
}

function Wait-ReplicaAcceptanceThresholdObserverReady {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$NodeExecutable,
        [Parameter(Mandatory = $true)][string]$ReadyEvidencePath,
        [Parameter(Mandatory = $true)][string]$ExpectedState,
        [int]$TimeoutSeconds = 30
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        [void](Assert-ReplicaAcceptanceProcessIdentity `
            -Process $Process `
            -ExpectedExecutable $NodeExecutable `
            -Label $script:ReplicaAcceptanceMessages.ThresholdObserverLabel)
        if (Test-Path -LiteralPath $ReadyEvidencePath -PathType Leaf) {
            $ready = Get-Content -LiteralPath $ReadyEvidencePath -Raw -Encoding utf8 | ConvertFrom-Json
            if ($ready.status -ne "ready" -or $ready.expected_state -cne $ExpectedState) {
                throw ($script:ReplicaAcceptanceMessages.ClientEvidence -f (
                    $ready | ConvertTo-Json -Depth 10 -Compress
                ))
            }
            return $ready
        }
        Start-Sleep -Milliseconds 100
    }
    throw ($script:ReplicaAcceptanceMessages.Readiness -f (
        $script:ReplicaAcceptanceMessages.ThresholdObserverLabel
    ), $TimeoutSeconds)
}

function Complete-ReplicaAcceptanceThresholdObserver {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$EvidencePath,
        [Parameter(Mandatory = $true)][string]$ExpectedState,
        [int]$TimeoutSeconds = 130
    )

    if (-not $Process.WaitForExit($TimeoutSeconds * 1000)) {
        throw ($script:ReplicaAcceptanceMessages.Readiness -f (
            $script:ReplicaAcceptanceMessages.ThresholdObserverLabel
        ), $TimeoutSeconds)
    }
    $Process.WaitForExit()
    if ($Process.ExitCode -ne 0) {
        throw ($script:ReplicaAcceptanceMessages.ClientFailed -f $Process.ExitCode)
    }
    if (-not (Test-Path -LiteralPath $EvidencePath -PathType Leaf)) {
        throw ($script:ReplicaAcceptanceMessages.MissingFile -f $EvidencePath)
    }
    $evidence = Get-Content -LiteralPath $EvidencePath -Raw -Encoding utf8 | ConvertFrom-Json
    if ($evidence.status -ne "passed" -or $evidence.expected_state -cne $ExpectedState) {
        throw ($script:ReplicaAcceptanceMessages.ClientEvidence -f (
            $evidence | ConvertTo-Json -Depth 10 -Compress
        ))
    }
    return $evidence
}

$scriptFile = (Resolve-Path -LiteralPath $PSCommandPath).Path
$scriptsDirectory = Split-Path -Parent $scriptFile
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDirectory "..")).Path
$expectedScriptsDirectory = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "scripts"))
if (-not (Test-ReplicaAcceptanceSamePath -Actual $scriptsDirectory -Expected $expectedScriptsDirectory)) {
    throw $script:ReplicaAcceptanceMessages.ScriptLocation
}

$expectedHelperPath = Join-Path $scriptsDirectory "runtime_acceptance_0_7_support.ps1"
if (
    -not (Test-Path -LiteralPath $DockerHelperPath -PathType Leaf) `
    -or -not (Test-ReplicaAcceptanceSamePath -Actual $DockerHelperPath -Expected $expectedHelperPath)
) {
    throw ($script:ReplicaAcceptanceMessages.HelperPath -f $DockerHelperPath)
}
. $DockerHelperPath
Assert-RyFrameV07ProjectName -ProjectName $ProjectName
Assert-RyFrameV07OwnershipToken -OwnershipToken $OwnershipToken

$targetRoot = [System.IO.Path]::GetFullPath(
    (Join-Path (Join-Path $repositoryRoot "target") "runtime-acceptance-0-7")
)
$resolvedRunDirectory = [System.IO.Path]::GetFullPath($RunDirectory)
$targetPrefix = $targetRoot.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
) + [System.IO.Path]::DirectorySeparatorChar
$pathComparison = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) {
    [System.StringComparison]::OrdinalIgnoreCase
}
else {
    [System.StringComparison]::Ordinal
}
if (
    -not $resolvedRunDirectory.StartsWith($targetPrefix, $pathComparison) `
    -or -not (Test-Path -LiteralPath $resolvedRunDirectory -PathType Container)
) {
    throw ($script:ReplicaAcceptanceMessages.RunDirectory -f $resolvedRunDirectory)
}

$composeFile = Join-Path $scriptsDirectory "runtime_acceptance_0_7_replica.compose.yml"
$clientScript = Join-Path $scriptsDirectory "replica_runtime_acceptance_client.mjs"
$configDirectory = Join-Path $repositoryRoot "config"
$targetDirectory = Join-Path $repositoryRoot "target"
$binarySuffix = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) { ".exe" } else { "" }
$debugDirectory = Join-Path $targetDirectory "debug"
$apiBinary = Join-Path $debugDirectory "ryframe$binarySuffix"
$resetBinary = Join-Path $debugDirectory "ryframe-db-reset$binarySuffix"
$migrateBinary = Join-Path $debugDirectory "ryframe-migrate$binarySuffix"
foreach ($requiredPath in @(
    $composeFile,
    $clientScript,
    (Join-Path $repositoryRoot "Cargo.toml"),
    $apiBinary,
    $resetBinary,
    $migrateBinary
)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw ($script:ReplicaAcceptanceMessages.MissingFile -f $requiredPath)
    }
}
$apiBinary = (Resolve-Path -LiteralPath $apiBinary).Path
$resetBinary = (Resolve-Path -LiteralPath $resetBinary).Path
$migrateBinary = (Resolve-Path -LiteralPath $migrateBinary).Path

$metadataPath = Join-Path $resolvedRunDirectory "replica-run.json"
if (Test-Path -LiteralPath $metadataPath) {
    throw ($script:ReplicaAcceptanceMessages.EvidenceExists -f $metadataPath)
}
$transcriptPath = Join-Path $resolvedRunDirectory "replica-transcript.log"
$apiOutput = Join-Path $resolvedRunDirectory "api.stdout.log"
$apiError = Join-Path $resolvedRunDirectory "api.stderr.log"
$initialEvidencePath = Join-Path $resolvedRunDirectory "initial-healthy.json"
$failureThresholdEvidencePath = Join-Path $resolvedRunDirectory "replica-failure-threshold.json"
$failureThresholdReadyPath = Join-Path $resolvedRunDirectory "replica-failure-observer-ready.json"
$failureThresholdOutput = Join-Path $resolvedRunDirectory "replica-failure-observer.stdout.log"
$failureThresholdError = Join-Path $resolvedRunDirectory "replica-failure-observer.stderr.log"
$stoppedEvidencePath = Join-Path $resolvedRunDirectory "replica-stopped.json"
$recoveryThresholdEvidencePath = Join-Path $resolvedRunDirectory "replica-recovery-threshold.json"
$recoveryThresholdReadyPath = Join-Path $resolvedRunDirectory "replica-recovery-observer-ready.json"
$recoveryThresholdOutput = Join-Path $resolvedRunDirectory "replica-recovery-observer.stdout.log"
$recoveryThresholdError = Join-Path $resolvedRunDirectory "replica-recovery-observer.stderr.log"
$recoveredEvidencePath = Join-Path $resolvedRunDirectory "replica-recovered.json"
$ledgerLagEvidencePath = Join-Path $resolvedRunDirectory "ledger-lag.json"
$ledgerRepairedEvidencePath = Join-Path $resolvedRunDirectory "ledger-repaired.json"
$ports = Get-ReplicaAcceptancePorts
$sentinelUser = "ryframe_v07_replica_marker"
$sentinelId = "799999999999999900"
$replicaNickname = "ryframe-v07-replica-only"

$metadata = [ordered]@{
    schema_version = 1
    stage = "replica"
    status = "starting"
    started_at = [DateTime]::UtcNow.ToString("o")
    completed_at = $null
    docker_project = $ProjectName
    docker_context = $DockerContext
    images = @()
    run_directory = $resolvedRunDirectory
    ports = $ports
    replica_fault = [ordered]@{
        method = "docker_stop_start"
        evicted = $false
        restored = $false
    }
    probe_thresholds = [ordered]@{
        failure = $null
        recovery = $null
    }
    ledger_lag = [ordered]@{
        method = "delete_latest_migration_row"
        version = $null
        before_count = $null
        lagged_count = $null
        rejected_for_seconds = 12
        rejected = $false
        repaired = $false
        rejoined = $false
    }
    phases = [ordered]@{}
    error = $null
    cleanup_errors = @()
}
Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

$runError = $null
$runSucceeded = $false
$cleanupErrors = New-Object System.Collections.Generic.List[string]
$transcriptStarted = $false
$dockerOwned = $false
$replicaFault = $null
$ledgerRemoved = $false
$ledgerVersion = $null
$ledgerAppliedAt = $null
$replicaContainer = $null
$apiProcess = $null
$thresholdObserverProcess = $null
$nodeExecutable = $null
$resolvedDockerExecutable = $null
$originalLocation = (Get-Location).Path
$locationChanged = $false
$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot

try {
    Start-Transcript -LiteralPath $transcriptPath -Force | Out-Null
    $transcriptStarted = $true
    Set-Location -LiteralPath $repositoryRoot
    $locationChanged = $true
    Assert-ReplicaAcceptancePortsAvailable -Ports $ports

    $nodeExecutable = Get-ReplicaAcceptanceCommand -Name "node"
    $resolvedDockerExecutable = (Resolve-Path -LiteralPath $DockerExecutable).Path
    $contextInfo = Get-RyFrameV07LocalDockerContext -DockerExecutable $resolvedDockerExecutable
    if ($contextInfo.Name -cne $DockerContext) {
        throw ($script:ReplicaAcceptanceMessages.ContextMismatch -f $contextInfo.Name, $DockerContext)
    }
    $metadata["docker_server_version"] = Get-RyFrameV07DockerServerVersion `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["status"] = "running"
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $existingAppVariables = @(
        [System.Environment]::GetEnvironmentVariables("Process").Keys |
            Where-Object { $_ -is [string] -and $_.StartsWith("APP_", [System.StringComparison]::Ordinal) }
    )
    foreach ($name in $existingAppVariables) {
        [System.Environment]::SetEnvironmentVariable($name, $null, "Process")
    }
    foreach ($name in @("ADMIN_USER", "ADMIN_PASS", "TENANT_ID", "SNOWFLAKE_WORKER_ID")) {
        [System.Environment]::SetEnvironmentVariable($name, $null, "Process")
    }

    Set-ReplicaAcceptanceEnvironment -Name "RYFRAME_V07_PRIMARY_PORT" -Value $ports.primary.ToString()
    Set-ReplicaAcceptanceEnvironment -Name "RYFRAME_V07_REPLICA_PORT" -Value $ports.replica.ToString()
    Set-ReplicaAcceptanceEnvironment -Name "RYFRAME_V07_OWNERSHIP_TOKEN" -Value $OwnershipToken
    Set-ReplicaAcceptanceEnvironment -Name "NO_PROXY" -Value "127.0.0.1,localhost"

    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments @("compose", "--project-name", $ProjectName, "--file", $composeFile, "config", "--quiet") `
        -Description $script:ReplicaAcceptanceMessages.ComposeValidate
    Assert-RyFrameV07ProjectEmpty `
        -ProjectName $ProjectName `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $dockerOwned = $true
    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments @("compose", "--project-name", $ProjectName, "--file", $composeFile, "up", "-d", "--wait") `
        -Description $script:ReplicaAcceptanceMessages.ComposeStart
    $imageEvidence = @(Get-RyFrameV07ProjectImageEvidence `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext)
    $imageServices = @($imageEvidence | ForEach-Object { [string]$_.service } | Sort-Object)
    if ($imageEvidence.Count -ne 2 -or ($imageServices -join ",") -cne "mysql-primary,mysql-replica") {
        throw ($script:ReplicaAcceptanceMessages.ImageEvidence -f ($imageServices -join ","))
    }
    $metadata["images"] = $imageEvidence
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    Set-ReplicaAcceptanceEnvironment -Name "APP_CONFIG_DIR" -Value $configDirectory
    Set-ReplicaAcceptanceEnvironment -Name "APP_ENV" -Value "test"
    Set-ReplicaAcceptanceEnvironment -Name "APP_APP_HOST" -Value "127.0.0.1"
    Set-ReplicaAcceptanceEnvironment -Name "APP_APP_PORT" -Value $ports.api.ToString()
    Set-ReplicaAcceptanceEnvironment -Name "APP_API_DOCS_ENABLED" -Value "false"
    Set-ReplicaAcceptanceEnvironment -Name "APP_MONITOR_METRICS_BEARER_TOKEN" -Value ""
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_HOST" -Value "127.0.0.1"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_NAME" -Value "ryframe_test"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_USERNAME" -Value "root"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_PASSWORD" -Value "ryframe_test_password"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_TLS_MODE" -Value "disabled"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_MIGRATION_MODE" -Value "verify"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_REPLICAS" -Value "[]"
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_SOURCES" -Value "[]"
    Set-ReplicaAcceptanceEnvironment -Name "APP_REDIS_MODE" -Value "disabled"
    Set-ReplicaAcceptanceEnvironment -Name "APP_RATE_LIMIT_ENABLED" -Value "false"
    Set-ReplicaAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_BACKEND" -Value "local"
    Set-ReplicaAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_LOCAL_BASE_DIR" `
        -Value (Join-Path $resolvedRunDirectory "local-storage")
    Set-ReplicaAcceptanceEnvironment -Name "APP_JOBS_MODE" -Value "external"
    Set-ReplicaAcceptanceEnvironment -Name "APP_AUTH_JWT_SECRET" `
        -Value "ryframe-v07-replica-acceptance-jwt-secret-2026"
    Set-ReplicaAcceptanceEnvironment -Name "APP_MESSAGING_ENABLED" -Value "false"
    Set-ReplicaAcceptanceEnvironment -Name "APP_TELEMETRY_ENABLED" -Value "false"
    Set-ReplicaAcceptanceEnvironment -Name "APP_LOGGER_OUTPUT" -Value "stdout"
    Set-ReplicaAcceptanceEnvironment -Name "APP_LOGGER_FORMAT" -Value "text"
    Set-ReplicaAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "903"

    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_PORT" -Value $ports.primary.ToString()
    Invoke-ReplicaAcceptanceCommand `
        -Executable $resetBinary `
        -Arguments @("--database", "ryframe_test", "--confirm-reset", "RESET-RYFRAME-DATABASE") `
        -Description $script:ReplicaAcceptanceMessages.ResetPrimary
    Invoke-ReplicaAcceptanceCommand `
        -Executable $migrateBinary `
        -Arguments @("verify") `
        -Description $script:ReplicaAcceptanceMessages.VerifyPrimary

    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_PORT" -Value $ports.replica.ToString()
    Invoke-ReplicaAcceptanceCommand `
        -Executable $resetBinary `
        -Arguments @("--database", "ryframe_test", "--confirm-reset", "RESET-RYFRAME-DATABASE") `
        -Description $script:ReplicaAcceptanceMessages.ResetReplica
    Invoke-ReplicaAcceptanceCommand `
        -Executable $migrateBinary `
        -Arguments @("verify") `
        -Description $script:ReplicaAcceptanceMessages.VerifyReplica

    $replicaContainer = Resolve-RyFrameV07ServiceContainer `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -Service "mysql-replica" `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    Write-Host ("`n==> {0}" -f $script:ReplicaAcceptanceMessages.SeedReplica)
    $sentinelInsertCount = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql ("INSERT INTO sys_login_info " +
            "(id, tenant_id, user_name, ipaddr, login_location, browser, os, status, msg, login_time) " +
            "VALUES ($sentinelId, 'system', '$sentinelUser', '127.0.0.7', " +
            "'replica-only', 'acceptance', 'acceptance', '1', 'replica-only', " +
            "'2099-12-31 23:59:59'); SELECT ROW_COUNT();")
    $nicknameUpdateCount = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql ("UPDATE sys_user SET nickname = '$replicaNickname' " +
            "WHERE tenant_id = 'system' AND username = 'admin'; SELECT ROW_COUNT();")
    if ($sentinelInsertCount -cne "1" -or $nicknameUpdateCount -cne "1") {
        throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f (
            "sentinel=$sentinelInsertCount,nickname=$nicknameUpdateCount"
        ))
    }

    $replicaConfigObject = @(
        [ordered]@{
            name = "replica-a"
            host = "127.0.0.1"
            port = [int]$ports.replica
            database = "ryframe_test"
            username = "root"
            password = "ryframe_test_password"
            max_connections = 5
            min_connections = 1
            acquire_timeout_secs = 5
            idle_timeout_secs = 60
            max_lifetime_secs = 300
            connect_timeout_secs = 3
            tls_mode = "disabled"
        }
    )
    $replicaConfig = ConvertTo-Json -InputObject $replicaConfigObject -Depth 5 -Compress
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_PORT" -Value $ports.primary.ToString()
    Set-ReplicaAcceptanceEnvironment -Name "APP_DATABASE_REPLICAS" -Value $replicaConfig

    $apiProcess = Start-ReplicaAcceptanceProcess `
        -Executable $apiBinary `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $apiOutput `
        -StandardErrorLog $apiError
    $apiBase = "http://127.0.0.1:$($ports.api)"
    Wait-ReplicaAcceptanceReadiness `
        -Uri "$apiBase/readyz" `
        -Process $apiProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API"

    $metadata["phases"]["initial_healthy"] = Invoke-ReplicaAcceptanceClient `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "healthy" `
        -EvidencePath $initialEvidencePath `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $thresholdObserverProcess = Start-ReplicaAcceptanceThresholdObserver `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "failure-threshold" `
        -EvidencePath $failureThresholdEvidencePath `
        -ReadyEvidencePath $failureThresholdReadyPath `
        -StandardOutputLog $failureThresholdOutput `
        -StandardErrorLog $failureThresholdError `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname
    [void](Wait-ReplicaAcceptanceThresholdObserverReady `
        -Process $thresholdObserverProcess `
        -NodeExecutable $nodeExecutable `
        -ReadyEvidencePath $failureThresholdReadyPath `
        -ExpectedState "failure-threshold")
    $replicaFault = Stop-RyFrameV07DockerService `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -Service "mysql-replica" `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["probe_thresholds"]["failure"] = Complete-ReplicaAcceptanceThresholdObserver `
        -Process $thresholdObserverProcess `
        -EvidencePath $failureThresholdEvidencePath `
        -ExpectedState "failure-threshold"
    $thresholdObserverProcess = $null
    $metadata["phases"]["replica_stopped"] = Invoke-ReplicaAcceptanceClient `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "fallback" `
        -EvidencePath $stoppedEvidencePath `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname
    $metadata["replica_fault"]["evicted"] = $true
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $thresholdObserverProcess = Start-ReplicaAcceptanceThresholdObserver `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "recovery-threshold" `
        -EvidencePath $recoveryThresholdEvidencePath `
        -ReadyEvidencePath $recoveryThresholdReadyPath `
        -StandardOutputLog $recoveryThresholdOutput `
        -StandardErrorLog $recoveryThresholdError `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname
    [void](Wait-ReplicaAcceptanceThresholdObserverReady `
        -Process $thresholdObserverProcess `
        -NodeExecutable $nodeExecutable `
        -ReadyEvidencePath $recoveryThresholdReadyPath `
        -ExpectedState "recovery-threshold")
    Restore-RyFrameV07DockerFault `
        -Fault $replicaFault `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $replicaFault = $null
    $metadata["probe_thresholds"]["recovery"] = Complete-ReplicaAcceptanceThresholdObserver `
        -Process $thresholdObserverProcess `
        -EvidencePath $recoveryThresholdEvidencePath `
        -ExpectedState "recovery-threshold"
    $thresholdObserverProcess = $null
    $metadata["phases"]["replica_recovered"] = Invoke-ReplicaAcceptanceClient `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "healthy" `
        -EvidencePath $recoveredEvidencePath `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname
    $metadata["replica_fault"]["restored"] = $true
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $ledgerVersion = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql "SELECT version FROM seaql_migrations ORDER BY applied_at DESC, version DESC LIMIT 1;"
    $ledgerAppliedAt = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql "SELECT applied_at FROM seaql_migrations ORDER BY applied_at DESC, version DESC LIMIT 1;"
    $ledgerCount = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql "SELECT COUNT(*) FROM seaql_migrations;"
    if (
        $ledgerVersion -notmatch "^[a-zA-Z0-9_]+$" `
        -or $ledgerAppliedAt -notmatch "^[0-9]+$" `
        -or $ledgerCount -notmatch "^[1-9][0-9]*$"
    ) {
        throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f (
            "version=$ledgerVersion,applied_at=$ledgerAppliedAt,count=$ledgerCount"
        ))
    }

    Write-Host ("`n==> {0}" -f $script:ReplicaAcceptanceMessages.LedgerRemove)
    $deletedLedgerRows = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql "DELETE FROM seaql_migrations WHERE version = '$ledgerVersion'; SELECT ROW_COUNT();"
    if ($deletedLedgerRows -cne "1") {
        throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f "deleted=$deletedLedgerRows")
    }
    $ledgerRemoved = $true
    $laggedLedgerCount = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql "SELECT COUNT(*) FROM seaql_migrations;"
    if ([int]$laggedLedgerCount -ne ([int]$ledgerCount - 1)) {
        throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f (
            "before=$ledgerCount,lagged=$laggedLedgerCount"
        ))
    }
    $metadata["ledger_lag"]["version"] = $ledgerVersion
    $metadata["ledger_lag"]["before_count"] = [int]$ledgerCount
    $metadata["ledger_lag"]["lagged_count"] = [int]$laggedLedgerCount
    $metadata["phases"]["ledger_lag"] = Invoke-ReplicaAcceptanceClient `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "fallback" `
        -EvidencePath $ledgerLagEvidencePath `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname `
        -StabilitySeconds 12
    $metadata["ledger_lag"]["rejected"] = $true
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    Write-Host ("`n==> {0}" -f $script:ReplicaAcceptanceMessages.LedgerRestore)
    $restoredLedgerRows = Invoke-ReplicaAcceptanceMySqlScalar `
        -ContainerId $replicaContainer `
        -ResolvedDockerExecutable $resolvedDockerExecutable `
        -Sql ("INSERT INTO seaql_migrations (version, applied_at) " +
            "VALUES ('$ledgerVersion', $ledgerAppliedAt); SELECT ROW_COUNT();")
    if ($restoredLedgerRows -cne "1") {
        throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f "restored=$restoredLedgerRows")
    }
    $ledgerRemoved = $false
    $metadata["ledger_lag"]["repaired"] = $true
    $metadata["phases"]["ledger_repaired"] = Invoke-ReplicaAcceptanceClient `
        -NodeExecutable $nodeExecutable `
        -ClientScript $clientScript `
        -ApiBase $apiBase `
        -ExpectedState "healthy" `
        -EvidencePath $ledgerRepairedEvidencePath `
        -SentinelUser $sentinelUser `
        -SentinelId $sentinelId `
        -ReplicaNickname $replicaNickname
    $metadata["ledger_lag"]["rejoined"] = $true
    $runSucceeded = $true
}
catch {
    $runError = $_
    $metadata["error"] = $_.Exception.Message
}
finally {
    if ($null -ne $thresholdObserverProcess) {
        try {
            Stop-ReplicaAcceptanceProcess `
                -Process $thresholdObserverProcess `
                -ExpectedExecutable $nodeExecutable `
                -Label $script:ReplicaAcceptanceMessages.ThresholdObserverLabel
            $thresholdObserverProcess = $null
        }
        catch {
            $cleanupErrors.Add((
                $script:ReplicaAcceptanceMessages.ProcessCleanup -f (
                    $script:ReplicaAcceptanceMessages.ThresholdObserverLabel
                ), $_.Exception.Message
            ))
        }
    }
    if ($null -ne $replicaFault) {
        try {
            Restore-RyFrameV07DockerFault `
                -Fault $replicaFault `
                -OwnershipToken $OwnershipToken `
                -DockerExecutable $resolvedDockerExecutable `
                -Context $DockerContext
            $replicaFault = $null
        }
        catch {
            $cleanupErrors.Add(($script:ReplicaAcceptanceMessages.ReplicaRestore -f $_.Exception.Message))
        }
    }

    if (
        $ledgerRemoved `
        -and $null -ne $replicaContainer `
        -and $null -ne $resolvedDockerExecutable `
        -and $ledgerVersion -match "^[a-zA-Z0-9_]+$" `
        -and $ledgerAppliedAt -match "^[0-9]+$"
    ) {
        try {
            $cleanupRestore = Invoke-ReplicaAcceptanceMySqlScalar `
                -ContainerId $replicaContainer `
                -ResolvedDockerExecutable $resolvedDockerExecutable `
                -Sql ("INSERT INTO seaql_migrations (version, applied_at) " +
                    "VALUES ('$ledgerVersion', $ledgerAppliedAt); SELECT ROW_COUNT();")
            if ($cleanupRestore -cne "1") {
                throw ($script:ReplicaAcceptanceMessages.SqlEvidence -f "cleanup_restored=$cleanupRestore")
            }
            $ledgerRemoved = $false
        }
        catch {
            $cleanupErrors.Add(($script:ReplicaAcceptanceMessages.LedgerCleanup -f $_.Exception.Message))
        }
    }

    if ($null -ne $apiProcess -and $null -ne $apiBinary) {
        try {
            Stop-ReplicaAcceptanceProcess `
                -Process $apiProcess `
                -ExpectedExecutable $apiBinary `
                -Label "API"
        }
        catch {
            $cleanupErrors.Add(($script:ReplicaAcceptanceMessages.ProcessCleanup -f "API", $_.Exception.Message))
        }
    }

    if ($dockerOwned -and $null -ne $resolvedDockerExecutable) {
        try {
            Remove-RyFrameV07DockerProjectResources `
                -ProjectName $ProjectName `
                -OwnershipToken $OwnershipToken `
                -DockerExecutable $resolvedDockerExecutable `
                -Context $DockerContext
        }
        catch {
            $cleanupErrors.Add(($script:ReplicaAcceptanceMessages.DockerCleanup -f $_.Exception.Message))
        }
    }

    if ($locationChanged) {
        try {
            Set-Location -LiteralPath $originalLocation
        }
        catch {
            $cleanupErrors.Add($_.Exception.Message)
        }
    }
    if ($transcriptStarted) {
        try {
            Stop-Transcript | Out-Null
        }
        catch {
            $cleanupErrors.Add(($script:ReplicaAcceptanceMessages.TranscriptCleanup -f $_.Exception.Message))
        }
    }

    try {
        Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot
    }
    catch {
        $cleanupErrors.Add(($script:ReplicaAcceptanceMessages.EnvironmentRestore -f $_.Exception.Message))
    }

    $metadata["completed_at"] = [DateTime]::UtcNow.ToString("o")
    $metadata["cleanup_errors"] = @($cleanupErrors)
    if ($null -ne $runError) {
        $metadata["status"] = "failed"
    }
    elseif ($cleanupErrors.Count -gt 0) {
        $metadata["status"] = "cleanup_failed"
    }
    elseif ($runSucceeded) {
        $metadata["status"] = "passed"
    }
    else {
        $metadata["status"] = "failed"
    }
    try {
        Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
    }
    catch {
        $metadataError = $script:ReplicaAcceptanceMessages.MetadataWrite -f $_.Exception.Message
        if ($null -eq $runError) {
            $runError = [System.InvalidOperationException]::new($metadataError)
        }
        else {
            $cleanupErrors.Add($metadataError)
        }
    }
}

if ($null -ne $runError) {
    throw $runError
}
if ($cleanupErrors.Count -gt 0) {
    throw ($cleanupErrors -join "; ")
}
Write-Host ("`n" + ($script:ReplicaAcceptanceMessages.Success -f $resolvedRunDirectory))
