[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:FileAMessages = ConvertFrom-Json @'
{
  "MissingTool": "FILE-A \u9a8c\u6536\u7f3a\u5c11\u547d\u4ee4\uff1a{0}",
  "CommandFailed": "\u547d\u4ee4\u6267\u884c\u5931\u8d25\uff08\u9000\u51fa\u7801 {0}\uff09\uff1a{1}\n{2}",
  "OutputMissing": "{0} \u672a\u8f93\u51fa\u9884\u671f\u6807\u8bb0\uff1a{1}",
  "OutputMismatch": "{0} \u672a\u6ee1\u8db3\u8f93\u51fa\u65ad\u8a00\uff1a{1}",
  "DockerContextEmpty": "Docker context \u4e3a\u7a7a\uff0c\u62d2\u7edd\u8fd0\u884c FILE-A \u9a8c\u6536",
  "DockerRemote": "Docker context {0} \u6307\u5411\u8fdc\u7a0b endpoint {1}\uff0c\u62d2\u7edd\u8fd0\u884c FILE-A \u9a8c\u6536",
  "MigrationUnexpected": "000017 \u5e94\u5728 {0} \u672a\u5904\u7406\u65f6\u95ed\u9501\uff0c\u4f46\u8fc1\u79fb\u610f\u5916\u6210\u529f",
  "MigrationGuardContext": "000017 \u95ed\u9501",
  "Version": "FILE-A \u9a8c\u6536\u9700\u8981 PowerShell 5.1 \u6216\u66f4\u9ad8\u7248\u672c",
  "NameValidation": "FILE-A \u9694\u79bb\u540d\u79f0\u6821\u9a8c\u5931\u8d25",
  "RunRootEscape": "FILE-A \u4e34\u65f6\u76ee\u5f55\u8d8a\u8fc7\u4e13\u7528\u9a8c\u6536\u6839\u76ee\u5f55",
  "MissingFixture": "\u627e\u4e0d\u5230\u56fa\u5b9a\u7684 v0.4.2 MySQL \u5939\u5177\uff1a{0}",
  "StepDockerContext": "\u786e\u8ba4 Docker context \u4ec5\u6307\u5411\u672c\u673a npipe \u6216 unix endpoint",
  "StepBuild": "\u4e00\u6b21\u6784\u5efa\u8fc1\u79fb\u4e0e FILE-A \u7ef4\u62a4\u4e8c\u8fdb\u5236",
  "MissingBinary": "\u6784\u5efa\u5b8c\u6210\u540e\u4ecd\u627e\u4e0d\u5230 FILE-A \u4e8c\u8fdb\u5236\uff1a{0}",
  "StepStart": "\u542f\u52a8\u552f\u4e00 Compose project\uff0c\u5e76\u7b49\u5f85 MySQL \u4e0e RustFS \u5c31\u7eea",
  "StepImport": "\u5411\u552f\u4e00\u4e34\u65f6\u6570\u636e\u5e93\u5bfc\u5165\u56fa\u5b9a v0.4.2 \u65e7 schema",
  "StepSeed": "\u5199\u5165\u65e7 file_md5 \u78b0\u649e\u6570\u636e\u4e0e\u4e24\u4e2a\u771f\u5b9e RustFS \u5bf9\u8c61",
  "SeedExpected": "FILE-A \u65e7 schema\u3001MD5 \u78b0\u649e\u6570\u636e\u4e0e RustFS \u5bf9\u8c61\u79cd\u5b50\u5df2\u5c31\u7eea",
  "SeedContext": "\u65e7\u6570\u636e\u79cd\u5b50",
  "StepBackfillGuard": "\u8bc1\u660e 000017 \u5728 SHA-256 \u56de\u586b\u524d\u95ed\u9501",
  "StepBackfillDry": "\u6267\u884c SHA-256 \u56de\u586b dry-run",
  "BackfillDryContext": "SHA-256 \u56de\u586b dry-run",
  "StepBackfillApply": "\u6267\u884c SHA-256 \u56de\u586b apply",
  "BackfillApplyContext": "SHA-256 \u56de\u586b apply",
  "StepDrainGuard": "\u8bc1\u660e 000017 \u5728\u65e7\u4e0a\u4f20\u9884\u7559\u6392\u7a7a\u524d\u4ecd\u95ed\u9501",
  "StepDrainDry": "\u6267\u884c\u65e7\u4e0a\u4f20\u9884\u7559\u6392\u7a7a dry-run",
  "DrainDryContext": "\u9884\u7559\u6392\u7a7a dry-run",
  "StepDrainApply": "\u6267\u884c\u65e7\u4e0a\u4f20\u9884\u7559\u6392\u7a7a apply \u81f3 remaining=0",
  "DrainApplyContext": "\u9884\u7559\u6392\u7a7a apply",
  "StepFinalMigration": "\u5b8c\u6210\u8fc1\u79fb\u5e76\u6267\u884c status \u4e0e verify",
  "MigrationUpContext": "\u6700\u7ec8 migration up",
  "MigrationStatusContext": "migration status",
  "MigrationVerifyContext": "migration verify",
  "StepFinalAssert": "\u65ad\u8a00\u65e7\u5217\u3001\u65e7\u7d22\u5f15\u3001SHA-256 \u4e0e MD5 \u78b0\u649e\u9694\u79bb\u7ed3\u679c",
  "FinalExpected": "FILE-A \u6700\u7ec8 schema\u3001SHA-256 \u4e0e\u78b0\u649e\u5bf9\u8c61\u9694\u79bb\u65ad\u8a00\u5df2\u901a\u8fc7",
  "FinalContext": "\u6700\u7ec8\u65ad\u8a00",
  "CleanupFailed": "FILE-A Compose \u6e05\u7406\u5931\u8d25\uff0c\u8bf7\u4f7f\u7528\u4fdd\u7559\u7684 compose \u6587\u4ef6\u68c0\u67e5 project\uff1a{0}",
  "ComposeEscape": "\u62d2\u7edd\u5220\u9664\u8d8a\u8fc7 FILE-A \u4e13\u7528\u9a8c\u6536\u6839\u76ee\u5f55\u7684 compose \u6587\u4ef6\uff1a{0}",
  "CleanupFinal": "{0}\uff1b\u8bc1\u636e\u76ee\u5f55\uff1a{1}",
  "Success": "FILE-A \u771f\u5b9e\u95ed\u73af\u9a8c\u6536\u901a\u8fc7\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{0}"
}
'@

$script:RepositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$mysqlPassword = "ryframe_test_password"
$rustfsAccessKey = "ryframe-test-access"
$rustfsSecretKey = "ryframe-test-secret-2026"
$applyConfirmation = "APPLY-FILE-A-MAINTENANCE"
$script:AcceptanceLogPath = $null

function Assert-RequiredTool {
    param([Parameter(Mandatory)][string]$Name)

    if ($null -eq (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw ($script:FileAMessages.MissingTool -f $Name)
    }
}

function Get-FreeLoopbackPort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function ConvertTo-WindowsCommandLineArgument {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$Value
    )

    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') {
        return $Value
    }

    $builder = [Text.StringBuilder]::new()
    [void]$builder.Append([char]34)
    $backslashCount = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq [char]92) {
            $backslashCount += 1
            continue
        }
        if ($character -eq [char]34) {
            for ($index = 0; $index -lt (($backslashCount * 2) + 1); $index++) {
                [void]$builder.Append([char]92)
            }
            [void]$builder.Append([char]34)
            $backslashCount = 0
            continue
        }
        for ($index = 0; $index -lt $backslashCount; $index++) {
            [void]$builder.Append([char]92)
        }
        $backslashCount = 0
        [void]$builder.Append($character)
    }
    for ($index = 0; $index -lt ($backslashCount * 2); $index++) {
        [void]$builder.Append([char]92)
    }
    [void]$builder.Append([char]34)
    return $builder.ToString()
}

