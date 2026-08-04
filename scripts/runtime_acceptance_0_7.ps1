[CmdletBinding()]
param(
    [string]$ConfirmRun = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:RyFrameV07Messages = ConvertFrom-Json @'
{
  "OptIn": "\u5fc5\u987b\u663e\u5f0f\u4f20\u5165 -ConfirmRun RUN-RYFRAME-V0-7-ACCEPTANCE \u624d\u80fd\u542f\u52a8 v0.7 \u8fd0\u884c\u9a8c\u6536",
  "PowerShellVersion": "v0.7 \u8fd0\u884c\u9a8c\u6536\u9700\u8981 PowerShell 5.1 \u6216\u66f4\u9ad8\u7248\u672c",
  "ScriptLocation": "v0.7 \u8fd0\u884c\u9a8c\u6536\u5165\u53e3\u5fc5\u987b\u4f4d\u4e8e\u4ed3\u5e93 scripts \u76ee\u5f55",
  "MissingSupport": "\u627e\u4e0d\u5230 v0.7 \u8fd0\u884c\u9a8c\u6536\u652f\u6301\u811a\u672c\uff1a{0}",
  "MissingStage": "v0.7 \u8fd0\u884c\u9a8c\u6536\u5b50\u9636\u6bb5\u5c1a\u672a\u5b9e\u73b0\uff0c\u62d2\u7edd\u4f2a\u9020\u901a\u8fc7\uff1a{0}",
  "ProjectName": "\u751f\u6210\u7684 v0.7 Docker project \u540d\u79f0\u4e0d\u7b26\u5408\u9694\u79bb\u89c4\u5219\uff1a{0}",
  "RunDirectoryEscape": "v0.7 \u9a8c\u6536\u8bc1\u636e\u76ee\u5f55\u8d8a\u51fa\u9650\u5b9a\u8303\u56f4\uff1a{0}",
  "RunDirectoryExists": "v0.7 \u9a8c\u6536\u8bc1\u636e\u76ee\u5f55\u5df2\u5b58\u5728\uff0c\u62d2\u7edd\u590d\u7528\uff1a{0}",
  "MissingCommand": "v0.7 \u8fd0\u884c\u9a8c\u6536\u7f3a\u5c11\u547d\u4ee4\uff1a{0}",
  "GitCommand": "Git \u547d\u4ee4\u6267\u884c\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {0}\uff1a{1}",
  "GitCommit": "\u65e0\u6cd5\u89e3\u6790\u5f53\u524d\u540e\u7aef\u5b8c\u6574 Git \u63d0\u4ea4\u8eab\u4efd\uff1a{0}",
  "DirtyWorktree": "v0.7 \u6b63\u5f0f\u8fd0\u884c\u9a8c\u6536\u53ea\u5141\u8bb8\u4ece\u5e72\u51c0\u540e\u7aef Git \u63d0\u4ea4\u8fd0\u884c\uff1a{0}",
  "CargoLock": "\u627e\u4e0d\u5230 v0.7 \u9a8c\u6536\u6240\u9700\u7684 Cargo.lock\uff1a{0}",
  "BinaryEvidence": "v0.7 \u9a8c\u6536\u7f3a\u5c11\u5df2\u8fd0\u884c\u7684\u4e8c\u8fdb\u5236\uff1a{0}",
  "PowerShellPath": "\u65e0\u6cd5\u786e\u5b9a\u5f53\u524d PowerShell \u53ef\u6267\u884c\u6587\u4ef6\u8def\u5f84",
  "StageMessage": "\u6267\u884c\u901a\u7528\u6d88\u606f\u4e2d\u5fc3\u53cc\u5b9e\u4f8b\u9a8c\u6536",
  "StageReplica": "\u6267\u884c\u8bfb\u526f\u672c\u6458\u9664\u4e0e\u6062\u590d\u9a8c\u6536",
  "StageOtel": "\u6267\u884c OTel Collector \u94fe\u8def\u4e0e\u6545\u969c\u6062\u590d\u9a8c\u6536",
  "StageFailed": "\u5b50\u9636\u6bb5\u201c{0}\u201d\u6267\u884c\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {1}",
  "StageEvidenceMissing": "\u5b50\u9636\u6bb5\u201c{0}\u201d\u672a\u751f\u6210\u5fc5\u9700\u8bc1\u636e\uff1a{1}",
  "StageEvidenceInvalid": "\u5b50\u9636\u6bb5\u201c{0}\u201d\u8bc1\u636e\u672a\u5b8c\u6574\u901a\u8fc7\uff1a{1}",
  "SourceDrift": "v0.7 \u9a8c\u6536\u671f\u95f4\u6e90\u7801\u8eab\u4efd\u53d1\u751f\u53d8\u5316\uff1a{0}",
  "Success": "v0.7 \u8fd0\u884c\u9a8c\u6536\u5168\u90e8\u901a\u8fc7\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{0}",
  "CleanupFailed": "v0.7 \u8fd0\u884c\u9a8c\u6536\u6267\u884c\u5b8c\u6210\uff0c\u4f46\u6e05\u7406\u5931\u8d25\uff1a{0}\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{1}",
  "DockerCleanup": "Docker \u9694\u79bb project \u6e05\u7406\u5931\u8d25\uff1a{0}",
  "DirectoryRestore": "\u5de5\u4f5c\u76ee\u5f55\u6062\u590d\u5931\u8d25\uff1a{0}",
  "TranscriptCleanup": "\u9a8c\u6536\u65e5\u5fd7\u6536\u5c3e\u5931\u8d25\uff1a{0}",
  "MetadataWrite": "\u9a8c\u6536\u8bc1\u636e\u5199\u5165\u5931\u8d25\uff1a{0}"
}
'@

$requiredConfirmation = "RUN-RYFRAME-V0-7-ACCEPTANCE"
if ($ConfirmRun -cne $requiredConfirmation) {
    throw $script:RyFrameV07Messages.OptIn
}
if ($PSVersionTable.PSVersion -lt [version]"5.1") {
    throw $script:RyFrameV07Messages.PowerShellVersion
}

function Test-RyFrameV07SamePath {
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

function Invoke-RyFrameV07GitLines {
    param(
        [Parameter(Mandatory = $true)][string]$GitExecutable,
        [Parameter(Mandatory = $true)][string]$RepositoryRoot,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    $LASTEXITCODE = 0
    $output = & $GitExecutable -C $RepositoryRoot @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:RyFrameV07Messages.GitCommand -f $exitCode, (@($output) -join [Environment]::NewLine))
    }
    return @($output | ForEach-Object { [string]$_ })
}

function Get-RyFrameV07BinaryEvidence {
    param([Parameter(Mandatory = $true)][string]$RepositoryRoot)

    $suffix = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) { ".exe" } else { "" }
    $debugDirectory = Join-Path $RepositoryRoot "target/debug"
    $evidence = [ordered]@{}
    foreach ($name in @("ryframe", "ryframe-worker", "ryframe-db-reset", "ryframe-migrate")) {
        $path = Join-Path $debugDirectory "$name$suffix"
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw ($script:RyFrameV07Messages.BinaryEvidence -f $path)
        }
        $evidence[$name] = [ordered]@{
            path = [System.IO.Path]::GetFullPath($path)
            sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }
    return $evidence
}

