[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$isWindowsHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
if ($isWindowsHost -and (
    $PSVersionTable.PSEdition -ne "Desktop" -or
    $PSVersionTable.PSVersion.Major -ne 5 -or
    $PSVersionTable.PSVersion.Minor -ne 1
)) {
    throw "This regression test requires Windows PowerShell 5.1 on Windows"
}

$scriptsDirectory = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDirectory "..")).Path
$runtimeScript = Join-Path (Join-Path $repositoryRoot "scripts") "runtime_acceptance.ps1"
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $runtimeScript,
    [ref]$tokens,
    [ref]$parseErrors
)
if ($parseErrors.Count -gt 0) {
    throw "runtime_acceptance.ps1 contains PowerShell parse errors"
}

$writerFunctions = @(
    $ast.FindAll(
        {
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq "Write-RuntimeMetadataAtomically"
        },
        $true
    )
)
if ($writerFunctions.Count -ne 1) {
    throw "Expected exactly one Write-RuntimeMetadataAtomically function"
}

$writerDefinition = [scriptblock]::Create($writerFunctions[0].Extent.Text)
. $writerDefinition
$script:RuntimeMessages = ConvertFrom-Json @'
{
  "MetadataArtifactCleanup": "\u9a8c\u6536\u8bc1\u636e\u5df2\u63d0\u4ea4\uff0c\u4f46\u4e34\u65f6\u6587\u4ef6\u6e05\u7406\u5931\u8d25\uff1a{0}"
}
'@

function Assert-MetadataWrite {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedRunId,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedStatus,

        [Parameter(Mandatory = $true)]
        [int]$ExpectedSequence
    )

    $raw = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
    $metadata = $raw | ConvertFrom-Json
    if (
        $metadata.run_id -ne $ExpectedRunId -or
        $metadata.status -ne $ExpectedStatus -or
        $metadata.sequence -ne $ExpectedSequence
    ) {
        throw "run.json does not match write sequence $ExpectedSequence"
    }

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $hasBom = $bytes.Length -ge 3 -and
        $bytes[0] -eq 0xEF -and
        $bytes[1] -eq 0xBB -and
        $bytes[2] -eq 0xBF
    if ($hasBom -or $bytes.Length -eq 0 -or $bytes[$bytes.Length - 1] -ne 0x0A) {
        throw "run.json must be UTF-8 without BOM and end with a newline"
    }

    $directory = Split-Path -Parent $Path
    $files = @(Get-ChildItem -LiteralPath $directory -File)
    if ($files.Count -ne 1 -or $files[0].Name -ne "run.json") {
        $names = ($files | ForEach-Object { $_.Name }) -join ", "
        throw "Atomic metadata write left temporary or backup files: $names"
    }
}

$testDirectory = Join-Path (
    [System.IO.Path]::GetTempPath()
) ("ryframe-runtime-metadata-{0}-{1}" -f $PID, [guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($testDirectory) | Out-Null
$metadataPath = Join-Path $testDirectory "run.json"

try {
    $firstMetadata = [ordered]@{
        run_id = "first-write"
        status = "starting"
        sequence = 1
        details = [ordered]@{ payload = "first payload" }
    }
    Write-RuntimeMetadataAtomically -Metadata $firstMetadata -Path $metadataPath
    Assert-MetadataWrite `
        -Path $metadataPath `
        -ExpectedRunId "first-write" `
        -ExpectedStatus "starting" `
        -ExpectedSequence 1

    $secondMetadata = [ordered]@{
        run_id = "second-write"
        status = "passed"
        sequence = 2
        details = [ordered]@{ payload = "replacement payload" }
    }
    Write-RuntimeMetadataAtomically -Metadata $secondMetadata -Path $metadataPath
    Assert-MetadataWrite `
        -Path $metadataPath `
        -ExpectedRunId "second-write" `
        -ExpectedStatus "passed" `
        -ExpectedSequence 2

    $thirdMetadata = [ordered]@{
        run_id = "cleanup-failure"
        status = "passed"
        sequence = 3
        details = [ordered]@{ payload = "committed before cleanup failure" }
    }
    $artifactDeleter = {
        param([string]$ArtifactPath)
        if ($ArtifactPath.EndsWith(".bak", [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "injected backup cleanup failure"
        }
        [System.IO.File]::Delete($ArtifactPath)
    }
    $cleanupWarnings = @()
    Write-RuntimeMetadataAtomically `
        -Metadata $thirdMetadata `
        -Path $metadataPath `
        -ArtifactDeleter $artifactDeleter `
        -WarningVariable cleanupWarnings

    $committed = [System.IO.File]::ReadAllText(
        $metadataPath,
        [System.Text.Encoding]::UTF8
    ) | ConvertFrom-Json
    if (
        $committed.run_id -ne "cleanup-failure" -or
        $committed.status -ne "passed" -or
        $committed.sequence -ne 3
    ) {
        throw "run.json was not committed before the injected cleanup failure"
    }
    if ($cleanupWarnings.Count -ne 1) {
        throw "injected cleanup failure must produce exactly one warning"
    }
    $remainingArtifacts = @(
        Get-ChildItem -LiteralPath $testDirectory -File |
            Where-Object { $_.Name -ne "run.json" }
    )
    if (
        $remainingArtifacts.Count -ne 1 -or
        -not $remainingArtifacts[0].Name.EndsWith(
            ".bak",
            [System.StringComparison]::OrdinalIgnoreCase
        )
    ) {
        $names = ($remainingArtifacts | ForEach-Object { $_.Name }) -join ", "
        throw "injected cleanup failure left unexpected artifacts: $names"
    }
    [System.IO.File]::Delete($remainingArtifacts[0].FullName)
    Assert-MetadataWrite `
        -Path $metadataPath `
        -ExpectedRunId "cleanup-failure" `
        -ExpectedStatus "passed" `
        -ExpectedSequence 3

    Write-Output "runtime metadata writer self-test passed"
}
finally {
    if ([System.IO.Directory]::Exists($testDirectory)) {
        [System.IO.Directory]::Delete($testDirectory, $true)
    }
}
