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

$script:MessageAcceptanceMessages = ConvertFrom-Json @'
{
  "OptIn": "\u5fc5\u987b\u7531 v0.7 \u9a8c\u6536\u5165\u53e3\u4f20\u5165\u7cbe\u786e\u5b50\u9636\u6bb5\u786e\u8ba4\u4ee4\u724c",
  "PowerShellVersion": "\u6d88\u606f\u4e2d\u5fc3\u8fd0\u884c\u9a8c\u6536\u9700\u8981 PowerShell 5.1 \u6216\u66f4\u9ad8\u7248\u672c",
  "ScriptLocation": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u811a\u672c\u5fc5\u987b\u4f4d\u4e8e\u4ed3\u5e93 scripts \u76ee\u5f55",
  "HelperPath": "Docker \u652f\u6301\u811a\u672c\u8def\u5f84\u4e0e\u4ed3\u5e93\u56fa\u5b9a\u8def\u5f84\u4e0d\u4e00\u81f4\uff1a{0}",
  "RunDirectory": "\u6d88\u606f\u4e2d\u5fc3\u8bc1\u636e\u76ee\u5f55\u5fc5\u987b\u4f4d\u4e8e v0.7 \u4e13\u7528 target \u6839\u76ee\u5f55\u5185\uff1a{0}",
  "EvidenceExists": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u8bc1\u636e\u5df2\u5b58\u5728\uff0c\u62d2\u7edd\u8986\u76d6\uff1a{0}",
  "MissingFile": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u7f3a\u5c11\u6587\u4ef6\uff1a{0}",
  "MissingCommand": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u7f3a\u5c11\u547d\u4ee4\uff1a{0}",
  "CommandFailed": "{0}\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {1}",
  "PortUnavailable": "\u56de\u73af\u7aef\u53e3 {0}\u5df2\u88ab\u5360\u7528\u6216\u4e0d\u53ef\u7ed1\u5b9a",
  "Build": "\u6784\u5efa\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u6240\u9700\u4e8c\u8fdb\u5236",
  "ImageEvidence": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u955c\u50cf\u8bc1\u636e\u5fc5\u987b\u7cbe\u786e\u5305\u542b mysql\u3001redis \u548c rustfs\uff1a{0}",
  "MissingBinary": "\u6784\u5efa\u5b8c\u6210\u540e\u4ecd\u7f3a\u5c11\u4e8c\u8fdb\u5236\uff1a{0}",
  "ComposeValidate": "\u6821\u9a8c\u6d88\u606f\u4e2d\u5fc3\u9694\u79bb Compose \u914d\u7f6e",
  "ComposeStart": "\u542f\u52a8\u9694\u79bb MySQL\u3001Redis \u4e0e RustFS",
  "ContextMismatch": "Docker context \u4e0d\u4e00\u81f4\uff1a\u5f53\u524d\u4e3a\u201c{0}\u201d\uff0c\u4f20\u5165\u4e3a\u201c{1}\u201d",
  "ResetDatabase": "\u91cd\u7f6e\u6d88\u606f\u4e2d\u5fc3\u9694\u79bb\u6570\u636e\u5e93",
  "MigrationStatus": "\u68c0\u67e5\u6d88\u606f\u4e2d\u5fc3\u8fc1\u79fb\u8d26\u672c",
  "MigrationVerify": "\u9a8c\u8bc1\u6d88\u606f\u4e2d\u5fc3\u6570\u636e\u5e93\u7ed3\u6784",
  "ProcessExited": "{0}\u8fdb\u7a0b\u5728\u9a8c\u6536\u5b8c\u6210\u524d\u9000\u51fa\uff0cPID \u4e3a {1}",
  "ProcessIdentity": "{0}\u8fdb\u7a0b PID {1}\u7684\u53ef\u6267\u884c\u6587\u4ef6\u4e0e\u8bb0\u5f55\u4e0d\u4e00\u81f4",
  "ProcessStopTimeout": "{0}\u8fdb\u7a0b\u672a\u5728\u505c\u6b62\u671f\u9650\u5185\u9000\u51fa\uff0cPID \u4e3a {1}",
  "Readiness": "{0}\u672a\u5728 {1} \u79d2\u5185\u5c31\u7eea",
  "WaitFile": "\u7b49\u5f85\u8bc1\u636e\u6587\u4ef6\u201c{0}\u201d\u8d85\u65f6",
  "WaitMetric": "{0}\u672a\u5728 {1} \u79d2\u5185\u8fbe\u5230\u9884\u671f\u6307\u6807\u72b6\u6001 {2}",
  "ClientResult": "\u6d88\u606f\u9a8c\u6536\u5ba2\u6237\u7aef\u8bc1\u636e\u4e0d\u7b26\u5408\u9884\u671f\uff1a{0}",
  "ClientFailed": "\u6d88\u606f\u9a8c\u6536\u5ba2\u6237\u7aef\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {0}\uff1b\u65e5\u5fd7\uff1a{1}\u3001{2}",
  "ClientLabel": "Node \u5ba2\u6237\u7aef",
  "SqlFailed": "\u6d88\u606f\u9a8c\u6536 SQL \u8bc1\u636e\u4e0d\u7b26\u5408\u9884\u671f\uff1a{0}",
  "TenantFixture": "\u5199\u5165\u9694\u79bb\u79df\u6237\u6d88\u606f\u9a8c\u6536\u5939\u5177",
  "TenantPublish": "\u53d1\u5e03\u9694\u79bb\u79df\u6237 Redis \u5524\u9192",
  "RedisFaultFixture": "\u5199\u5165 Redis \u6545\u969c\u671f\u95f4\u7684 MySQL \u8865\u62c9\u5939\u5177",
  "RetentionPrepare": "\u5c06\u6307\u5b9a\u9a8c\u6536\u6d88\u606f\u7f6e\u4e3a 90 \u5929\u4fdd\u7559\u671f\u5df2\u8fc7\u671f",
  "RetentionWorker": "\u8fd0\u884c\u771f\u5b9e\u6d88\u606f\u4fdd\u7559\u6e05\u7406 Worker \u4efb\u52a1",
  "RetentionVerify": "\u9a8c\u8bc1 90 \u5929\u6d88\u606f\u6e05\u7406\u53ca\u7ea7\u8054\u8bb0\u5f55",
  "ApiAInterruptedLabel": "API-A Redis \u4e2d\u65ad",
  "ApiBInterruptedLabel": "API-B Redis \u4e2d\u65ad",
  "ApiARestoredLabel": "API-A Redis \u6062\u590d",
  "ApiBRestoredLabel": "API-B Redis \u6062\u590d",
  "DockerCleanup": "\u6d88\u606f\u4e2d\u5fc3 Docker \u8d44\u6e90\u6e05\u7406\u5931\u8d25\uff1a{0}",
  "RedisRestore": "Redis \u6545\u969c\u6062\u590d\u5931\u8d25\uff1a{0}",
  "ProcessCleanup": "{0}\u8fdb\u7a0b\u6e05\u7406\u5931\u8d25\uff1a{1}",
  "TranscriptCleanup": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u65e5\u5fd7\u6536\u5c3e\u5931\u8d25\uff1a{0}",
  "EnvironmentRestore": "\u6062\u590d\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u524d\u7684\u8fdb\u7a0b\u73af\u5883\u5931\u8d25\uff1a{0}",
  "MetadataWrite": "\u6d88\u606f\u4e2d\u5fc3\u9a8c\u6536\u8bc1\u636e\u5199\u5165\u5931\u8d25\uff1a{0}",
  "Success": "\u6d88\u606f\u4e2d\u5fc3\u5b89\u5168\u7968\u636e\u3001\u6162\u6d88\u8d39\u8005\u3001\u79df\u6237\u9694\u79bb\u3001\u6301\u4e45\u72b6\u6001\u3001\u4fdd\u7559\u6e05\u7406\u4e0e Redis \u6545\u969c\u6062\u590d\u9a8c\u6536\u901a\u8fc7\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{0}"
}
'@

if ($ConfirmRun -cne "RUN-RYFRAME-V0-7-STAGE") {
    throw $script:MessageAcceptanceMessages.OptIn
}
if ($PSVersionTable.PSVersion -lt [version]"5.1") {
    throw $script:MessageAcceptanceMessages.PowerShellVersion
}