function Assert-RyFrameV07SourceIdentity {
    param(
        [Parameter(Mandatory = $true)][string]$GitExecutable,
        [Parameter(Mandatory = $true)][string]$RepositoryRoot,
        [Parameter(Mandatory = $true)][string]$ExpectedCommit,
        [Parameter(Mandatory = $true)][string]$ExpectedCargoLockSha256
    )

    $actualCommit = (@(Invoke-RyFrameV07GitLines `
        -GitExecutable $GitExecutable `
        -RepositoryRoot $RepositoryRoot `
        -Arguments @("rev-parse", "--verify", "HEAD")) -join "").Trim()
    if ($actualCommit -cne $ExpectedCommit) {
        throw ($script:RyFrameV07Messages.SourceDrift -f "HEAD $actualCommit")
    }
    $status = @(Invoke-RyFrameV07GitLines `
        -GitExecutable $GitExecutable `
        -RepositoryRoot $RepositoryRoot `
        -Arguments @("status", "--porcelain=v1", "--untracked-files=all"))
    if ($status.Count -gt 0) {
        throw ($script:RyFrameV07Messages.SourceDrift -f ($status -join "; "))
    }
    $cargoLockPath = Join-Path $RepositoryRoot "Cargo.lock"
    if (-not (Test-Path -LiteralPath $cargoLockPath -PathType Leaf)) {
        throw ($script:RyFrameV07Messages.SourceDrift -f $cargoLockPath)
    }
    $actualLockSha256 = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualLockSha256 -cne $ExpectedCargoLockSha256) {
        throw ($script:RyFrameV07Messages.SourceDrift -f "Cargo.lock $actualLockSha256")
    }
}

