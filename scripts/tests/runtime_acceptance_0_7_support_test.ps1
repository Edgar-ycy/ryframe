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
$supportScript = Join-Path $scriptsDirectory "runtime_acceptance_0_7_support.ps1"
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $supportScript,
    [ref]$tokens,
    [ref]$parseErrors
)
if ($parseErrors.Count -gt 0) {
    throw "runtime_acceptance_0_7_support.ps1 contains PowerShell parse errors"
}

foreach ($functionName in @(
    "ConvertTo-RyFrameV07ProcessArgument",
    "Invoke-RyFrameV07ProcessLines",
    "Write-RyFrameV07MetadataAtomically"
)) {
    $definitions = @(
        $ast.FindAll(
            {
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $functionName
            },
            $true
        )
    )
    if ($definitions.Count -ne 1) {
        throw "Expected exactly one $functionName function"
    }
    . ([scriptblock]::Create($definitions[0].Extent.Text))
}

$script:RyFrameV07SupportMessages = ConvertFrom-Json @'
{
  "CommandFailed": "\u539f\u751f\u547d\u4ee4\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {0}\uff1a{1}",
  "MetadataArtifactCleanup": "\u9a8c\u6536\u8bc1\u636e\u5df2\u63d0\u4ea4\uff0c\u4f46\u4e34\u65f6\u6587\u4ef6\u6e05\u7406\u5931\u8d25\uff1a{0}"
}
'@

$testDirectory = Join-Path (
    [System.IO.Path]::GetTempPath()
) ("ryframe v07 support {0} {1}" -f $PID, [guid]::NewGuid().ToString("N"))
[System.IO.Directory]::CreateDirectory($testDirectory) | Out-Null
$metadataPath = Join-Path $testDirectory "run metadata.json"
$echoScript = Join-Path $testDirectory "echo arguments.ps1"

try {
    $firstMetadata = [ordered]@{ status = "starting"; sequence = 1 }
    $secondMetadata = [ordered]@{ status = "passed"; sequence = 2 }
    Write-RyFrameV07MetadataAtomically -Metadata $firstMetadata -Path $metadataPath
    Write-RyFrameV07MetadataAtomically -Metadata $secondMetadata -Path $metadataPath

    $raw = [System.IO.File]::ReadAllText($metadataPath, [System.Text.Encoding]::UTF8)
    $metadata = $raw | ConvertFrom-Json
    $bytes = [System.IO.File]::ReadAllBytes($metadataPath)
    $hasBom = $bytes.Length -ge 3 -and
        $bytes[0] -eq 0xEF -and
        $bytes[1] -eq 0xBB -and
        $bytes[2] -eq 0xBF
    if (
        $metadata.status -ne "passed" -or
        $metadata.sequence -ne 2 -or
        $hasBom -or
        $bytes.Length -eq 0 -or
        $bytes[$bytes.Length - 1] -ne 0x0A
    ) {
        throw "Atomic v0.7 metadata replacement produced invalid output"
    }
    $artifacts = @(
        Get-ChildItem -LiteralPath $testDirectory -File |
            Where-Object { $_.Name -like "run metadata.json.*" }
    )
    if ($artifacts.Count -ne 0) {
        throw "Atomic v0.7 metadata replacement left temporary artifacts"
    }

    $echoSource = @'
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [AllowEmptyCollection()]
    [string[]]$Values
)
@($Values) | ConvertTo-Json -Compress
'@
    [System.IO.File]::WriteAllText(
        $echoScript,
        $echoSource,
        [System.Text.UTF8Encoding]::new($false)
    )
    $powershellExecutable = if ($isWindowsHost) {
        (Get-Command powershell.exe -ErrorAction Stop).Source
    }
    else {
        (Get-Command pwsh -ErrorAction Stop).Source
    }
    $expectedArguments = @(
        "plain",
        "contains spaces",
        'quote "inside"',
        "trailing\",
        ""
    )
    $lines = @(Invoke-RyFrameV07ProcessLines `
        -Executable $powershellExecutable `
        -Arguments (@(
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            $echoScript
        ) + $expectedArguments))
    if ($lines.Count -ne 1) {
        throw "Native process argument test produced an unexpected line count"
    }
    $parsedArguments = $lines[0] | ConvertFrom-Json
    $actualArguments = @()
    foreach ($value in $parsedArguments) {
        $actualArguments += [string]$value
    }
    if ($actualArguments.Count -ne $expectedArguments.Count) {
        throw (
            "Native process argument count changed: expected {0}, actual {1}, payload {2}" -f
                $expectedArguments.Count, $actualArguments.Count, $lines[0]
        )
    }
    for ($index = 0; $index -lt $expectedArguments.Count; $index++) {
        if ($actualArguments[$index] -cne $expectedArguments[$index]) {
            throw "Native process argument $index changed"
        }
    }

    Write-Output "v0.7 support self-test passed"
}
finally {
    if ([System.IO.Directory]::Exists($testDirectory)) {
        [System.IO.Directory]::Delete($testDirectory, $true)
    }
}