function Test-MessageAcceptanceSamePath {
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

function Get-MessageAcceptanceCommand {
    param([Parameter(Mandatory = $true)][string]$Name)

    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command -or [string]::IsNullOrWhiteSpace($command.Source)) {
        throw ($script:MessageAcceptanceMessages.MissingCommand -f $Name)
    }
    return $command.Source
}

function Invoke-MessageAcceptanceCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Description
    )

    Write-Host ("`n==> {0}" -f $Description)
    & $Executable @Arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:MessageAcceptanceMessages.CommandFailed -f $Description, $exitCode)
    }
}

function Invoke-MessageAcceptanceSql {
    param(
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$DockerContext,
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$ComposeFile,
        [Parameter(Mandatory = $true)][string]$OwnershipComposeFile,
        [Parameter(Mandatory = $true)][string]$Sql,
        [Parameter(Mandatory = $true)][string]$Description
    )

    Write-Host ("`n==> {0}" -f $Description)
    $lines = @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $DockerContext `
        -Arguments @(
            "compose", "--project-name", $ProjectName,
            "--file", $ComposeFile,
            "--file", $OwnershipComposeFile,
            "exec", "-T", "mysql", "env", "MYSQL_PWD=ryframe_test_password", "mysql",
            "--user=root",
            "--database=ryframe_test", "--batch", "--skip-column-names", "--raw",
            "--execute", $Sql
        ))
    foreach ($line in $lines) {
        Write-Host $line
    }
    return @($lines)
}

function Assert-MessageAcceptanceSqlResult {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Lines,
        [Parameter(Mandatory = $true)][string]$Expected
    )

    $actual = (@($Lines | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" }) -join "`n")
    if ($actual -cne $Expected) {
        throw ($script:MessageAcceptanceMessages.SqlFailed -f $actual)
    }
}

function Invoke-MessageAcceptanceRedisPublish {
    param(
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$DockerContext,
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$ComposeFile,
        [Parameter(Mandatory = $true)][string]$OwnershipComposeFile,
        [Parameter(Mandatory = $true)][string]$MessageId
    )

    Write-Host ("`n==> {0}" -f $script:MessageAcceptanceMessages.TenantPublish)
    $lines = @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $DockerContext `
        -Arguments @(
            "compose", "--project-name", $ProjectName,
            "--file", $ComposeFile,
            "--file", $OwnershipComposeFile,
            "exec", "-T", "redis", "redis-cli", "PUBLISH",
            "ryframe:message:dispatch", $MessageId
        ))
    Assert-MessageAcceptanceSqlResult -Lines $lines -Expected "2"
}

function Get-MessageAcceptanceFreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function Get-MessageAcceptancePorts {
    param([Parameter(Mandatory = $true)][string[]]$Names)

    $ports = [ordered]@{}
    $used = New-Object System.Collections.Generic.HashSet[int]
    foreach ($name in $Names) {
        do {
            $port = Get-MessageAcceptanceFreePort
        } while (-not $used.Add($port))
        $ports[$name] = $port
    }
    return $ports
}

function Assert-MessageAcceptancePortsAvailable {
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
            throw ($script:MessageAcceptanceMessages.PortUnavailable -f $port)
        }
        finally {
            $listener.Stop()
        }
    }
}

function Set-MessageAcceptanceEnvironment {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Value
    )

    [System.Environment]::SetEnvironmentVariable($Name, $Value, "Process")
}

function Start-MessageAcceptanceProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Arguments,
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
    if ($Arguments.Count -gt 0) {
        $startArguments.ArgumentList = (@($Arguments | ForEach-Object {
            ConvertTo-RyFrameV07ProcessArgument -Value $_
        }) -join " ")
    }
    if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        $startArguments.WindowStyle = "Hidden"
    }
    return Start-Process @startArguments
}

function Assert-MessageAcceptanceProcess {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $ownedProcess = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $ownedProcess) {
        throw ($script:MessageAcceptanceMessages.ProcessExited -f $Label, $Process.Id)
    }
    return $ownedProcess
}

function Stop-MessageAcceptanceProcess {
    param(
        [AllowNull()][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($null -eq $Process) {
        return
    }
    $ownedProcess = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $ownedProcess) {
        return
    }
    Stop-Process -InputObject $ownedProcess -ErrorAction Stop
    if ($ownedProcess.WaitForExit(10000)) {
        return
    }
    $ownedProcess = Assert-MessageAcceptanceProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable `
        -Label $Label
    Stop-Process -InputObject $ownedProcess -Force -ErrorAction Stop
    if (-not $ownedProcess.WaitForExit(10000)) {
        throw ($script:MessageAcceptanceMessages.ProcessStopTimeout -f $Label, $Process.Id)
    }
}

function Wait-MessageAcceptanceReadiness {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 120
    )

    if ($Uri.Scheme -ne "http" -or $Uri.Host -ne "127.0.0.1") {
        throw ($script:MessageAcceptanceMessages.Readiness -f $Label, 0)
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        [void](Assert-MessageAcceptanceProcess `
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
    throw ($script:MessageAcceptanceMessages.Readiness -f $Label, $TimeoutSeconds)
}

function Wait-MessageAcceptanceMetric {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [Parameter(Mandatory = $true)][string]$MetricName,
        [Parameter(Mandatory = $true)][int]$ExpectedValue,
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 30
    )

    if (
        $Uri.Scheme -ne "http" `
        -or $Uri.Host -ne "127.0.0.1" `
        -or $Uri.AbsolutePath -ne "/api/v1/monitor/metrics" `
        -or $MetricName -notmatch "^[a-zA-Z_:][a-zA-Z0-9_:]*$"
    ) {
        throw ($script:MessageAcceptanceMessages.WaitMetric -f $Label, 0, $ExpectedValue)
    }

    $expectedText = $ExpectedValue.ToString([System.Globalization.CultureInfo]::InvariantCulture)
    $pattern = "(?m)^" + [regex]::Escape($MetricName) + "\s+(-?\d+)\s*$"
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        [void](Assert-MessageAcceptanceProcess `
            -Process $Process `
            -ExpectedExecutable $ExpectedExecutable `
            -Label $Label)
        try {
            $response = Invoke-WebRequest -Uri $Uri.AbsoluteUri -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 300) {
                $match = [regex]::Match([string]$response.Content, $pattern)
                if ($match.Success -and $match.Groups[1].Value -eq $expectedText) {
                    return
                }
            }
        }
        catch {
        }
        Start-Sleep -Milliseconds 250
    }
    throw ($script:MessageAcceptanceMessages.WaitMetric -f $Label, $TimeoutSeconds, $ExpectedValue)
}

function Wait-MessageAcceptanceFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [AllowNull()][System.Diagnostics.Process]$Process,
        [AllowNull()][string]$ExpectedExecutable,
        [string]$Label = $script:MessageAcceptanceMessages.ClientLabel,
        [int]$TimeoutSeconds = 30
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $Path -PathType Leaf) {
            return
        }
        if ($null -ne $Process -and $null -ne $ExpectedExecutable) {
            [void](Assert-MessageAcceptanceProcess `
                -Process $Process `
                -ExpectedExecutable $ExpectedExecutable `
                -Label $Label)
        }
        Start-Sleep -Milliseconds 100
    }
    throw ($script:MessageAcceptanceMessages.WaitFile -f $Path)
}

function Wait-MessageAcceptanceProcessExit {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 30
    )

    $ownedProcess = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $ownedProcess) {
        if ($Process.WaitForExit(0)) {
            return $Process.ExitCode
        }
        throw ($script:MessageAcceptanceMessages.ProcessExited -f $Label, $Process.Id)
    }
    if (-not $ownedProcess.WaitForExit($TimeoutSeconds * 1000)) {
        throw ($script:MessageAcceptanceMessages.ProcessStopTimeout -f $Label, $Process.Id)
    }
    return $ownedProcess.ExitCode
}

function Write-MessageAcceptanceSignal {
    param([Parameter(Mandatory = $true)][string]$Path)

    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, "ok`n", $encoding)
}