function Set-NativeProcessArguments {
    param(
        [Parameter(Mandatory)]
        [Diagnostics.ProcessStartInfo]$StartInfo,

        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    if ($null -ne $StartInfo.PSObject.Properties["ArgumentList"]) {
        foreach ($argument in $Arguments) {
            [void]$StartInfo.ArgumentList.Add($argument)
        }
        return
    }

    $StartInfo.Arguments = ($Arguments | ForEach-Object {
        ConvertTo-WindowsCommandLineArgument -Value $_
    }) -join " "
}

function Invoke-NativeProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter(Mandatory)][string[]]$ArgumentList,
        [hashtable]$Environment = @{},
        [string[]]$RemoveEnvironment = @(),
        [AllowNull()][string]$StandardInput = $null,
        [switch]$AllowFailure
    )

    $utf8NoBom = [Text.UTF8Encoding]::new($false)
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $script:RepositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.StandardOutputEncoding = $utf8NoBom
    $startInfo.StandardErrorEncoding = $utf8NoBom
    Set-NativeProcessArguments -StartInfo $startInfo -Arguments $ArgumentList

    foreach ($name in $RemoveEnvironment) {
        [void]$startInfo.EnvironmentVariables.Remove($name)
    }
    foreach ($entry in $Environment.GetEnumerator()) {
        $startInfo.EnvironmentVariables[[string]$entry.Key] = [string]$entry.Value
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw ($script:FileAMessages.CommandFailed -f -1, $FilePath, "进程启动返回 false")
        }
        $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
        $standardErrorTask = $process.StandardError.ReadToEndAsync()

        if ($null -ne $StandardInput) {
            $inputBytes = $utf8NoBom.GetBytes($StandardInput)
            $standardInputStream = $process.StandardInput.BaseStream
            $standardInputStream.Write($inputBytes, 0, $inputBytes.Length)
            $standardInputStream.Flush()
            $standardInputStream.Close()
        }
        else {
            $process.StandardInput.Close()
        }
        $process.WaitForExit()

        $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
        $standardError = $standardErrorTask.GetAwaiter().GetResult()
        $exitCode = $process.ExitCode
        $combined = $standardOutput
        if (-not [string]::IsNullOrEmpty($standardError)) {
            if (-not [string]::IsNullOrEmpty($combined) -and
                -not $combined.EndsWith("`n", [StringComparison]::Ordinal)) {
                $combined += [Environment]::NewLine
            }
            $combined += $standardError
        }

        if ($null -ne $script:AcceptanceLogPath) {
            $logRecord = "[{0}] command={1} exit_code={2}`n[stdout]`n{3}`n[stderr]`n{4}`n" -f `
                [DateTimeOffset]::Now.ToString("O"), $FilePath, $exitCode, `
                $standardOutput, $standardError
            [IO.File]::AppendAllText(
                $script:AcceptanceLogPath,
                $logRecord,
                $utf8NoBom
            )
        }
        if (-not [string]::IsNullOrWhiteSpace($standardOutput)) {
            Write-Host $standardOutput.TrimEnd()
        }
        if (-not [string]::IsNullOrWhiteSpace($standardError)) {
            $stderrColor = if ($exitCode -eq 0) { "DarkYellow" } else { "Red" }
            Write-Host $standardError.TrimEnd() -ForegroundColor $stderrColor
        }
        if ($exitCode -ne 0 -and -not $AllowFailure) {
            throw ($script:FileAMessages.CommandFailed -f $exitCode, $FilePath, $combined)
        }
        return [PSCustomObject]@{
            ExitCode = $exitCode
            Output = $combined
            StandardOutput = $standardOutput
            StandardError = $standardError
        }
    }
    finally {
        $process.Dispose()
    }
}