function Resolve-RyFrameV07TerminalStatus {
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

function Invoke-RyFrameV07Stage {
    param(
        [Parameter(Mandatory = $true)][string]$PowerShellExecutable,
        [Parameter(Mandatory = $true)][string]$ScriptPath,
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$RunDirectory,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$DockerContext,
        [Parameter(Mandatory = $true)][string]$DockerHelperPath,
        [Parameter(Mandatory = $true)][string]$StageName,
        [Parameter(Mandatory = $true)][string]$EvidenceFile
    )

    $arguments = @("-NoLogo", "-NoProfile", "-NonInteractive")
    if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        $arguments += @("-ExecutionPolicy", "Bypass")
    }
    $arguments += @(
        "-File", $ScriptPath,
        "-ConfirmRun", "RUN-RYFRAME-V0-7-STAGE",
        "-ProjectName", $ProjectName,
        "-OwnershipToken", $OwnershipToken,
        "-RunDirectory", $RunDirectory,
        "-DockerExecutable", $DockerExecutable,
        "-DockerContext", $DockerContext,
        "-DockerHelperPath", $DockerHelperPath
    )

    & $PowerShellExecutable @arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:RyFrameV07Messages.StageFailed -f $StageName, $exitCode)
    }

    $evidencePath = Join-Path $RunDirectory $EvidenceFile
    if (-not (Test-Path -LiteralPath $evidencePath -PathType Leaf)) {
        throw ($script:RyFrameV07Messages.StageEvidenceMissing -f $StageName, $evidencePath)
    }
    try {
        $evidence = Get-Content -LiteralPath $evidencePath -Raw -Encoding utf8 |
            ConvertFrom-Json
    }
    catch {
        throw ($script:RyFrameV07Messages.StageEvidenceInvalid -f $StageName, $_.Exception.Message)
    }
    $evidenceValid = $evidence.stage -ceq $StageName `
        -and $evidence.status -ceq "passed" `
        -and $evidence.docker_project -ceq $ProjectName `
        -and $evidence.ownership_token -ceq $OwnershipToken `
        -and (Test-RyFrameV07SamePath -Actual $evidence.run_directory -Expected $RunDirectory) `
        -and -not [string]::IsNullOrWhiteSpace([string]$evidence.completed_at) `
        -and $null -eq $evidence.error `
        -and @($evidence.cleanup_errors).Count -eq 0
    if (-not $evidenceValid) {
        throw ($script:RyFrameV07Messages.StageEvidenceInvalid -f $StageName, (
            $evidence | ConvertTo-Json -Depth 6 -Compress
        ))
    }
}

$scriptFile = (Resolve-Path -LiteralPath $PSCommandPath).Path
$scriptsDirectory = Split-Path -Parent $scriptFile
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDirectory "..")).Path
$expectedScriptsDirectory = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "scripts"))
if (-not (Test-RyFrameV07SamePath -Actual $scriptsDirectory -Expected $expectedScriptsDirectory)) {
    throw $script:RyFrameV07Messages.ScriptLocation
}

$supportScriptPath = Join-Path $scriptsDirectory "runtime_acceptance_0_7_support.ps1"
if (-not (Test-Path -LiteralPath $supportScriptPath -PathType Leaf)) {
    throw ($script:RyFrameV07Messages.MissingSupport -f $supportScriptPath)
}
. $supportScriptPath

$stageDefinitions = @(
    [ordered]@{
        name = "message"
        description = $script:RyFrameV07Messages.StageMessage
        script_path = Join-Path $scriptsDirectory "runtime_acceptance_0_7_message.ps1"
        evidence_file = "message-run.json"
    },
    [ordered]@{
        name = "replica"
        description = $script:RyFrameV07Messages.StageReplica
        script_path = Join-Path $scriptsDirectory "runtime_acceptance_0_7_replica.ps1"
        evidence_file = "replica-run.json"
    },
    [ordered]@{
        name = "otel"
        description = $script:RyFrameV07Messages.StageOtel
        script_path = Join-Path $scriptsDirectory "runtime_acceptance_0_7_otel.ps1"
        evidence_file = "otel-run.json"
    }
)

$gitCommand = Get-Command git -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($null -eq $gitCommand -or [string]::IsNullOrWhiteSpace($gitCommand.Source)) {
    throw ($script:RyFrameV07Messages.MissingCommand -f "git")
}
$gitExecutable = $gitCommand.Source
$gitCommit = (@(Invoke-RyFrameV07GitLines `
    -GitExecutable $gitExecutable `
    -RepositoryRoot $repositoryRoot `
    -Arguments @("rev-parse", "--verify", "HEAD")) -join "").Trim()