$scriptFile = (Resolve-Path -LiteralPath $PSCommandPath).Path
$scriptsDirectory = Split-Path -Parent $scriptFile
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDirectory "..")).Path
$expectedScriptsDirectory = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "scripts"))
if (-not (Test-MessageAcceptanceSamePath -Actual $scriptsDirectory -Expected $expectedScriptsDirectory)) {
    throw $script:MessageAcceptanceMessages.ScriptLocation
}

$expectedHelperPath = [System.IO.Path]::GetFullPath((Join-Path $scriptsDirectory "runtime_acceptance_0_7_support.ps1"))
if (-not (Test-Path -LiteralPath $DockerHelperPath -PathType Leaf) -or -not (
    Test-MessageAcceptanceSamePath -Actual $DockerHelperPath -Expected $expectedHelperPath
)) {
    throw ($script:MessageAcceptanceMessages.HelperPath -f $DockerHelperPath)
}
. $expectedHelperPath
Assert-RyFrameV07ProjectName -ProjectName $ProjectName
Assert-RyFrameV07OwnershipToken -OwnershipToken $OwnershipToken

$targetDirectory = Join-Path $repositoryRoot "target"
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path $targetDirectory "runtime-acceptance-0-7"))
$resolvedRunDirectory = [System.IO.Path]::GetFullPath($RunDirectory)
$targetPrefix = $targetRoot.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
) + [System.IO.Path]::DirectorySeparatorChar
$runPathComparison = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) {
    [System.StringComparison]::OrdinalIgnoreCase
}
else {
    [System.StringComparison]::Ordinal
}
if (-not $resolvedRunDirectory.StartsWith($targetPrefix, $runPathComparison)) {
    throw ($script:MessageAcceptanceMessages.RunDirectory -f $resolvedRunDirectory)
}
if (-not (Test-Path -LiteralPath $resolvedRunDirectory -PathType Container)) {
    throw ($script:MessageAcceptanceMessages.RunDirectory -f $resolvedRunDirectory)
}

$composeFile = Join-Path $repositoryRoot "docker-compose.test.yml"
$ownershipComposeFile = Join-Path $repositoryRoot "deploy/tests/runtime-acceptance-0-7-ownership.compose.yml"
$configDirectory = Join-Path $repositoryRoot "config"
$clientScript = Join-Path $scriptsDirectory "message_runtime_acceptance_client.mjs"
foreach ($requiredPath in @(
    $composeFile,
    $ownershipComposeFile,
    $clientScript,
    (Join-Path $repositoryRoot "Cargo.toml")
)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw ($script:MessageAcceptanceMessages.MissingFile -f $requiredPath)
    }
}

$metadataPath = Join-Path $resolvedRunDirectory "message-run.json"
if (Test-Path -LiteralPath $metadataPath) {
    throw ($script:MessageAcceptanceMessages.EvidenceExists -f $metadataPath)
}
$transcriptPath = Join-Path $resolvedRunDirectory "message-transcript.log"
$apiAOutput = Join-Path $resolvedRunDirectory "api-a.stdout.log"
$apiAError = Join-Path $resolvedRunDirectory "api-a.stderr.log"
$apiBOutput = Join-Path $resolvedRunDirectory "api-b.stdout.log"
$apiBError = Join-Path $resolvedRunDirectory "api-b.stderr.log"
$clientOutput = Join-Path $resolvedRunDirectory "client.stdout.log"
$clientError = Join-Path $resolvedRunDirectory "client.stderr.log"
$clientReadyPath = Join-Path $resolvedRunDirectory "client-ready.json"
$tenantFixturePath = Join-Path $resolvedRunDirectory "tenant-fixture.json"
$tenantResultPath = Join-Path $resolvedRunDirectory "tenant-result.json"
$clientDeliveredPath = Join-Path $resolvedRunDirectory "client-delivered.json"
$clientResultPath = Join-Path $resolvedRunDirectory "client-result.json"
$cleanupReadyPath = Join-Path $resolvedRunDirectory "cleanup-ready.json"
$cleanupResultPath = Join-Path $resolvedRunDirectory "cleanup-result.json"
$redisFaultFixturePath = Join-Path $resolvedRunDirectory "redis-fault-fixture.json"
$redisRestoredSignal = Join-Path $resolvedRunDirectory "redis-restored.signal"

$ports = Get-MessageAcceptancePorts -Names @("mysql", "redis", "rustfs", "api_a", "api_b")
$metadata = [ordered]@{
    schema_version = 1
    stage = "message"
    status = "starting"
    started_at = [DateTime]::UtcNow.ToString("o")
    completed_at = $null
    docker_project = $ProjectName
    docker_context = $DockerContext
    ownership_token = $OwnershipToken
    images = @()
    run_directory = $resolvedRunDirectory
    ports = $ports
    redis_fault = [ordered]@{
        method = "docker_stop_start"
        listener_metric = "ryframe_message_redis_listener_connected"
        interrupted = $false
        restored = $false
    }
    scenario_evidence = [ordered]@{
        tenant_isolation = $null
        retention_cleanup = $null
    }
    client_result = $null
    error = $null
    cleanup_errors = @()
}
Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

$runError = $null
$runSucceeded = $false
$cleanupErrors = New-Object System.Collections.Generic.List[string]
$transcriptStarted = $false
$dockerOwned = $false
$redisFault = $null
$apiAProcess = $null
$apiBProcess = $null
$clientProcess = $null
$apiBinary = $null
$workerBinary = $null
$nodeExecutable = $null
$originalLocation = (Get-Location).Path
$locationChanged = $false
$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot

try {
    Start-Transcript -LiteralPath $transcriptPath -Force | Out-Null
    $transcriptStarted = $true
    Set-Location -LiteralPath $repositoryRoot
    $locationChanged = $true
    Assert-MessageAcceptancePortsAvailable -Ports $ports

    $cargoExecutable = Get-MessageAcceptanceCommand -Name "cargo"
    $nodeExecutable = Get-MessageAcceptanceCommand -Name "node"
    $resolvedDockerExecutable = (Resolve-Path -LiteralPath $DockerExecutable).Path
    $contextInfo = Get-RyFrameV07LocalDockerContext -DockerExecutable $resolvedDockerExecutable
    if ($contextInfo.Name -cne $DockerContext) {
        throw ($script:MessageAcceptanceMessages.ContextMismatch -f $contextInfo.Name, $DockerContext)
    }
    $dockerServerVersion = Get-RyFrameV07DockerServerVersion `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["docker_server_version"] = $dockerServerVersion
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

    Set-MessageAcceptanceEnvironment -Name "RYFRAME_TEST_MYSQL_PORT" -Value $ports.mysql.ToString()
    Set-MessageAcceptanceEnvironment -Name "RYFRAME_TEST_REDIS_PORT" -Value $ports.redis.ToString()
    Set-MessageAcceptanceEnvironment -Name "RYFRAME_TEST_RUSTFS_PORT" -Value $ports.rustfs.ToString()
    Set-MessageAcceptanceEnvironment -Name "RYFRAME_TEST_MYSQL_ADMIN_URL" `
        -Value "mysql://root:ryframe_test_password@127.0.0.1:$($ports.mysql)/mysql"
    Set-MessageAcceptanceEnvironment -Name "NO_PROXY" -Value "127.0.0.1,localhost"
    Set-MessageAcceptanceEnvironment -Name "RYFRAME_V07_OWNERSHIP_TOKEN" -Value $OwnershipToken

    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments @(
            "compose", "--project-name", $ProjectName,
            "--file", $composeFile,
            "--file", $ownershipComposeFile,
            "config", "--quiet"
        ) `
        -Description $script:MessageAcceptanceMessages.ComposeValidate
    Assert-RyFrameV07ProjectEmpty `
        -ProjectName $ProjectName `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $dockerOwned = $true
    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments @(
            "compose", "--project-name", $ProjectName,
            "--file", $composeFile,
            "--file", $ownershipComposeFile,
            "up", "-d", "--wait", "mysql", "redis", "rustfs"
        ) `
        -Description $script:MessageAcceptanceMessages.ComposeStart
    $imageEvidence = @(Get-RyFrameV07ProjectImageEvidence `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext)
    $imageServices = @($imageEvidence | ForEach-Object { [string]$_.service } | Sort-Object)
    if ($imageEvidence.Count -ne 3 -or ($imageServices -join ",") -cne "mysql,redis,rustfs") {
        throw ($script:MessageAcceptanceMessages.ImageEvidence -f ($imageServices -join ","))
    }
    $metadata["images"] = $imageEvidence
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    Invoke-MessageAcceptanceCommand `
        -Executable $cargoExecutable `
        -Arguments @("build", "--locked", "-p", "ryframe", "--bins") `
        -Description $script:MessageAcceptanceMessages.Build

    $binarySuffix = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) { ".exe" } else { "" }
    $debugDirectory = Join-Path $targetDirectory "debug"
    $apiBinary = Join-Path $debugDirectory "ryframe$binarySuffix"
    $resetBinary = Join-Path $debugDirectory "ryframe-db-reset$binarySuffix"
    $migrateBinary = Join-Path $debugDirectory "ryframe-migrate$binarySuffix"
    $workerBinary = Join-Path $debugDirectory "ryframe-worker$binarySuffix"
    foreach ($binary in @($apiBinary, $resetBinary, $migrateBinary, $workerBinary)) {
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
            throw ($script:MessageAcceptanceMessages.MissingBinary -f $binary)
        }
    }
    $apiBinary = (Resolve-Path -LiteralPath $apiBinary).Path
    $resetBinary = (Resolve-Path -LiteralPath $resetBinary).Path
    $migrateBinary = (Resolve-Path -LiteralPath $migrateBinary).Path
    $workerBinary = (Resolve-Path -LiteralPath $workerBinary).Path

    Set-MessageAcceptanceEnvironment -Name "APP_CONFIG_DIR" -Value $configDirectory
    Set-MessageAcceptanceEnvironment -Name "APP_ENV" -Value "test"
    Set-MessageAcceptanceEnvironment -Name "APP_APP_HOST" -Value "127.0.0.1"
    Set-MessageAcceptanceEnvironment -Name "APP_API_DOCS_ENABLED" -Value "false"
    Set-MessageAcceptanceEnvironment -Name "APP_MONITOR_METRICS_BEARER_TOKEN" -Value ""
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_HOST" -Value "127.0.0.1"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_PORT" -Value $ports.mysql.ToString()
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_NAME" -Value "ryframe_test"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_USERNAME" -Value "root"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_PASSWORD" -Value "ryframe_test_password"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_TLS_MODE" -Value "disabled"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_MIGRATION_MODE" -Value "verify"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_REPLICAS" -Value "[]"
    Set-MessageAcceptanceEnvironment -Name "APP_DATABASE_SOURCES" -Value "[]"
    Set-MessageAcceptanceEnvironment -Name "APP_REDIS_MODE" -Value "optional"
    Set-MessageAcceptanceEnvironment -Name "APP_REDIS_HOST" -Value "127.0.0.1"
    Set-MessageAcceptanceEnvironment -Name "APP_REDIS_PORT" -Value $ports.redis.ToString()
    Set-MessageAcceptanceEnvironment -Name "APP_REDIS_PASSWORD" -Value ""
    Set-MessageAcceptanceEnvironment -Name "APP_REDIS_DATABASE" -Value "0"
    Set-MessageAcceptanceEnvironment -Name "APP_REDIS_TLS" -Value "false"
    Set-MessageAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_BACKEND" -Value "rustfs"
    Set-MessageAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_ENDPOINT" -Value "http://127.0.0.1:$($ports.rustfs)"
    Set-MessageAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_ACCESS_KEY" -Value "ryframe-test-access"
    Set-MessageAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_SECRET_KEY" -Value "ryframe-test-secret-2026"
    Set-MessageAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_USE_SSL" -Value "false"
    Set-MessageAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_REGION" -Value "us-east-1"
    Set-MessageAcceptanceEnvironment -Name "APP_JOBS_MODE" -Value "external"
    Set-MessageAcceptanceEnvironment -Name "APP_AUTH_JWT_SECRET" -Value "ryframe-v07-message-acceptance-jwt-secret-2026"
    Set-MessageAcceptanceEnvironment -Name "APP_RATE_LIMIT_ENABLED" -Value "false"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_ENABLED" -Value "true"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_TICKET_TTL_SECONDS" -Value "2"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_MAX_CONNECTIONS_PER_USER" -Value "5"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_OUTBOUND_BUFFER" -Value "4"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_RETENTION_DAYS" -Value "90"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_REPLAY_INTERVAL_SECONDS" -Value "3"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_REPLAY_JITTER_SECONDS" -Value "0"
    Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_REPLAY_BATCH_SIZE" -Value "100"
    Set-MessageAcceptanceEnvironment -Name "APP_TELEMETRY_ENABLED" -Value "false"
    Set-MessageAcceptanceEnvironment -Name "APP_LOGGER_OUTPUT" -Value "stdout"
    Set-MessageAcceptanceEnvironment -Name "APP_LOGGER_FORMAT" -Value "text"
    Set-MessageAcceptanceEnvironment -Name "TOKIO_WORKER_THREADS" -Value "1"
    Set-MessageAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "0"

    Invoke-MessageAcceptanceCommand `
        -Executable $resetBinary `
        -Arguments @("--database", "ryframe_test", "--confirm-reset", "RESET-RYFRAME-DATABASE") `
        -Description $script:MessageAcceptanceMessages.ResetDatabase
    Invoke-MessageAcceptanceCommand `
        -Executable $migrateBinary `
        -Arguments @("status") `
        -Description $script:MessageAcceptanceMessages.MigrationStatus
    Invoke-MessageAcceptanceCommand `
        -Executable $migrateBinary `
        -Arguments @("verify") `
        -Description $script:MessageAcceptanceMessages.MigrationVerify

    Set-MessageAcceptanceEnvironment -Name "APP_APP_PORT" -Value $ports.api_a.ToString()
    Set-MessageAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "901"
    $apiAProcess = Start-MessageAcceptanceProcess `
        -Executable $apiBinary `
        -Arguments @() `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $apiAOutput `
        -StandardErrorLog $apiAError
    Wait-MessageAcceptanceReadiness `
        -Uri "http://127.0.0.1:$($ports.api_a)/readyz" `
        -Process $apiAProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API-A"
    Wait-MessageAcceptanceMetric `
        -Uri "http://127.0.0.1:$($ports.api_a)/api/v1/monitor/metrics" `
        -MetricName "ryframe_message_redis_listener_connected" `
        -ExpectedValue 1 `
        -Process $apiAProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API-A"

    Set-MessageAcceptanceEnvironment -Name "APP_APP_PORT" -Value $ports.api_b.ToString()
    Set-MessageAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "902"
    $apiBProcess = Start-MessageAcceptanceProcess `
        -Executable $apiBinary `
        -Arguments @() `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $apiBOutput `
        -StandardErrorLog $apiBError
    Wait-MessageAcceptanceReadiness `
        -Uri "http://127.0.0.1:$($ports.api_b)/readyz" `
        -Process $apiBProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API-B"
    Wait-MessageAcceptanceMetric `
        -Uri "http://127.0.0.1:$($ports.api_b)/api/v1/monitor/metrics" `
        -MetricName "ryframe_message_redis_listener_connected" `
        -ExpectedValue 1 `
        -Process $apiBProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API-B"

    $clientProcess = Start-MessageAcceptanceProcess `
        -Executable $nodeExecutable `
        -Arguments @(
            $clientScript,
            "--internal-token", "RUN-RYFRAME-V0-7-MESSAGE-CLIENT",
            "--api-base", "http://127.0.0.1:$($ports.api_a)",
            "--secondary-api-base", "http://127.0.0.1:$($ports.api_b)",
            "--control-directory", $resolvedRunDirectory
        ) `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $clientOutput `
        -StandardErrorLog $clientError
    Wait-MessageAcceptanceFile `
        -Path $clientReadyPath `
        -Process $clientProcess `
        -ExpectedExecutable $nodeExecutable `
        -Label $script:MessageAcceptanceMessages.ClientLabel `
        -TimeoutSeconds 45
    $clientReady = Get-Content -LiteralPath $clientReadyPath -Raw -Encoding utf8 | ConvertFrom-Json
    if (
        $clientReady.status -ne "ready" `
        -or $clientReady.primary_connection_count -ne 3 `
        -or $clientReady.secondary_connection_count -ne 1 `
        -or $clientReady.total_connection_count -ne 4 `
        -or $clientReady.ticket_guards.expired_status -ne 401 `
        -or $clientReady.ticket_guards.wrong_origin_status -ne 403 `
        -or $clientReady.ticket_guards.rejected_origin_preserved_ticket -ne $true `
        -or $clientReady.ticket_guards.replay_status -ne 401 `
        -or $clientReady.slow_consumer.close_code -ne 1013 `
        -or $clientReady.slow_consumer.backlog_count -ne 16 `
        -or $clientReady.slow_consumer.persisted_count -ne 16 `
        -or $clientReady.slow_consumer.read_back_count -ne 16 `
        -or $clientReady.slow_consumer.marked_read_count -lt 0 `
        -or $clientReady.offline_reconnect.disconnected_instance -ne "api_a" `
        -or $clientReady.offline_reconnect.reconnected_instance -ne "api_b" `
        -or $clientReady.offline_reconnect.published_while_offline -ne $true `
        -or $clientReady.offline_reconnect.message_count -ne 1 `
        -or $clientReady.offline_reconnect.replay_query_delta -ne 1 `
        -or $clientReady.offline_reconnect.delivery_delta -ne 1 `
        -or @($clientReady.offline_reconnect.initial_connections).Count -ne 2 `
        -or @($clientReady.offline_reconnect.initial_connections | Where-Object { $_ -ne 0 }).Count -ne 0 `
        -or $clientReady.offline_reconnect.final_secondary_connections -ne 0 `
        -or $clientReady.offline_reconnect.stability_window.full_replay_cycle_observed -ne $true `
        -or $clientReady.offline_reconnect.stability_window.error_count -ne 0 `
        -or $clientReady.offline_reconnect.stability_window.instance_metrics.api_b.replay_query_delta -lt 1 `
        -or $clientReady.offline_reconnect.stability_window.instance_metrics.api_b.total_replay_query_delta -lt 2 `
        -or $clientReady.offline_reconnect.stability_window.instance_metrics.api_b.delivery_delta -ne 0 `
        -or $clientReady.offline_reconnect.stability_window.instance_metrics.api_b.connection_count -ne 1 `
        -or @($clientReady.offline_reconnect.stability_window.probe_counts).Count -ne 1 `
        -or @($clientReady.offline_reconnect.stability_window.probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or @($clientReady.offline_reconnect.stability_window.final_probe_counts).Count -ne 1 `
        -or @($clientReady.offline_reconnect.stability_window.final_probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0
    ) {
        throw ($script:MessageAcceptanceMessages.ClientResult -f ($clientReady | ConvertTo-Json -Compress))
    }

    $tenantFixtureSql = @'
START TRANSACTION;
INSERT INTO sys_tenant (id, tenant_id, name, status)
VALUES (900000000000000101, 'runtime-isolated', 'runtime-isolated-tenant', '1');
INSERT INTO sys_user (id, tenant_id, username, password_hash, nickname, status, authorization_version, del_flag)
SELECT 900000000000000102, 'runtime-isolated', 'runtime-isolated-user', password_hash,
       'runtime-isolated-user', '1', 1, '0'
FROM sys_user
WHERE tenant_id = 'system' AND username = 'admin';
INSERT INTO sys_config (id, tenant_id, name, `key`, `value`, remark, del_flag)
VALUES (
    900000000000000104, 'runtime-isolated', 'runtime acceptance captcha switch',
    'sys.account.captchaEnabled', 'false', 'runtime acceptance only', '0'
);
INSERT INTO sys_message (
    id, tenant_id, topic, title_text, body_text, severity, source_type, source_id,
    published_at, expires_at, created_at, updated_at
)
VALUES (
    900000000000000103, 'runtime-isolated', 'runtime-acceptance',
    'tenant-isolation-proof', 'tenant-isolation-proof', 'info',
    'runtime_acceptance_0_7_tenant', 'runtime-isolated-message',
    UTC_TIMESTAMP(), UTC_TIMESTAMP() + INTERVAL 1 DAY, UTC_TIMESTAMP(), UTC_TIMESTAMP()
);
INSERT INTO sys_message_audience (message_id, tenant_id, kind, target_id)
VALUES (900000000000000103, 'runtime-isolated', 'user', 900000000000000102);
INSERT INTO sys_message_recipient (
    message_id, user_id, tenant_id, created_at, enqueued_at, acked_at, read_at
)
VALUES (
    900000000000000103, 900000000000000102, 'runtime-isolated',
    UTC_TIMESTAMP(), UTC_TIMESTAMP(), NULL, NULL
);
COMMIT;
SELECT CONCAT(
    (SELECT COUNT(*) FROM sys_tenant WHERE tenant_id = 'runtime-isolated'), ':',
    (SELECT COUNT(*) FROM sys_user WHERE tenant_id = 'runtime-isolated' AND id = 900000000000000102), ':',
    (SELECT COUNT(*) FROM sys_config
     WHERE tenant_id = 'runtime-isolated'
       AND id = 900000000000000104
       AND `key` = 'sys.account.captchaEnabled'
       AND `value` = 'false'
       AND del_flag = '0'), ':',
    (SELECT COUNT(*) FROM sys_message WHERE tenant_id = 'runtime-isolated' AND id = 900000000000000103), ':',
    (SELECT COUNT(*) FROM sys_message_recipient WHERE tenant_id = 'runtime-isolated' AND message_id = 900000000000000103)
);
'@
    $tenantFixtureLines = @(Invoke-MessageAcceptanceSql `
        -DockerExecutable $resolvedDockerExecutable `
        -DockerContext $DockerContext `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -OwnershipComposeFile $ownershipComposeFile `
        -Sql $tenantFixtureSql `
        -Description $script:MessageAcceptanceMessages.TenantFixture)
    Assert-MessageAcceptanceSqlResult -Lines $tenantFixtureLines -Expected "1:1:1:1:1"
    Invoke-MessageAcceptanceRedisPublish `
        -DockerExecutable $resolvedDockerExecutable `
        -DockerContext $DockerContext `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -OwnershipComposeFile $ownershipComposeFile `
        -MessageId "900000000000000103"
    Write-RyFrameV07MetadataAtomically `
        -Metadata ([ordered]@{
            status = "ready"
            tenant_id = "runtime-isolated"
            username = "runtime-isolated-user"
            user_id = "900000000000000102"
            message_id = "900000000000000103"
            expected_text = "tenant-isolation-proof"
        }) `
        -Path $tenantFixturePath
    Wait-MessageAcceptanceFile `
        -Path $tenantResultPath `
        -Process $clientProcess `
        -ExpectedExecutable $nodeExecutable `
        -Label $script:MessageAcceptanceMessages.ClientLabel `
        -TimeoutSeconds 60
    $tenantResult = Get-Content -LiteralPath $tenantResultPath -Raw -Encoding utf8 | ConvertFrom-Json
    if (
        $tenantResult.status -ne "passed" `
        -or $tenantResult.tenant_id -ne "runtime-isolated" `
        -or $tenantResult.message_id -ne "900000000000000103" `
        -or $tenantResult.system_inbox_count -ne 0 `
        -or $tenantResult.system_connection_count -ne 0 `
        -or $tenantResult.isolated_inbox_count -ne 1 `
        -or $tenantResult.isolated_connection_count -ne 1
    ) {
        throw ($script:MessageAcceptanceMessages.ClientResult -f ($tenantResult | ConvertTo-Json -Depth 8 -Compress))
    }
    $metadata["scenario_evidence"]["tenant_isolation"] = $tenantResult
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $redisFault = Stop-RyFrameV07DockerService `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -ComposeFile $composeFile `
        -Service "redis" `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    Wait-MessageAcceptanceMetric `
        -Uri "http://127.0.0.1:$($ports.api_a)/api/v1/monitor/metrics" `
        -MetricName "ryframe_message_redis_listener_connected" `
        -ExpectedValue 0 `
        -Process $apiAProcess `
        -ExpectedExecutable $apiBinary `
        -Label $script:MessageAcceptanceMessages.ApiAInterruptedLabel
    Wait-MessageAcceptanceMetric `
        -Uri "http://127.0.0.1:$($ports.api_b)/api/v1/monitor/metrics" `
        -MetricName "ryframe_message_redis_listener_connected" `
        -ExpectedValue 0 `
        -Process $apiBProcess `
        -ExpectedExecutable $apiBinary `
        -Label $script:MessageAcceptanceMessages.ApiBInterruptedLabel
    $metadata["redis_fault"]["interrupted"] = $true
    $metadata["redis_fault"]["interrupted_instance_count"] = 2
    $redisFaultFixtureSql = @'
START TRANSACTION;
INSERT INTO sys_message (
    id, tenant_id, topic, title_text, body_text, title_key, body_key, args_json,
    severity, source_type, source_id, created_by, published_at, expires_at, created_at, updated_at
)
VALUES (
    900000000000000105, 'system', 'runtime-acceptance', NULL, NULL,
    'user.welcome', 'user.welcome', JSON_OBJECT('name', 'redis-fault-proof'),
    'info', 'runtime_acceptance_0_7_redis_fault', 'redis-fault-proof', 1,
    UTC_TIMESTAMP(), UTC_TIMESTAMP() + INTERVAL 1 DAY, UTC_TIMESTAMP(), UTC_TIMESTAMP()
);
INSERT INTO sys_message_audience (message_id, tenant_id, kind, target_id)
VALUES (900000000000000105, 'system', 'user', 1);
INSERT INTO sys_message_recipient (
    message_id, user_id, tenant_id, created_at, enqueued_at, acked_at, read_at
)
VALUES (900000000000000105, 1, 'system', UTC_TIMESTAMP(), NULL, NULL, NULL);
COMMIT;
SELECT CONCAT(
    (SELECT COUNT(*) FROM sys_message
     WHERE id = 900000000000000105
       AND tenant_id = 'system'
       AND source_type = 'runtime_acceptance_0_7_redis_fault'
       AND source_id = 'redis-fault-proof'), ':',
    (SELECT COUNT(*) FROM sys_message_audience
     WHERE message_id = 900000000000000105
       AND tenant_id = 'system'
       AND kind = 'user'
       AND target_id = 1), ':',
    (SELECT COUNT(*) FROM sys_message_recipient
     WHERE message_id = 900000000000000105
       AND tenant_id = 'system'
       AND user_id = 1
       AND acked_at IS NULL
       AND read_at IS NULL)
);
'@
    $redisFaultFixtureLines = @(Invoke-MessageAcceptanceSql `
        -DockerExecutable $resolvedDockerExecutable `
        -DockerContext $DockerContext `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -OwnershipComposeFile $ownershipComposeFile `
        -Sql $redisFaultFixtureSql `
        -Description $script:MessageAcceptanceMessages.RedisFaultFixture)
    Assert-MessageAcceptanceSqlResult -Lines $redisFaultFixtureLines -Expected "1:1:1"
    $metadata["redis_fault"]["message_id"] = "900000000000000105"
    $metadata["redis_fault"]["fixture_source"] = "mysql"
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
    Write-RyFrameV07MetadataAtomically `
        -Metadata ([ordered]@{
            status = "ready"
            message_id = "900000000000000105"
            source_type = "runtime_acceptance_0_7_redis_fault"
        }) `
        -Path $redisFaultFixturePath

    Wait-MessageAcceptanceFile `
        -Path $clientDeliveredPath `
        -Process $clientProcess `
        -ExpectedExecutable $nodeExecutable `
        -Label $script:MessageAcceptanceMessages.ClientLabel `
        -TimeoutSeconds 45
    $clientDelivered = Get-Content -LiteralPath $clientDeliveredPath -Raw -Encoding utf8 | ConvertFrom-Json
    if (
        $clientDelivered.status -ne "delivered" `
        -or $clientDelivered.message_id -ne "900000000000000105" `
        -or $clientDelivered.fixture_source -ne "mysql" `
        -or $clientDelivered.published_while_redis_unavailable -ne $true `
        -or $clientDelivered.primary_connection_count -ne 3 `
        -or $clientDelivered.secondary_connection_count -ne 1 `
        -or $clientDelivered.total_connection_count -ne 4 `
        -or $clientDelivered.instance_metrics.api_a.replay_query_delta -lt 1 `
        -or $clientDelivered.instance_metrics.api_a.delivery_delta -ne 3 `
        -or $clientDelivered.instance_metrics.api_b.replay_query_delta -lt 1 `
        -or $clientDelivered.instance_metrics.api_b.delivery_delta -ne 1 `
        -or $clientDelivered.websocket_ack_received -ne $true `
        -or $clientDelivered.ticket_guards.expired_status -ne 401 `
        -or $clientDelivered.ticket_guards.wrong_origin_status -ne 403 `
        -or $clientDelivered.ticket_guards.replay_status -ne 401 `
        -or $clientDelivered.slow_consumer.close_code -ne 1013 `
        -or $clientDelivered.offline_reconnect.disconnected_instance -ne "api_a" `
        -or $clientDelivered.offline_reconnect.reconnected_instance -ne "api_b" `
        -or $clientDelivered.offline_reconnect.published_while_offline -ne $true `
        -or $clientDelivered.offline_reconnect.message_count -ne 1 `
        -or $clientDelivered.offline_reconnect.replay_query_delta -ne 1 `
        -or $clientDelivered.offline_reconnect.delivery_delta -ne 1 `
        -or @($clientDelivered.offline_reconnect.initial_connections).Count -ne 2 `
        -or @($clientDelivered.offline_reconnect.initial_connections | Where-Object { $_ -ne 0 }).Count -ne 0 `
        -or $clientDelivered.offline_reconnect.final_secondary_connections -ne 0 `
        -or $clientDelivered.offline_reconnect.stability_window.full_replay_cycle_observed -ne $true `
        -or $clientDelivered.offline_reconnect.stability_window.error_count -ne 0 `
        -or $clientDelivered.offline_reconnect.stability_window.instance_metrics.api_b.replay_query_delta -lt 1 `
        -or $clientDelivered.offline_reconnect.stability_window.instance_metrics.api_b.total_replay_query_delta -lt 2 `
        -or $clientDelivered.offline_reconnect.stability_window.instance_metrics.api_b.delivery_delta -ne 0 `
        -or $clientDelivered.offline_reconnect.stability_window.instance_metrics.api_b.connection_count -ne 1 `
        -or @($clientDelivered.offline_reconnect.stability_window.probe_counts).Count -ne 1 `
        -or @($clientDelivered.offline_reconnect.stability_window.probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or @($clientDelivered.offline_reconnect.stability_window.final_probe_counts).Count -ne 1 `
        -or @($clientDelivered.offline_reconnect.stability_window.final_probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or $clientDelivered.tenant_isolation.system_inbox_count -ne 0 `
        -or $clientDelivered.tenant_isolation.isolated_inbox_count -ne 1 `
        -or @($clientDelivered.per_connection_counts).Count -ne 4 `
        -or @($clientDelivered.per_connection_counts | Where-Object { $_.count -ne 1 }).Count -ne 0
    ) {
        throw ($script:MessageAcceptanceMessages.ClientResult -f ($clientDelivered | ConvertTo-Json -Depth 8 -Compress))
    }

    Restore-RyFrameV07DockerFault `
        -Fault $redisFault `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $redisFault = $null
    Wait-MessageAcceptanceMetric `
        -Uri "http://127.0.0.1:$($ports.api_a)/api/v1/monitor/metrics" `
        -MetricName "ryframe_message_redis_listener_connected" `
        -ExpectedValue 1 `
        -Process $apiAProcess `
        -ExpectedExecutable $apiBinary `
        -Label $script:MessageAcceptanceMessages.ApiARestoredLabel `
        -TimeoutSeconds 40
    Wait-MessageAcceptanceMetric `
        -Uri "http://127.0.0.1:$($ports.api_b)/api/v1/monitor/metrics" `
        -MetricName "ryframe_message_redis_listener_connected" `
        -ExpectedValue 1 `
        -Process $apiBProcess `
        -ExpectedExecutable $apiBinary `
        -Label $script:MessageAcceptanceMessages.ApiBRestoredLabel `
        -TimeoutSeconds 40
    $metadata["redis_fault"]["restored"] = $true
    $metadata["redis_fault"]["restored_instance_count"] = 2
    Write-MessageAcceptanceSignal -Path $redisRestoredSignal

    Wait-MessageAcceptanceFile `
        -Path $cleanupReadyPath `
        -Process $clientProcess `
        -ExpectedExecutable $nodeExecutable `
        -Label $script:MessageAcceptanceMessages.ClientLabel `
        -TimeoutSeconds 30
    $cleanupReady = Get-Content -LiteralPath $cleanupReadyPath -Raw -Encoding utf8 | ConvertFrom-Json
    $retentionMessageId = [string]$cleanupReady.message_id
    $retentionMessageIdValue = 0L
    $defaultRetentionSeconds = 0
    $overLimitStatus = 0
    if (
        $cleanupReady.status -ne "ready" `
        -or $cleanupReady.tenant_id -ne "system" `
        -or $cleanupReady.source_type -ne "runtime_acceptance_0_7_retention" `
        -or -not [long]::TryParse($retentionMessageId, [ref]$retentionMessageIdValue) `
        -or $retentionMessageIdValue -le 0 `
        -or -not [int]::TryParse([string]$cleanupReady.default_retention_seconds, [ref]$defaultRetentionSeconds) `
        -or $defaultRetentionSeconds -lt 7775995 `
        -or $defaultRetentionSeconds -gt 7776005 `
        -or -not [int]::TryParse([string]$cleanupReady.over_limit_status, [ref]$overLimitStatus) `
        -or $overLimitStatus -ne 400 `
        -or $cleanupReady.over_limit_error_key -ne "validation"
    ) {
        throw ($script:MessageAcceptanceMessages.ClientResult -f ($cleanupReady | ConvertTo-Json -Compress))
    }

    $retentionPrepareSql = @"
START TRANSACTION;
UPDATE sys_message
SET created_at = UTC_TIMESTAMP() - INTERVAL 91 DAY,
    published_at = UTC_TIMESTAMP() - INTERVAL 91 DAY,
    updated_at = UTC_TIMESTAMP() - INTERVAL 91 DAY,
    expires_at = UTC_TIMESTAMP() - INTERVAL 1 DAY
WHERE id = $retentionMessageIdValue
  AND tenant_id = 'system'
  AND source_type = 'runtime_acceptance_0_7_retention';
UPDATE sys_message_recipient
SET created_at = UTC_TIMESTAMP() - INTERVAL 91 DAY
WHERE message_id = $retentionMessageIdValue AND tenant_id = 'system';
SET @retention_job_count := (
    SELECT COUNT(*)
    FROM sys_background_job
    WHERE job_type = 'system.message.retention' AND status = 'pending'
);
SET @retention_job_id := (
    SELECT MIN(id)
    FROM sys_background_job
    WHERE job_type = 'system.message.retention' AND status = 'pending'
);
UPDATE sys_background_job
SET priority = 2147483647, available_at = UTC_TIMESTAMP()
WHERE id = @retention_job_id
  AND @retention_job_count = 1
  AND job_type = 'system.message.retention'
  AND status = 'pending';
SET @retention_job_updated := ROW_COUNT();
COMMIT;
SELECT CONCAT(
    (SELECT COUNT(*) FROM sys_message
     WHERE id = $retentionMessageIdValue
       AND tenant_id = 'system'
       AND source_type = 'runtime_acceptance_0_7_retention'), ':',
    (SELECT TIMESTAMPDIFF(DAY, created_at, UTC_TIMESTAMP()) FROM sys_message
     WHERE id = $retentionMessageIdValue AND tenant_id = 'system'), ':',
    (SELECT expires_at <= UTC_TIMESTAMP() FROM sys_message
     WHERE id = $retentionMessageIdValue AND tenant_id = 'system'), ':',
    @retention_job_count, ':',
    COALESCE(@retention_job_id, 0), ':',
    @retention_job_updated
);
"@
    $retentionPrepareLines = @(Invoke-MessageAcceptanceSql `
        -DockerExecutable $resolvedDockerExecutable `
        -DockerContext $DockerContext `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -OwnershipComposeFile $ownershipComposeFile `
        -Sql $retentionPrepareSql `
        -Description $script:MessageAcceptanceMessages.RetentionPrepare)
    $retentionPrepareEvidence = (@(
        $retentionPrepareLines | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" }
    ) -join "")
    $retentionPrepareFields = @($retentionPrepareEvidence -split ":")
    $retentionJobIdValue = 0L
    if (
        $retentionPrepareFields.Count -ne 6 `
        -or $retentionPrepareFields[0] -ne "1" `
        -or [int]$retentionPrepareFields[1] -lt 90 `
        -or $retentionPrepareFields[2] -ne "1" `
        -or $retentionPrepareFields[3] -ne "1" `
        -or -not [long]::TryParse($retentionPrepareFields[4], [ref]$retentionJobIdValue) `
        -or $retentionJobIdValue -le 0 `
        -or $retentionPrepareFields[5] -ne "1"
    ) {
        throw ($script:MessageAcceptanceMessages.SqlFailed -f $retentionPrepareEvidence)
    }

    Set-MessageAcceptanceEnvironment -Name "APP_JOBS_WORKER_ID" -Value "message-runtime-acceptance"
    Set-MessageAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "903"
    Invoke-MessageAcceptanceCommand `
        -Executable $workerBinary `
        -Arguments @("--once") `
        -Description $script:MessageAcceptanceMessages.RetentionWorker

    $retentionVerifySql = @"
SELECT CONCAT(
    (SELECT COUNT(*) FROM sys_message WHERE id = $retentionMessageIdValue), ':',
    (SELECT COUNT(*) FROM sys_message_audience WHERE message_id = $retentionMessageIdValue), ':',
    (SELECT COUNT(*) FROM sys_message_recipient WHERE message_id = $retentionMessageIdValue), ':',
    COALESCE((SELECT status FROM sys_background_job WHERE id = $retentionJobIdValue), 'missing'), ':',
    COALESCE((SELECT attempts FROM sys_background_job WHERE id = $retentionJobIdValue), -1), ':',
    COALESCE((SELECT last_error IS NULL FROM sys_background_job WHERE id = $retentionJobIdValue), 0)
);
"@
    $retentionVerifyLines = @(Invoke-MessageAcceptanceSql `
        -DockerExecutable $resolvedDockerExecutable `
        -DockerContext $DockerContext `
        -ProjectName $ProjectName `
        -ComposeFile $composeFile `
        -OwnershipComposeFile $ownershipComposeFile `
        -Sql $retentionVerifySql `
        -Description $script:MessageAcceptanceMessages.RetentionVerify)
    $retentionVerifyEvidence = (@(
        $retentionVerifyLines | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" }
    ) -join "")
    Assert-MessageAcceptanceSqlResult `
        -Lines $retentionVerifyLines `
        -Expected "0:0:0:succeeded:1:1"
    $retentionVerifyFields = @($retentionVerifyEvidence -split ":")
    $retentionResult = [ordered]@{
        status = "passed"
        tenant_id = "system"
        message_id = $retentionMessageId
        retention_days = 90
        default_retention_seconds = $defaultRetentionSeconds
        over_limit_status = $overLimitStatus
        over_limit_error_key = [string]$cleanupReady.over_limit_error_key
        aged_days = [int]$retentionPrepareFields[1]
        message_rows = [int]$retentionVerifyFields[0]
        audience_rows = [int]$retentionVerifyFields[1]
        recipient_rows = [int]$retentionVerifyFields[2]
        job_id = $retentionJobIdValue.ToString()
        job_status = $retentionVerifyFields[3]
        job_attempts = [int]$retentionVerifyFields[4]
    }
    Write-RyFrameV07MetadataAtomically -Metadata $retentionResult -Path $cleanupResultPath
    $metadata["scenario_evidence"]["retention_cleanup"] = $retentionResult
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $clientExitCode = Wait-MessageAcceptanceProcessExit `
        -Process $clientProcess `
        -ExpectedExecutable $nodeExecutable `
        -Label $script:MessageAcceptanceMessages.ClientLabel `
        -TimeoutSeconds 30
    if ($clientExitCode -ne 0) {
        throw ($script:MessageAcceptanceMessages.ClientFailed -f $clientExitCode, $clientOutput, $clientError)
    }
    $clientProcess = $null
    Wait-MessageAcceptanceFile -Path $clientResultPath -Process $null -ExpectedExecutable $null
    $clientResult = Get-Content -LiteralPath $clientResultPath -Raw -Encoding utf8 | ConvertFrom-Json
    if (
        $clientResult.status -ne "passed" `
        -or $clientResult.primary_connection_count -ne 3 `
        -or $clientResult.secondary_connection_count -ne 1 `
        -or $clientResult.total_connection_count -ne 4 `
        -or $clientResult.instance_metrics.api_a.replay_query_delta -lt 1 `
        -or $clientResult.instance_metrics.api_a.delivery_delta -ne 3 `
        -or $clientResult.instance_metrics.api_b.replay_query_delta -lt 1 `
        -or $clientResult.instance_metrics.api_b.delivery_delta -ne 1 `
        -or $clientResult.ticket_guards.expired_status -ne 401 `
        -or $clientResult.ticket_guards.wrong_origin_status -ne 403 `
        -or $clientResult.ticket_guards.replay_status -ne 401 `
        -or $clientResult.slow_consumer.close_code -ne 1013 `
        -or $clientResult.offline_reconnect.disconnected_instance -ne "api_a" `
        -or $clientResult.offline_reconnect.reconnected_instance -ne "api_b" `
        -or $clientResult.offline_reconnect.published_while_offline -ne $true `
        -or $clientResult.offline_reconnect.message_count -ne 1 `
        -or $clientResult.offline_reconnect.replay_query_delta -ne 1 `
        -or $clientResult.offline_reconnect.delivery_delta -ne 1 `
        -or @($clientResult.offline_reconnect.initial_connections).Count -ne 2 `
        -or @($clientResult.offline_reconnect.initial_connections | Where-Object { $_ -ne 0 }).Count -ne 0 `
        -or $clientResult.offline_reconnect.final_secondary_connections -ne 0 `
        -or $clientResult.offline_reconnect.stability_window.full_replay_cycle_observed -ne $true `
        -or $clientResult.offline_reconnect.stability_window.error_count -ne 0 `
        -or $clientResult.offline_reconnect.stability_window.instance_metrics.api_b.replay_query_delta -lt 1 `
        -or $clientResult.offline_reconnect.stability_window.instance_metrics.api_b.total_replay_query_delta -lt 2 `
        -or $clientResult.offline_reconnect.stability_window.instance_metrics.api_b.delivery_delta -ne 0 `
        -or $clientResult.offline_reconnect.stability_window.instance_metrics.api_b.connection_count -ne 1 `
        -or @($clientResult.offline_reconnect.stability_window.probe_counts).Count -ne 1 `
        -or @($clientResult.offline_reconnect.stability_window.probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or @($clientResult.offline_reconnect.stability_window.final_probe_counts).Count -ne 1 `
        -or @($clientResult.offline_reconnect.stability_window.final_probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or $clientResult.tenant_isolation.system_inbox_count -ne 0 `
        -or $clientResult.tenant_isolation.system_connection_count -ne 0 `
        -or $clientResult.tenant_isolation.isolated_inbox_count -ne 1 `
        -or $clientResult.tenant_isolation.isolated_connection_count -ne 1 `
        -or $clientResult.persisted_state.verified_across_instances -ne $true `
        -or [string]::IsNullOrWhiteSpace([string]$clientResult.persisted_state.acked_at) `
        -or [string]::IsNullOrWhiteSpace([string]$clientResult.persisted_state.read_at) `
        -or $clientResult.deduplication_stability.full_replay_cycle_observed -ne $true `
        -or $clientResult.deduplication_stability.error_count -ne 0 `
        -or $clientResult.deduplication_stability.instance_metrics.api_a.replay_query_delta -lt 1 `
        -or $clientResult.deduplication_stability.instance_metrics.api_a.total_replay_query_delta -lt 2 `
        -or $clientResult.deduplication_stability.instance_metrics.api_a.delivery_delta -ne 0 `
        -or $clientResult.deduplication_stability.instance_metrics.api_a.connection_count -ne 3 `
        -or $clientResult.deduplication_stability.instance_metrics.api_b.replay_query_delta -lt 1 `
        -or $clientResult.deduplication_stability.instance_metrics.api_b.total_replay_query_delta -lt 2 `
        -or $clientResult.deduplication_stability.instance_metrics.api_b.delivery_delta -ne 0 `
        -or $clientResult.deduplication_stability.instance_metrics.api_b.connection_count -ne 1 `
        -or @($clientResult.deduplication_stability.probe_counts).Count -ne 4 `
        -or @($clientResult.deduplication_stability.probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or @($clientResult.deduplication_stability.final_probe_counts).Count -ne 4 `
        -or @($clientResult.deduplication_stability.final_probe_counts | Where-Object { $_.target_count -ne 1 }).Count -ne 0 `
        -or $clientResult.retention_cleanup.status -ne "passed" `
        -or $clientResult.retention_cleanup.retention_days -ne 90 `
        -or $clientResult.retention_cleanup.default_retention_seconds -lt 7775995 `
        -or $clientResult.retention_cleanup.default_retention_seconds -gt 7776005 `
        -or $clientResult.retention_cleanup.over_limit_status -ne 400 `
        -or $clientResult.retention_cleanup.over_limit_error_key -ne "validation" `
        -or $clientResult.retention_cleanup.message_rows -ne 0 `
        -or $clientResult.retention_cleanup.audience_rows -ne 0 `
        -or $clientResult.retention_cleanup.recipient_rows -ne 0 `
        -or $clientResult.retention_cleanup.job_status -ne "succeeded" `
        -or $clientResult.retention_cleanup.job_attempts -lt 1
    ) {
        throw ($script:MessageAcceptanceMessages.ClientResult -f ($clientResult | ConvertTo-Json -Depth 8 -Compress))
    }
    $metadata["client_result"] = $clientResult
    $runSucceeded = $true
}
catch {
    $runError = $_
    $metadata["error"] = $_.Exception.Message
}
finally {
    if ($null -ne $redisFault) {
        try {
            Restore-RyFrameV07DockerFault `
                -Fault $redisFault `
                -OwnershipToken $OwnershipToken `
                -DockerExecutable $DockerExecutable `
                -Context $DockerContext
        }
        catch {
            $cleanupErrors.Add(($script:MessageAcceptanceMessages.RedisRestore -f $_.Exception.Message))
        }
    }

    foreach ($processInfo in @(
        [pscustomobject]@{
            Process = $clientProcess
            Executable = $nodeExecutable
            Label = $script:MessageAcceptanceMessages.ClientLabel
        },
        [pscustomobject]@{ Process = $apiBProcess; Executable = $apiBinary; Label = "API-B" },
        [pscustomobject]@{ Process = $apiAProcess; Executable = $apiBinary; Label = "API-A" }
    )) {
        if ($null -eq $processInfo.Process -or $null -eq $processInfo.Executable) {
            continue
        }
        try {
            Stop-MessageAcceptanceProcess `
                -Process $processInfo.Process `
                -ExpectedExecutable $processInfo.Executable `
                -Label $processInfo.Label
        }
        catch {
            $cleanupErrors.Add(($script:MessageAcceptanceMessages.ProcessCleanup -f $processInfo.Label, $_.Exception.Message))
        }
    }

    if ($dockerOwned) {
        try {
            Remove-RyFrameV07DockerProjectResources `
                -ProjectName $ProjectName `
                -OwnershipToken $OwnershipToken `
                -DockerExecutable $DockerExecutable `
                -Context $DockerContext
        }
        catch {
            $cleanupErrors.Add(($script:MessageAcceptanceMessages.DockerCleanup -f $_.Exception.Message))
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
            $cleanupErrors.Add(($script:MessageAcceptanceMessages.TranscriptCleanup -f $_.Exception.Message))
        }
    }

    try {
        Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot
    }
    catch {
        $cleanupErrors.Add(($script:MessageAcceptanceMessages.EnvironmentRestore -f $_.Exception.Message))
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
        $metadataError = $script:MessageAcceptanceMessages.MetadataWrite -f $_.Exception.Message
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
Write-Host ("`n" + ($script:MessageAcceptanceMessages.Success -f $resolvedRunDirectory))