function Assert-OutputContains {
    param(
        [Parameter(Mandatory)][string]$Output,
        [Parameter(Mandatory)][string]$Expected,
        [Parameter(Mandatory)][string]$Context
    )

    if ($Output.IndexOf($Expected, [StringComparison]::Ordinal) -lt 0) {
        throw ($script:FileAMessages.OutputMissing -f $Context, $Expected)
    }
}

function Assert-OutputMatches {
    param(
        [Parameter(Mandatory)][string]$Output,
        [Parameter(Mandatory)][string]$Pattern,
        [Parameter(Mandatory)][string]$Context
    )

    if ($Output -notmatch $Pattern) {
        throw ($script:FileAMessages.OutputMismatch -f $Context, $Pattern)
    }
}

function Write-AcceptanceStep {
    param([Parameter(Mandatory)][string]$Message)

    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Get-LocalDockerContext {
    $contextResult = Invoke-NativeProcess -FilePath "docker" -ArgumentList @(
        "context", "show"
    )
    $contextName = ($contextResult.Output -split "`r?`n" |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -First 1).Trim()
    if ([string]::IsNullOrWhiteSpace($contextName)) {
        throw $script:FileAMessages.DockerContextEmpty
    }
    $endpointResult = Invoke-NativeProcess -FilePath "docker" -ArgumentList @(
        "context", "inspect", "--format", "{{ .Endpoints.docker.Host }}", $contextName
    )
    $endpoint = ($endpointResult.Output -split "`r?`n" |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -First 1).Trim()
    if ($endpoint -notmatch '^(npipe|unix)://') {
        throw ($script:FileAMessages.DockerRemote -f $contextName, $endpoint)
    }
    return [PSCustomObject]@{
        Name = $contextName
        Endpoint = $endpoint
    }
}

function Invoke-MigrationExpectingBlock {
    param(
        [Parameter(Mandatory)][string]$ExpectedHint,
        [Parameter(Mandatory)][string]$MigrateBinary,
        [Parameter(Mandatory)][hashtable]$ProcessEnvironment,
        [Parameter(Mandatory)][string[]]$RemovedEnvironment
    )

    $result = Invoke-NativeProcess -FilePath $MigrateBinary -ArgumentList @("up") `
        -Environment $ProcessEnvironment -RemoveEnvironment $RemovedEnvironment -AllowFailure
    if ($result.ExitCode -eq 0) {
        throw ($script:FileAMessages.MigrationUnexpected -f $ExpectedHint)
    }
    Assert-OutputContains -Output $result.Output -Expected $ExpectedHint `
        -Context $script:FileAMessages.MigrationGuardContext
}

function Invoke-Maintenance {
    param(
        [Parameter(Mandatory)][ValidateSet("backfill-sha256", "drain-legacy-reservations")]
        [string]$Command,
        [Parameter(Mandatory)][ValidateSet("dry-run", "apply")]
        [string]$Mode,
        [Parameter(Mandatory)][string]$DatabaseName,
        [Parameter(Mandatory)][string]$MaintenanceBinary,
        [Parameter(Mandatory)][hashtable]$ProcessEnvironment,
        [Parameter(Mandatory)][string[]]$RemovedEnvironment
    )

    $commandArguments = @(
        $Command, $Mode, "--database", $DatabaseName, "--batch-size", "10"
    )
    if ($Mode -eq "apply") {
        $commandArguments += @("--confirm-apply", $applyConfirmation)
    }
    return Invoke-NativeProcess -FilePath $MaintenanceBinary -ArgumentList $commandArguments `
        -Environment $ProcessEnvironment -RemoveEnvironment $RemovedEnvironment
}

function Invoke-AcceptanceTest {
    param(
        [Parameter(Mandatory)][ValidateSet("seed_file_a_legacy_fixture", "assert_file_a_final_state")]
        [string]$Name,
        [Parameter(Mandatory)][hashtable]$ProcessEnvironment,
        [Parameter(Mandatory)][string[]]$RemovedEnvironment
    )

    return Invoke-NativeProcess -FilePath "cargo" -ArgumentList @(
        "test", "--quiet", "-p", "ryframe", "--features", "file-maintenance",
        "--test", "file_a_acceptance_test", $Name, "--", "--ignored", "--exact", "--nocapture"
    ) -Environment $ProcessEnvironment -RemoveEnvironment $RemovedEnvironment
}

if ($PSVersionTable.PSVersion -lt [Version]"5.1") {
    throw $script:FileAMessages.Version
}
Assert-RequiredTool -Name "docker"
Assert-RequiredTool -Name "cargo"

$runId = [Guid]::NewGuid().ToString("N").Substring(0, 12).ToLowerInvariant()
$projectName = "ryframe-file-a-$runId"
$databaseName = "ryframe_file_a_$runId"
$bucketName = "ryframe-file-a-$runId"
if ($projectName -notmatch '^ryframe-file-a-[a-f0-9]{12}$' -or
    $databaseName -notmatch '^ryframe_file_a_[a-f0-9]{12}$' -or
    $bucketName -notmatch '^ryframe-file-a-[a-f0-9]{12}$') {
    throw $script:FileAMessages.NameValidation
}

$mysqlPort = Get-FreeLoopbackPort
do {
    $rustfsPort = Get-FreeLoopbackPort
} while ($rustfsPort -eq $mysqlPort)

$acceptanceRoot = [IO.Path]::GetFullPath((Join-Path $script:RepositoryRoot "target/file-a-acceptance"))
[void][IO.Directory]::CreateDirectory($acceptanceRoot)
$runRoot = [IO.Path]::GetFullPath((Join-Path $acceptanceRoot $runId))
$requiredPrefix = $acceptanceRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $runRoot.StartsWith($requiredPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw $script:FileAMessages.RunRootEscape
}
[void][IO.Directory]::CreateDirectory($runRoot)
$composeFile = Join-Path $runRoot "compose.yml"
$fixturePath = Join-Path $script:RepositoryRoot "crates/ryframe-db-migration/tests/fixtures/v0_4_2_mysql.sql"
if (-not [IO.File]::Exists($fixturePath)) {
    throw ($script:FileAMessages.MissingFixture -f $fixturePath)
}

$composeTemplate = @'
services:
  mysql:
    image: mysql:8.4
    environment:
      MYSQL_ROOT_PASSWORD: __MYSQL_PASSWORD__
      MYSQL_DATABASE: __DATABASE_NAME__
      MYSQL_INITDB_SKIP_TZINFO: "1"
    ports:
      - "127.0.0.1:__MYSQL_PORT__:3306"
    command:
      - --character-set-server=utf8mb4
      - --collation-server=utf8mb4_general_ci
      - --log-error-verbosity=1
    healthcheck:
      test: ["CMD-SHELL", "mysqladmin ping -h 127.0.0.1 -uroot -p$$MYSQL_ROOT_PASSWORD --silent"]
      interval: 2s
      timeout: 3s
      retries: 40
    tmpfs:
      - /var/lib/mysql
  rustfs:
    image: rustfs/rustfs:1.0.0-beta.8
    environment:
      RUSTFS_ACCESS_KEY: __RUSTFS_ACCESS_KEY__
      RUSTFS_SECRET_KEY: __RUSTFS_SECRET_KEY__
    ports:
      - "127.0.0.1:__RUSTFS_PORT__:9000"
    tmpfs:
      - /data:uid=10001,gid=10001,mode=0750
    healthcheck:
      test: ["CMD-SHELL", "curl --fail --silent http://127.0.0.1:9000/health >/dev/null"]
      interval: 2s
      timeout: 3s
      retries: 40
      start_period: 3s
'@
$compose = $composeTemplate.Replace("__MYSQL_PASSWORD__", $mysqlPassword).
    Replace("__DATABASE_NAME__", $databaseName).
    Replace("__MYSQL_PORT__", [string]$mysqlPort).
    Replace("__RUSTFS_ACCESS_KEY__", $rustfsAccessKey).
    Replace("__RUSTFS_SECRET_KEY__", $rustfsSecretKey).
    Replace("__RUSTFS_PORT__", [string]$rustfsPort)
[IO.File]::WriteAllText($composeFile, $compose, [Text.UTF8Encoding]::new($false))

$processEnvironment = @{
    APP_ENV = "test"
    APP_CONFIG_DIR = (Join-Path $script:RepositoryRoot "config")
    APP_DATABASE_HOST = "127.0.0.1"
    APP_DATABASE_PORT = [string]$mysqlPort
    APP_DATABASE_NAME = $databaseName
    APP_DATABASE_USERNAME = "root"
    APP_DATABASE_PASSWORD = $mysqlPassword
    APP_DATABASE_MAX_CONNECTIONS = "4"
    APP_DATABASE_MIN_CONNECTIONS = "1"
    APP_DATABASE_TLS_MODE = "disabled"
    APP_DATABASE_MIGRATION_MODE = "off"
    APP_OBJECT_STORAGE_BACKEND = "rustfs"
    APP_OBJECT_STORAGE_ENDPOINT = "http://127.0.0.1:$rustfsPort"
    APP_OBJECT_STORAGE_ACCESS_KEY = $rustfsAccessKey
    APP_OBJECT_STORAGE_SECRET_KEY = $rustfsSecretKey
    APP_OBJECT_STORAGE_USE_SSL = "false"
    APP_OBJECT_STORAGE_REGION = "us-east-1"
    FILE_A_DATABASE_NAME = $databaseName
    FILE_A_MYSQL_PORT = [string]$mysqlPort
    FILE_A_MYSQL_PASSWORD = $mysqlPassword
    FILE_A_RUSTFS_PORT = [string]$rustfsPort
    FILE_A_RUSTFS_ACCESS_KEY = $rustfsAccessKey
    FILE_A_RUSTFS_SECRET_KEY = $rustfsSecretKey
    FILE_A_BUCKET = $bucketName
    CARGO_TERM_COLOR = "never"
    CARGO_TARGET_DIR = (Join-Path $script:RepositoryRoot "target")
}
$removedEnvironment = @(
    "APP_DATABASE_REPLICAS",
    "APP_DATABASE_SOURCES",
    "APP_GENERATOR_DATA_SOURCE"
)
$binarySuffix = if ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT) {
    ".exe"
}
else {
    ""
}
$migrateBinary = Join-Path $script:RepositoryRoot "target/debug/ryframe-migrate$binarySuffix"
$maintenanceBinary = Join-Path $script:RepositoryRoot "target/debug/ryframe-file-maintenance$binarySuffix"
$script:AcceptanceLogPath = Join-Path $runRoot "acceptance.log"
$metadataPath = Join-Path $runRoot "run.json"
$dockerContextName = $null
$dockerEndpoint = $null
$composeOwned = $false
$runSucceeded = $false
$runError = $null
$cleanupSucceeded = $true
$cleanupError = $null
$metadata = [ordered]@{
    run_id = $runId
    docker_project = $projectName
    docker_context = $null
    docker_endpoint = $null
    database = $databaseName
    bucket = $bucketName
    mysql_port = $mysqlPort
    rustfs_port = $rustfsPort
    status = "starting"
    started_at = [DateTimeOffset]::Now.ToString("O")
    completed_at = $null
    error = $null
}

try {
    Write-AcceptanceStep -Message $script:FileAMessages.StepDockerContext
    $dockerContext = Get-LocalDockerContext
    $dockerContextName = $dockerContext.Name
    $dockerEndpoint = $dockerContext.Endpoint
    $metadata["docker_context"] = $dockerContextName
    $metadata["docker_endpoint"] = $dockerEndpoint
    $metadata["status"] = "running"
    $metadata | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $metadataPath -Encoding UTF8

    Write-AcceptanceStep -Message $script:FileAMessages.StepBuild
    [void](Invoke-NativeProcess -FilePath "cargo" -ArgumentList @(
        "build", "--quiet", "--locked", "-p", "ryframe", "--features", "file-maintenance",
        "--bin", "ryframe-migrate", "--bin", "ryframe-file-maintenance"
    ) -Environment $processEnvironment -RemoveEnvironment $removedEnvironment)
    foreach ($binary in @($migrateBinary, $maintenanceBinary)) {
        if (-not [IO.File]::Exists($binary)) {
            throw ($script:FileAMessages.MissingBinary -f $binary)
        }
    }
    $migrateBinary = (Resolve-Path -LiteralPath $migrateBinary).Path
    $maintenanceBinary = (Resolve-Path -LiteralPath $maintenanceBinary).Path

    Write-AcceptanceStep -Message $script:FileAMessages.StepStart
    $composeOwned = $true
    [void](Invoke-NativeProcess -FilePath "docker" -ArgumentList @(
        "--context", $dockerContextName, "compose", "--project-name", $projectName, "--file", $composeFile,
        "up", "--detach", "--wait", "mysql", "rustfs"
    ))

    Write-AcceptanceStep -Message $script:FileAMessages.StepImport
    $legacySql = [IO.File]::ReadAllText($fixturePath, [Text.Encoding]::UTF8)
    [void](Invoke-NativeProcess -FilePath "docker" -ArgumentList @(
        "--context", $dockerContextName, "compose", "--project-name", $projectName, "--file", $composeFile,
        "exec", "--no-TTY", "mysql", "mysql", "--protocol=TCP", "--host=127.0.0.1",
        "--user=root", "--password=$mysqlPassword", $databaseName
    ) -StandardInput $legacySql)

    Write-AcceptanceStep -Message $script:FileAMessages.StepSeed
    $seed = Invoke-AcceptanceTest -Name "seed_file_a_legacy_fixture" `
        -ProcessEnvironment $processEnvironment -RemovedEnvironment $removedEnvironment
    Assert-OutputContains -Output $seed.StandardOutput -Expected $script:FileAMessages.SeedExpected `
        -Context $script:FileAMessages.SeedContext

    Write-AcceptanceStep -Message $script:FileAMessages.StepBackfillGuard
    Invoke-MigrationExpectingBlock -ExpectedHint "backfill-sha256" `
        -MigrateBinary $migrateBinary `
        -ProcessEnvironment $processEnvironment -RemovedEnvironment $removedEnvironment

    Write-AcceptanceStep -Message $script:FileAMessages.StepBackfillDry
    $backfillDryRun = Invoke-Maintenance -Command "backfill-sha256" -Mode "dry-run" `
        -DatabaseName $databaseName -MaintenanceBinary $maintenanceBinary `
        -ProcessEnvironment $processEnvironment `
        -RemovedEnvironment $removedEnvironment
    Assert-OutputMatches -Output $backfillDryRun.Output `
        -Pattern 'SHA-256 backfill summary:.*remaining=2(?:\D|$)' `
        -Context $script:FileAMessages.BackfillDryContext

    Write-AcceptanceStep -Message $script:FileAMessages.StepBackfillApply
    $backfillApply = Invoke-Maintenance -Command "backfill-sha256" -Mode "apply" `
        -DatabaseName $databaseName -MaintenanceBinary $maintenanceBinary `
        -ProcessEnvironment $processEnvironment `
        -RemovedEnvironment $removedEnvironment
    Assert-OutputMatches -Output $backfillApply.Output `
        -Pattern 'SHA-256 backfill summary:.*updated=2.*remaining=0(?:\D|$)' `
        -Context $script:FileAMessages.BackfillApplyContext

    Write-AcceptanceStep -Message $script:FileAMessages.StepDrainGuard
    Invoke-MigrationExpectingBlock -ExpectedHint "drain-legacy-reservations" `
        -MigrateBinary $migrateBinary `
        -ProcessEnvironment $processEnvironment -RemovedEnvironment $removedEnvironment

    Write-AcceptanceStep -Message $script:FileAMessages.StepDrainDry
    $drainDryRun = Invoke-Maintenance -Command "drain-legacy-reservations" -Mode "dry-run" `
        -DatabaseName $databaseName -MaintenanceBinary $maintenanceBinary `
        -ProcessEnvironment $processEnvironment `
        -RemovedEnvironment $removedEnvironment
    Assert-OutputMatches -Output $drainDryRun.Output `
        -Pattern 'reservation drain summary:.*remaining=2(?:\D|$)' `
        -Context $script:FileAMessages.DrainDryContext

    Write-AcceptanceStep -Message $script:FileAMessages.StepDrainApply
    $drainApply = Invoke-Maintenance -Command "drain-legacy-reservations" -Mode "apply" `
        -DatabaseName $databaseName -MaintenanceBinary $maintenanceBinary `
        -ProcessEnvironment $processEnvironment `
        -RemovedEnvironment $removedEnvironment
    Assert-OutputMatches -Output $drainApply.Output `
        -Pattern 'reservation drain summary:.*normalized_ready=2.*remaining=0(?:\D|$)' `
        -Context $script:FileAMessages.DrainApplyContext

    Write-AcceptanceStep -Message $script:FileAMessages.StepFinalMigration
    $migrationUp = Invoke-NativeProcess -FilePath $migrateBinary -ArgumentList @("up") `
        -Environment $processEnvironment -RemoveEnvironment $removedEnvironment
    Assert-OutputContains -Output $migrationUp.Output `
        -Expected "migration completed and schema verified" `
        -Context $script:FileAMessages.MigrationUpContext
    $migrationStatus = Invoke-NativeProcess -FilePath $migrateBinary -ArgumentList @("status") `
        -Environment $processEnvironment -RemoveEnvironment $removedEnvironment
    Assert-OutputMatches -Output $migrationStatus.Output `
        -Pattern 'applied=(\d+) expected=\1 up_to_date=true' `
        -Context $script:FileAMessages.MigrationStatusContext
    $migrationVerify = Invoke-NativeProcess -FilePath $migrateBinary -ArgumentList @("verify") `
        -Environment $processEnvironment -RemoveEnvironment $removedEnvironment
    Assert-OutputContains -Output $migrationVerify.Output `
        -Expected "migration ledger and schema are current" `
        -Context $script:FileAMessages.MigrationVerifyContext

    Write-AcceptanceStep -Message $script:FileAMessages.StepFinalAssert
    $finalAssertion = Invoke-AcceptanceTest -Name "assert_file_a_final_state" `
        -ProcessEnvironment $processEnvironment -RemovedEnvironment $removedEnvironment
    Assert-OutputContains -Output $finalAssertion.StandardOutput `
        -Expected $script:FileAMessages.FinalExpected -Context $script:FileAMessages.FinalContext

    $runSucceeded = $true
}
catch {
    $runError = $_.Exception.Message
    throw
}
finally {
    if ($composeOwned -and [IO.File]::Exists($composeFile)) {
        $cleanup = Invoke-NativeProcess -FilePath "docker" -ArgumentList @(
            "--context", $dockerContextName, "compose", "--project-name", $projectName, "--file", $composeFile,
            "down", "--volumes", "--remove-orphans"
        ) -AllowFailure
        if ($cleanup.ExitCode -ne 0) {
            $cleanupSucceeded = $false
            $cleanupError = $script:FileAMessages.CleanupFailed -f $projectName
            Write-Warning $cleanupError
        }
    }
    if ($cleanupSucceeded -and [IO.File]::Exists($composeFile)) {
        $resolvedComposeFile = [IO.Path]::GetFullPath($composeFile)
        if (-not $resolvedComposeFile.StartsWith($requiredPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            $cleanupSucceeded = $false
            $cleanupError = $script:FileAMessages.ComposeEscape -f $resolvedComposeFile
            Write-Warning $cleanupError
        }
        else {
            Remove-Item -LiteralPath $resolvedComposeFile -Force
        }
    }
    $metadata["completed_at"] = [DateTimeOffset]::Now.ToString("O")
    $metadata["error"] = if ($null -ne $runError) { $runError } else { $cleanupError }
    $metadata["status"] = if (-not $cleanupSucceeded) {
        "cleanup_failed"
    }
    elseif ($runSucceeded) {
        "passed"
    }
    else {
        "failed"
    }
    $metadata | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $metadataPath -Encoding UTF8
}

if (-not $cleanupSucceeded) {
    throw ($script:FileAMessages.CleanupFinal -f $cleanupError, $runRoot)
}
Write-Host ("`n" + ($script:FileAMessages.Success -f $runRoot)) -ForegroundColor Green