if ($gitCommit -notmatch "^[0-9a-f]{40}$") {
    throw ($script:RyFrameV07Messages.GitCommit -f $gitCommit)
}
$worktreeStatus = @(Invoke-RyFrameV07GitLines `
    -GitExecutable $gitExecutable `
    -RepositoryRoot $repositoryRoot `
    -Arguments @("status", "--porcelain=v1", "--untracked-files=all"))
if ($worktreeStatus.Count -gt 0) {
    throw ($script:RyFrameV07Messages.DirtyWorktree -f ($worktreeStatus -join "; "))
}
$cargoLockPath = Join-Path $repositoryRoot "Cargo.lock"
if (-not (Test-Path -LiteralPath $cargoLockPath -PathType Leaf)) {
    throw ($script:RyFrameV07Messages.CargoLock -f $cargoLockPath)
}
$sourceEvidence = [ordered]@{
    git_commit = $gitCommit
    worktree_clean = $true
    cargo_lock_sha256 = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
}

$runId = "{0}-{1}-{2}" -f (
    Get-Date -Format "yyyyMMddHHmmss"
), $PID, ([guid]::NewGuid().ToString("N").Substring(0, 12))
$projectName = "ryframe-v07-$runId".ToLowerInvariant()
$ownershipToken = "ryframe-v07-owner-{0}" -f ([guid]::NewGuid().ToString("N"))
if ($projectName.Length -gt 63 -or $projectName -notmatch "^ryframe-v07-[a-z0-9-]+$") {
    throw ($script:RyFrameV07Messages.ProjectName -f $projectName)
}

$targetDirectory = Join-Path $repositoryRoot "target"
$runRoot = [System.IO.Path]::GetFullPath((Join-Path $targetDirectory "runtime-acceptance-0-7"))
$runDirectory = [System.IO.Path]::GetFullPath((Join-Path $runRoot $runId))
$runPrefix = $runRoot.TrimEnd(
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
if (-not $runDirectory.StartsWith($runPrefix, $pathComparison)) {
    throw ($script:RyFrameV07Messages.RunDirectoryEscape -f $runDirectory)
}
if (Test-Path -LiteralPath $runDirectory) {
    throw ($script:RyFrameV07Messages.RunDirectoryExists -f $runDirectory)
}
New-Item -ItemType Directory -Path $runDirectory | Out-Null

$stageMetadata = @()
foreach ($stage in $stageDefinitions) {
    $stageMetadata += [ordered]@{
        name = $stage.name
        description = $stage.description
        script_path = $stage.script_path
        evidence_file = $stage.evidence_file
        evidence_path = $null
        binaries = $null
        status = "not_run"
        started_at = $null
        completed_at = $null
        error = $null
    }
}

$metadataPath = Join-Path $runDirectory "run.json"
$transcriptPath = Join-Path $runDirectory "acceptance-transcript.log"
$metadata = [ordered]@{
    schema_version = 1
    suite = "v0.7"
    status = "starting"
    started_at = [DateTime]::UtcNow.ToString("o")
    completed_at = $null
    repository_root = $repositoryRoot
    run_directory = $runDirectory
    docker_project = $projectName
    ownership_token = $ownershipToken
    docker_context = $null
    docker_endpoint = $null
    docker_server_version = $null
    fault_injection = "docker_native"
    source = $sourceEvidence
    binaries = $null
    stages = $stageMetadata
    error = $null
    cleanup_errors = @()
}
Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

$runSucceeded = $false
$runError = $null
$cleanupErrors = New-Object System.Collections.Generic.List[string]
$dockerOwnershipAcquired = $false
$transcriptStarted = $false
$locationChanged = $false
$originalLocation = (Get-Location).Path
$dockerExecutable = $null
$dockerContext = $null

try {
    Start-Transcript -LiteralPath $transcriptPath -Force | Out-Null
    $transcriptStarted = $true

    foreach ($stage in $stageDefinitions) {
        if (-not (Test-Path -LiteralPath $stage.script_path -PathType Leaf)) {
            throw ($script:RyFrameV07Messages.MissingStage -f $stage.script_path)
        }
    }

    $dockerCommand = Get-Command docker -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $dockerCommand -or [string]::IsNullOrWhiteSpace($dockerCommand.Source)) {
        throw ($script:RyFrameV07Messages.MissingCommand -f "docker")
    }
    $dockerExecutable = $dockerCommand.Source
    $powerShellExecutable = (Get-Process -Id $PID -ErrorAction Stop).Path
    if ([string]::IsNullOrWhiteSpace($powerShellExecutable)) {
        throw $script:RyFrameV07Messages.PowerShellPath
    }

    $contextInfo = Get-RyFrameV07LocalDockerContext -DockerExecutable $dockerExecutable
    $dockerContext = $contextInfo.Name
    $dockerServerVersion = Get-RyFrameV07DockerServerVersion `
        -DockerExecutable $dockerExecutable `
        -Context $dockerContext
    $metadata["docker_context"] = $dockerContext
    $metadata["docker_endpoint"] = $contextInfo.Endpoint
    $metadata["docker_server_version"] = $dockerServerVersion
    $metadata["status"] = "running"
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    Assert-RyFrameV07ProjectName -ProjectName $projectName
    Assert-RyFrameV07OwnershipToken -OwnershipToken $ownershipToken
    Assert-RyFrameV07ProjectEmpty `
        -ProjectName $projectName `
        -DockerExecutable $dockerExecutable `
        -Context $dockerContext
    $dockerOwnershipAcquired = $true
    Set-Location -LiteralPath $repositoryRoot
    $locationChanged = $true

    for ($index = 0; $index -lt $stageDefinitions.Count; $index++) {
        $stage = $stageDefinitions[$index]
        $stageEvidenceDirectory = Join-Path $runDirectory $stage.name
        New-Item -ItemType Directory -Path $stageEvidenceDirectory | Out-Null
        $metadata["stages"][$index]["status"] = "running"
        $metadata["stages"][$index]["started_at"] = [DateTime]::UtcNow.ToString("o")
        Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
        Write-Host ("`n==> {0}" -f $stage.description)

        try {
            Invoke-RyFrameV07Stage `
                -PowerShellExecutable $powerShellExecutable `
                -ScriptPath $stage.script_path `
                -ProjectName $projectName `
                -OwnershipToken $ownershipToken `
                -RunDirectory $stageEvidenceDirectory `
                -DockerExecutable $dockerExecutable `
                -DockerContext $dockerContext `
                -DockerHelperPath $supportScriptPath `
                -StageName $stage.name `
                -EvidenceFile $stage.evidence_file
            Assert-RyFrameV07SourceIdentity `
                -GitExecutable $gitExecutable `
                -RepositoryRoot $repositoryRoot `
                -ExpectedCommit $sourceEvidence.git_commit `
                -ExpectedCargoLockSha256 $sourceEvidence.cargo_lock_sha256
            $metadata["stages"][$index]["evidence_path"] = Join-Path `
                $stageEvidenceDirectory $stage.evidence_file
            $metadata["stages"][$index]["binaries"] = Get-RyFrameV07BinaryEvidence `
                -RepositoryRoot $repositoryRoot
        }
        catch {
            $stageError = $_
            $metadata["stages"][$index]["status"] = "failed"
            $metadata["stages"][$index]["completed_at"] = [DateTime]::UtcNow.ToString("o")
            $metadata["stages"][$index]["error"] = $stageError.Exception.Message
            try {
                Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
            }
            catch {
                $cleanupErrors.Add(($script:RyFrameV07Messages.MetadataWrite -f $_.Exception.Message))
            }
            throw $stageError
        }

        $metadata["stages"][$index]["status"] = "passed"
        $metadata["stages"][$index]["completed_at"] = [DateTime]::UtcNow.ToString("o")
        Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
    }

    Assert-RyFrameV07SourceIdentity `
        -GitExecutable $gitExecutable `
        -RepositoryRoot $repositoryRoot `
        -ExpectedCommit $sourceEvidence.git_commit `
        -ExpectedCargoLockSha256 $sourceEvidence.cargo_lock_sha256
    $metadata["binaries"] = Get-RyFrameV07BinaryEvidence -RepositoryRoot $repositoryRoot
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
    $runSucceeded = $true
}
catch {
    $runError = $_
    $metadata["error"] = $_.Exception.Message
}
finally {
    if ($dockerOwnershipAcquired -and $null -ne $dockerExecutable -and $null -ne $dockerContext) {
        try {
            Remove-RyFrameV07DockerProjectResources `
                -ProjectName $projectName `
                -OwnershipToken $ownershipToken `
                -DockerExecutable $dockerExecutable `
                -Context $dockerContext
        }
        catch {
            $cleanupErrors.Add(($script:RyFrameV07Messages.DockerCleanup -f $_.Exception.Message))
        }
    }

    if ($locationChanged) {
        try {
            Set-Location -LiteralPath $originalLocation
        }
        catch {
            $cleanupErrors.Add(($script:RyFrameV07Messages.DirectoryRestore -f $_.Exception.Message))
        }
    }

    if ($transcriptStarted) {
        try {
            Stop-Transcript | Out-Null
        }
        catch {
            $cleanupErrors.Add(($script:RyFrameV07Messages.TranscriptCleanup -f $_.Exception.Message))
        }
    }

    $metadata["completed_at"] = [DateTime]::UtcNow.ToString("o")
    $metadata["cleanup_errors"] = @($cleanupErrors)
    $metadata["status"] = Resolve-RyFrameV07TerminalStatus `
        -RunSucceeded $runSucceeded `
        -HasRunError ($null -ne $runError) `
        -CleanupErrorCount $cleanupErrors.Count
    try {
        Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
    }
    catch {
        $metadataWriteError = $script:RyFrameV07Messages.MetadataWrite -f $_.Exception.Message
        if ($null -eq $runError) {
            $runError = [System.InvalidOperationException]::new($metadataWriteError)
        }
        else {
            $cleanupErrors.Add($metadataWriteError)
        }
    }
}

if ($null -ne $runError) {
    throw $runError
}
if ($cleanupErrors.Count -gt 0) {
    throw ($script:RyFrameV07Messages.CleanupFailed -f ($cleanupErrors -join "; "), $runDirectory)
}
Write-Host ("`n" + ($script:RyFrameV07Messages.Success -f $runDirectory))
