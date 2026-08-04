Set-StrictMode -Version Latest

$script:RyFrameV07SupportMessages = ConvertFrom-Json @'
{
  "CommandFailed": "Docker \u547d\u4ee4\u6267\u884c\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {0}\uff1a{1}",
  "ProjectName": "\u9694\u79bb Docker project \u540d\u79f0\u4e0d\u5b89\u5168\uff1a{0}",
  "OwnershipToken": "Docker \u9a8c\u6536\u6240\u6709\u6743\u4ee4\u724c\u4e0d\u5b89\u5168\uff1a{0}",
  "ResourceName": "Docker \u8d44\u6e90\u540d\u79f0\u4e0d\u5b89\u5168\uff1a{0}",
  "ComposeMissing": "\u627e\u4e0d\u5230 Compose \u6587\u4ef6\uff1a{0}",
  "ResourceCount": "\u9694\u79bb project\u201c{0}\u201d\u4e2d\u7684\u8d44\u6e90\u201c{1}\u201d\u6570\u91cf\u5e94\u4e3a 1\uff0c\u5b9e\u9645\u4e3a {2}",
  "ProjectLabel": "Docker {0}\u201c{1}\u201d\u7684 project \u6807\u7b7e\u4e3a\u201c{2}\u201d\uff0c\u9884\u671f\u4e3a\u201c{3}\u201d",
  "OwnerLabel": "Docker {0}\u201c{1}\u201d\u7684\u6240\u6709\u6743\u6807\u7b7e\u4e3a\u201c{2}\u201d\uff0c\u9884\u671f\u4e3a\u201c{3}\u201d",
  "ProjectNotEmpty": "\u9694\u79bb Docker project\u201c{0}\u201d\u5df2\u5305\u542b\u8d44\u6e90\uff0c\u62d2\u7edd\u63a5\u7ba1\uff1a{1}",
  "ImageEvidence": "\u65e0\u6cd5\u89e3\u6790 Docker \u5bb9\u5668\u6216\u955c\u50cf\u8eab\u4efd\uff1a{0}",
  "ProcessInspect": "\u65e0\u6cd5\u68c0\u67e5\u9a8c\u6536\u8fdb\u7a0b PID {0}\uff1a{1}",
  "ProcessIdentity": "\u9a8c\u6536\u8fdb\u7a0b PID {0} \u7684\u542f\u52a8\u65f6\u95f4\u6216\u53ef\u6267\u884c\u6587\u4ef6\u5df2\u53d8\u5316\uff0c\u62d2\u7edd\u64cd\u4f5c",
  "ContextRead": "\u65e0\u6cd5\u8bfb\u53d6 Docker context\uff1a{0}",
  "ContextEmpty": "Docker context \u4e3a\u7a7a\uff0c\u62d2\u7edd\u7ee7\u7eed",
  "ContextInspect": "\u65e0\u6cd5\u68c0\u67e5 Docker context\u201c{0}\u201d\uff1a{1}",
  "ContextRemote": "Docker context\u201c{0}\u201d\u6307\u5411\u975e\u672c\u673a endpoint\u201c{1}\u201d\uff0c\u62d2\u7edd\u8fd0\u884c\u9a8c\u6536",
  "DaemonUnavailable": "Docker context\u201c{0}\u201d\u7684\u672c\u673a daemon \u4e0d\u53ef\u7528\uff1a{1}",
  "StopService": "\u505c\u6b62\u9694\u79bb project\u201c{0}\u201d\u7684\u670d\u52a1\u201c{1}\u201d",
  "StartService": "\u542f\u52a8\u9694\u79bb project\u201c{0}\u201d\u7684\u670d\u52a1\u201c{1}\u201d",
  "DisconnectNetwork": "\u65ad\u5f00\u670d\u52a1\u201c{0}\u201d\u4e0e\u7f51\u7edc\u201c{1}\u201d\u7684\u8fde\u63a5",
  "RestoreNetwork": "\u6062\u590d\u670d\u52a1\u201c{0}\u201d\u4e0e\u7f51\u7edc\u201c{1}\u201d\u7684\u8fde\u63a5",
  "StateMismatch": "\u670d\u52a1\u201c{0}\u201d\u7684\u8fd0\u884c\u72b6\u6001\u4e0e\u9884\u671f\u4e0d\u4e00\u81f4\uff1a{1}",
  "StoppedBeforeInjection": "\u5bb9\u5668\u5728\u6545\u969c\u6ce8\u5165\u524d\u5df2\u505c\u6b62",
  "StillRunning": "\u505c\u6b62\u540e\u4ecd\u5728\u8fd0\u884c",
  "StillStopped": "\u542f\u52a8\u540e\u4ecd\u672a\u8fd0\u884c",
  "StillConnected": "\u65ad\u7f51\u540e\u4ecd\u5728\u9694\u79bb\u7f51\u7edc\u4e2d",
  "StillDisconnected": "\u7f51\u7edc\u6062\u590d\u540e\u4ecd\u672a\u8fde\u63a5",
  "AlreadyDisconnected": "\u670d\u52a1\u201c{0}\u201d\u5df2\u4e0e\u7f51\u7edc\u201c{1}\u201d\u65ad\u5f00\uff0c\u65e0\u6cd5\u8bc1\u660e\u672c\u6b21\u6545\u969c\u6ce8\u5165",
  "FaultToken": "\u4e0d\u652f\u6301\u7684 Docker \u6545\u969c\u6062\u590d\u4ee4\u724c\uff1a{0}",
  "Cleanup": "\u6e05\u7406\u9694\u79bb Docker project\u201c{0}\u201d\u7684\u5bb9\u5668\u3001\u7f51\u7edc\u548c\u6570\u636e\u5377",
  "CleanupRemaining": "\u9694\u79bb Docker project\u201c{0}\u201d\u6e05\u7406\u540e\u4ecd\u6709\u8d44\u6e90\u6b8b\u7559\uff1a{1}",
  "MetadataCleanup": "\u9a8c\u6536\u8bc1\u636e\u5df2\u63d0\u4ea4\uff0c\u4f46\u4e34\u65f6\u6587\u4ef6\u6e05\u7406\u5931\u8d25\uff1a{0}"
}
'@

function Assert-RyFrameV07ProjectName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ProjectName
    )

    if ($ProjectName.Length -gt 63 -or $ProjectName -notmatch "^ryframe-v07-[a-z0-9-]+$") {
        throw ($script:RyFrameV07SupportMessages.ProjectName -f $ProjectName)
    }
}

function Assert-RyFrameV07OwnershipToken {
    param(
        [Parameter(Mandatory = $true)]
        [string]$OwnershipToken
    )

    if ($OwnershipToken -notmatch "^ryframe-v07-owner-[a-f0-9]{32}$") {
        throw ($script:RyFrameV07SupportMessages.OwnershipToken -f $OwnershipToken)
    }
}

function Assert-RyFrameV07ResourceName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if ($Name -notmatch "^[a-zA-Z0-9][a-zA-Z0-9_.-]*$") {
        throw ($script:RyFrameV07SupportMessages.ResourceName -f $Name)
    }
}

function ConvertTo-RyFrameV07ProcessArgument {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Value)

    if ($Value.Length -eq 0) {
        return '""'
    }
    if ($Value -notmatch '[\s"]') {
        return $Value
    }

    $builder = [System.Text.StringBuilder]::new()
    [void]$builder.Append([char]34)
    $backslashCount = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq [char]92) {
            $backslashCount++
            continue
        }
        if ($character -eq [char]34) {
            [void]$builder.Append([char]92, ($backslashCount * 2) + 1)
            [void]$builder.Append([char]34)
            $backslashCount = 0
            continue
        }
        if ($backslashCount -gt 0) {
            [void]$builder.Append([char]92, $backslashCount)
            $backslashCount = 0
        }
        [void]$builder.Append($character)
    }
    if ($backslashCount -gt 0) {
        [void]$builder.Append([char]92, $backslashCount * 2)
    }
    [void]$builder.Append([char]34)
    return $builder.ToString()
}

function Invoke-RyFrameV07ProcessLines {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Executable,

        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$Arguments
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.Arguments = (@($Arguments | ForEach-Object {
        ConvertTo-RyFrameV07ProcessArgument -Value $_
    }) -join " ")
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw [System.InvalidOperationException]::new("native process did not start")
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        $exitCode = $process.ExitCode
    }
    finally {
        $process.Dispose()
    }

    $allOutput = New-Object System.Collections.Generic.List[string]
    foreach ($text in @($stdout, $stderr)) {
        $reader = [System.IO.StringReader]::new($text)
        try {
            while (($line = $reader.ReadLine()) -ne $null) {
                $allOutput.Add($line)
            }
        }
        finally {
            $reader.Dispose()
        }
    }
    if ($exitCode -ne 0) {
        $detail = ($allOutput | ForEach-Object { [string]$_ }) -join [Environment]::NewLine
        throw ($script:RyFrameV07SupportMessages.CommandFailed -f $exitCode, $detail)
    }
    return @($allOutput | ForEach-Object { [string]$_ })
}

function Invoke-RyFrameV07DockerLines {
    param(
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Arguments
    )

    return @(Invoke-RyFrameV07ProcessLines `
        -Executable $DockerExecutable `
        -Arguments (@("--context", $Context) + @($Arguments)))
}

function Invoke-RyFrameV07DockerChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context,

        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    Write-Host ("`n==> {0}" -f $Description)
    $lines = @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments $Arguments)
    foreach ($line in $lines) {
        Write-Host $line
    }
}

function Get-RyFrameV07LocalDockerContext {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable
    )

    try {
        $contextOutput = @(Invoke-RyFrameV07ProcessLines `
            -Executable $DockerExecutable `
            -Arguments @("context", "show"))
    }
    catch {
        throw ($script:RyFrameV07SupportMessages.ContextRead -f $_.Exception.Message)
    }
    $context = ($contextOutput | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($context)) {
        throw $script:RyFrameV07SupportMessages.ContextEmpty
    }

    try {
        $endpointOutput = @(Invoke-RyFrameV07ProcessLines `
            -Executable $DockerExecutable `
            -Arguments @("context", "inspect", "--format", "{{ .Endpoints.docker.Host }}", $context))
    }
    catch {
        throw ($script:RyFrameV07SupportMessages.ContextInspect -f $context, $_.Exception.Message)
    }
    $endpoint = ($endpointOutput | Out-String).Trim()
    if ($endpoint -notmatch "^(npipe|unix)://") {
        throw ($script:RyFrameV07SupportMessages.ContextRemote -f $context, $endpoint)
    }

    return [pscustomobject]@{
        Name = $context
        Endpoint = $endpoint
    }
}

function Get-RyFrameV07DockerServerVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    try {
        $serverOutput = @(Invoke-RyFrameV07DockerLines `
            -DockerExecutable $DockerExecutable `
            -Context $Context `
            -Arguments @("info", "--format", "{{ .ServerVersion }}"))
    }
    catch {
        throw ($script:RyFrameV07SupportMessages.DaemonUnavailable -f $Context, $_.Exception.Message)
    }
    $serverVersion = ($serverOutput | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($serverVersion)) {
        throw ($script:RyFrameV07SupportMessages.DaemonUnavailable -f $Context, "empty server version")
    }
    return $serverVersion
}

function Assert-RyFrameV07ResourceProjectLabel {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("container", "network", "volume")]
        [string]$ResourceKind,

        [Parameter(Mandatory = $true)]
        [string]$Identifier,

        [Parameter(Mandatory = $true)]
        [string]$ProjectName,

        [Parameter(Mandatory = $true)]
        [string]$OwnershipToken,

        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    Assert-RyFrameV07ProjectName -ProjectName $ProjectName
    Assert-RyFrameV07OwnershipToken -OwnershipToken $OwnershipToken
    $inspectJson = (@(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @($ResourceKind, "inspect", $Identifier)) -join [Environment]::NewLine)
    try {
        $documents = @($inspectJson | ConvertFrom-Json)
    }
    catch {
        throw ($script:RyFrameV07SupportMessages.ProjectLabel -f $ResourceKind, $Identifier, "invalid-inspect-json", $ProjectName)
    }
    if ($documents.Count -ne 1) {
        throw ($script:RyFrameV07SupportMessages.ProjectLabel -f $ResourceKind, $Identifier, "inspect-count-$($documents.Count)", $ProjectName)
    }
    $labels = if ($ResourceKind -eq "container") {
        $documents[0].Config.Labels
    }
    else {
        $documents[0].Labels
    }
    $labelProperty = if ($null -eq $labels) {
        $null
    }
    else {
        $labels.PSObject.Properties["com.docker.compose.project"]
    }
    $label = if ($null -eq $labelProperty) { "" } else { [string]$labelProperty.Value }
    if ($label -cne $ProjectName) {
        throw ($script:RyFrameV07SupportMessages.ProjectLabel -f $ResourceKind, $Identifier, $label, $ProjectName)
    }
    $ownerProperty = if ($null -eq $labels) {
        $null
    }
    else {
        $labels.PSObject.Properties["com.ryframe.runtime-acceptance-owner"]
    }
    $owner = if ($null -eq $ownerProperty) { "" } else { [string]$ownerProperty.Value }
    if ($owner -cne $OwnershipToken) {
        throw ($script:RyFrameV07SupportMessages.OwnerLabel -f $ResourceKind, $Identifier, $owner, $OwnershipToken)
    }
}

function Get-RyFrameV07ProjectImageEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    $evidence = New-Object System.Collections.Generic.List[object]
    $containerIds = @(Get-RyFrameV07ProjectResourceIds `
        -ResourceKind "container" `
        -ProjectName $ProjectName `
        -DockerExecutable $DockerExecutable `
        -Context $Context)
    foreach ($containerId in $containerIds) {
        Assert-RyFrameV07ResourceProjectLabel `
            -ResourceKind "container" `
            -Identifier $containerId `
            -ProjectName $ProjectName `
            -OwnershipToken $OwnershipToken `
            -DockerExecutable $DockerExecutable `
            -Context $Context
        try {
            $containerDocuments = @((@(Invoke-RyFrameV07DockerLines `
                -DockerExecutable $DockerExecutable `
                -Context $Context `
                -Arguments @("container", "inspect", $containerId)) `
                -join [Environment]::NewLine) | ConvertFrom-Json)
        }
        catch {
            throw ($script:RyFrameV07SupportMessages.ImageEvidence -f $_.Exception.Message)
        }
        if ($containerDocuments.Count -ne 1) {
            throw ($script:RyFrameV07SupportMessages.ImageEvidence -f "container inspect count $($containerDocuments.Count)")
        }
        $container = $containerDocuments[0]
        $imageId = [string]$container.Image
        if ([string]::IsNullOrWhiteSpace($imageId)) {
            throw ($script:RyFrameV07SupportMessages.ImageEvidence -f "empty image id")
        }
        try {
            $imageDocuments = @((@(Invoke-RyFrameV07DockerLines `
                -DockerExecutable $DockerExecutable `
                -Context $Context `
                -Arguments @("image", "inspect", $imageId)) `
                -join [Environment]::NewLine) | ConvertFrom-Json)
        }
        catch {
            throw ($script:RyFrameV07SupportMessages.ImageEvidence -f $_.Exception.Message)
        }
        if ($imageDocuments.Count -ne 1) {
            throw ($script:RyFrameV07SupportMessages.ImageEvidence -f "image inspect count $($imageDocuments.Count)")
        }
        $containerLabels = $container.Config.Labels
        $serviceProperty = if ($null -eq $containerLabels) {
            $null
        }
        else {
            $containerLabels.PSObject.Properties["com.docker.compose.service"]
        }
        $service = if ($null -eq $serviceProperty) { "" } else { [string]$serviceProperty.Value }
        $evidence.Add([ordered]@{
            service = $service
            container_name = ([string]$container.Name).TrimStart("/")
            configured_image = [string]$container.Config.Image
            image_id = $imageId
            repo_digests = @($imageDocuments[0].RepoDigests | Sort-Object)
        })
    }
    return @($evidence | Sort-Object service, container_name)
}

function Get-RyFrameV07ProcessEnvironmentSnapshot {
    $comparer = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        [System.StringComparer]::OrdinalIgnoreCase
    }
    else {
        [System.StringComparer]::Ordinal
    }
    $snapshot = [System.Collections.Generic.Dictionary[string, string]]::new($comparer)
    foreach ($entry in [Environment]::GetEnvironmentVariables(
        [EnvironmentVariableTarget]::Process
    ).GetEnumerator()) {
        $snapshot[[string]$entry.Key] = [string]$entry.Value
    }
    return ,$snapshot
}

function Restore-RyFrameV07ProcessEnvironmentSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.Generic.IDictionary[string, string]]$Snapshot
    )

    $currentNames = @([Environment]::GetEnvironmentVariables(
        [EnvironmentVariableTarget]::Process
    ).Keys | ForEach-Object { [string]$_ })
    foreach ($name in $currentNames) {
        if (-not $Snapshot.ContainsKey($name)) {
            [Environment]::SetEnvironmentVariable(
                $name,
                $null,
                [EnvironmentVariableTarget]::Process
            )
        }
    }
    foreach ($entry in $Snapshot.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable(
            [string]$entry.Key,
            [string]$entry.Value,
            [EnvironmentVariableTarget]::Process
        )
    }
}

function Get-RyFrameV07OwnedProcess {
    param(
        [AllowNull()][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable
    )

    if ($null -eq $Process) {
        return $null
    }
    try {
        $Process.Refresh()
        if ($Process.HasExited) {
            return $null
        }
        $expectedStartedAt = $Process.StartTime.ToUniversalTime().Ticks
    }
    catch {
        throw ($script:RyFrameV07SupportMessages.ProcessInspect -f $Process.Id, $_.Exception.Message)
    }

    $current = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
    if ($null -eq $current) {
        return $null
    }
    try {
        $current.Refresh()
        if ($current.HasExited) {
            return $null
        }
        $actualStartedAt = $current.StartTime.ToUniversalTime().Ticks
        $actualExecutable = $current.Path
    }
    catch {
        throw ($script:RyFrameV07SupportMessages.ProcessInspect -f $Process.Id, $_.Exception.Message)
    }

    $comparison = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
        [System.StringComparison]::OrdinalIgnoreCase
    }
    else {
        [System.StringComparison]::Ordinal
    }
    $sameExecutable = -not [string]::IsNullOrWhiteSpace($actualExecutable) `
        -and [string]::Equals(
            [System.IO.Path]::GetFullPath($actualExecutable),
            [System.IO.Path]::GetFullPath($ExpectedExecutable),
            $comparison
        )
    if ($actualStartedAt -ne $expectedStartedAt -or -not $sameExecutable) {
        throw ($script:RyFrameV07SupportMessages.ProcessIdentity -f $Process.Id)
    }
    return $current
}

function Resolve-RyFrameV07ServiceContainer {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ProjectName,

        [Parameter(Mandatory = $true)]
        [string]$OwnershipToken,

        [Parameter(Mandatory = $true)]
        [string]$ComposeFile,

        [Parameter(Mandatory = $true)]
        [string]$Service,

        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    Assert-RyFrameV07ProjectName -ProjectName $ProjectName
    Assert-RyFrameV07ResourceName -Name $Service
    if (-not (Test-Path -LiteralPath $ComposeFile -PathType Leaf)) {
        throw ($script:RyFrameV07SupportMessages.ComposeMissing -f $ComposeFile)
    }
    $containers = @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @(
            "compose", "--project-name", $ProjectName, "--file", $ComposeFile,
            "ps", "--all", "--quiet", $Service
        ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($containers.Count -ne 1) {
        throw ($script:RyFrameV07SupportMessages.ResourceCount -f $ProjectName, $Service, $containers.Count)
    }
    $containerId = $containers[0].Trim()
    Assert-RyFrameV07ResourceProjectLabel `
        -ResourceKind "container" `
        -Identifier $containerId `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $DockerExecutable `
        -Context $Context
    return $containerId
}

function Resolve-RyFrameV07ProjectNetwork {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ProjectName,

        [Parameter(Mandatory = $true)]
        [string]$OwnershipToken,

        [Parameter(Mandatory = $true)]
        [string]$Network,

        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    Assert-RyFrameV07ProjectName -ProjectName $ProjectName
    Assert-RyFrameV07ResourceName -Name $Network
    $networks = @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @(
            "network", "ls", "--quiet",
            "--filter", "label=com.docker.compose.project=$ProjectName",
            "--filter", "label=com.docker.compose.network=$Network"
        ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($networks.Count -ne 1) {
        throw ($script:RyFrameV07SupportMessages.ResourceCount -f $ProjectName, $Network, $networks.Count)
    }
    $networkId = $networks[0].Trim()
    Assert-RyFrameV07ResourceProjectLabel `
        -ResourceKind "network" `
        -Identifier $networkId `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $DockerExecutable `
        -Context $Context
    return $networkId
}

function Test-RyFrameV07ContainerRunning {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ContainerId,

        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    $state = (@(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @("container", "inspect", "--format", "{{ .State.Running }}", $ContainerId)) -join "").Trim()
    return $state -ceq "true"
}

function Test-RyFrameV07ContainerInNetwork {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ContainerId,

        [Parameter(Mandatory = $true)]
        [string]$NetworkId,

        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    $json = (@(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @("network", "inspect", "--format", "{{json .Containers}}", $NetworkId)) -join "").Trim()
    if ([string]::IsNullOrWhiteSpace($json) -or $json -eq "null") {
        return $false
    }
    $members = $json | ConvertFrom-Json
    return $null -ne $members.PSObject.Properties[$ContainerId]
}

function Stop-RyFrameV07DockerService {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$ComposeFile,
        [Parameter(Mandatory = $true)][string]$Service,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    $containerId = Resolve-RyFrameV07ServiceContainer @PSBoundParameters
    if (-not (Test-RyFrameV07ContainerRunning -ContainerId $containerId -DockerExecutable $DockerExecutable -Context $Context)) {
        throw ($script:RyFrameV07SupportMessages.StateMismatch -f $Service, $script:RyFrameV07SupportMessages.StoppedBeforeInjection)
    }
    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @("container", "stop", "--time", "10", $containerId) `
        -Description ($script:RyFrameV07SupportMessages.StopService -f $ProjectName, $Service)
    if (Test-RyFrameV07ContainerRunning -ContainerId $containerId -DockerExecutable $DockerExecutable -Context $Context) {
        throw ($script:RyFrameV07SupportMessages.StateMismatch -f $Service, $script:RyFrameV07SupportMessages.StillRunning)
    }
    return [pscustomobject]@{
        Kind = "stopped_service"
        ProjectName = $ProjectName
        ComposeFile = $ComposeFile
        Service = $Service
    }
}

function Start-RyFrameV07DockerService {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$ComposeFile,
        [Parameter(Mandatory = $true)][string]$Service,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    $containerId = Resolve-RyFrameV07ServiceContainer @PSBoundParameters
    if (-not (Test-RyFrameV07ContainerRunning -ContainerId $containerId -DockerExecutable $DockerExecutable -Context $Context)) {
        Invoke-RyFrameV07DockerChecked `
            -DockerExecutable $DockerExecutable `
            -Context $Context `
            -Arguments @("container", "start", $containerId) `
            -Description ($script:RyFrameV07SupportMessages.StartService -f $ProjectName, $Service)
    }
    if (-not (Test-RyFrameV07ContainerRunning -ContainerId $containerId -DockerExecutable $DockerExecutable -Context $Context)) {
        throw ($script:RyFrameV07SupportMessages.StateMismatch -f $Service, $script:RyFrameV07SupportMessages.StillStopped)
    }
}

function Disconnect-RyFrameV07DockerServiceNetwork {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$ComposeFile,
        [Parameter(Mandatory = $true)][string]$Service,
        [Parameter(Mandatory = $true)][string]$Network,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    $containerId = Resolve-RyFrameV07ServiceContainer `
        -ProjectName $ProjectName -ComposeFile $ComposeFile -Service $Service `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $DockerExecutable -Context $Context
    $networkId = Resolve-RyFrameV07ProjectNetwork `
        -ProjectName $ProjectName -Network $Network `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $DockerExecutable -Context $Context
    if (-not (Test-RyFrameV07ContainerInNetwork -ContainerId $containerId -NetworkId $networkId -DockerExecutable $DockerExecutable -Context $Context)) {
        throw ($script:RyFrameV07SupportMessages.AlreadyDisconnected -f $Service, $Network)
    }
    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments @("network", "disconnect", $networkId, $containerId) `
        -Description ($script:RyFrameV07SupportMessages.DisconnectNetwork -f $Service, $Network)
    if (Test-RyFrameV07ContainerInNetwork -ContainerId $containerId -NetworkId $networkId -DockerExecutable $DockerExecutable -Context $Context) {
        throw ($script:RyFrameV07SupportMessages.StateMismatch -f $Service, $script:RyFrameV07SupportMessages.StillConnected)
    }
    return [pscustomobject]@{
        Kind = "disconnected_network"
        ProjectName = $ProjectName
        ComposeFile = $ComposeFile
        Service = $Service
        Network = $Network
    }
}

function Restore-RyFrameV07DockerServiceNetwork {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$ComposeFile,
        [Parameter(Mandatory = $true)][string]$Service,
        [Parameter(Mandatory = $true)][string]$Network,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    $containerId = Resolve-RyFrameV07ServiceContainer `
        -ProjectName $ProjectName -ComposeFile $ComposeFile -Service $Service `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $DockerExecutable -Context $Context
    $networkId = Resolve-RyFrameV07ProjectNetwork `
        -ProjectName $ProjectName -Network $Network `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $DockerExecutable -Context $Context
    if (-not (Test-RyFrameV07ContainerInNetwork -ContainerId $containerId -NetworkId $networkId -DockerExecutable $DockerExecutable -Context $Context)) {
        Invoke-RyFrameV07DockerChecked `
            -DockerExecutable $DockerExecutable `
            -Context $Context `
            -Arguments @("network", "connect", $networkId, $containerId) `
            -Description ($script:RyFrameV07SupportMessages.RestoreNetwork -f $Service, $Network)
    }
    if (-not (Test-RyFrameV07ContainerInNetwork -ContainerId $containerId -NetworkId $networkId -DockerExecutable $DockerExecutable -Context $Context)) {
        throw ($script:RyFrameV07SupportMessages.StateMismatch -f $Service, $script:RyFrameV07SupportMessages.StillDisconnected)
    }
}

function Restore-RyFrameV07DockerFault {
    param(
        [Parameter(Mandatory = $true)][psobject]$Fault,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    if ($Fault.Kind -eq "stopped_service") {
        Start-RyFrameV07DockerService `
            -ProjectName $Fault.ProjectName -ComposeFile $Fault.ComposeFile -Service $Fault.Service `
            -OwnershipToken $OwnershipToken `
            -DockerExecutable $DockerExecutable -Context $Context
        return
    }
    if ($Fault.Kind -eq "disconnected_network") {
        Restore-RyFrameV07DockerServiceNetwork `
            -ProjectName $Fault.ProjectName -ComposeFile $Fault.ComposeFile -Service $Fault.Service `
            -OwnershipToken $OwnershipToken `
            -Network $Fault.Network -DockerExecutable $DockerExecutable -Context $Context
        return
    }
    throw ($script:RyFrameV07SupportMessages.FaultToken -f $Fault.Kind)
}

function Get-RyFrameV07ProjectResourceIds {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("container", "network", "volume")]
        [string]$ResourceKind,

        [Parameter(Mandatory = $true)]
        [string]$ProjectName,

        [Parameter(Mandatory = $true)]
        [string]$DockerExecutable,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    Assert-RyFrameV07ProjectName -ProjectName $ProjectName
    $arguments = @($ResourceKind, "ls")
    if ($ResourceKind -eq "container") {
        $arguments += "--all"
    }
    $arguments += @("--quiet", "--filter", "label=com.docker.compose.project=$ProjectName")
    return @(Invoke-RyFrameV07DockerLines `
        -DockerExecutable $DockerExecutable `
        -Context $Context `
        -Arguments $arguments | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

function Assert-RyFrameV07ProjectEmpty {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    Assert-RyFrameV07ProjectName -ProjectName $ProjectName
    $resources = New-Object System.Collections.Generic.List[string]
    foreach ($resourceKind in @("container", "network", "volume")) {
        foreach ($resourceId in @(Get-RyFrameV07ProjectResourceIds `
            -ResourceKind $resourceKind `
            -ProjectName $ProjectName `
            -DockerExecutable $DockerExecutable `
            -Context $Context)) {
            $resources.Add("${resourceKind}:$resourceId")
        }
    }
    if ($resources.Count -gt 0) {
        throw ($script:RyFrameV07SupportMessages.ProjectNotEmpty -f $ProjectName, ($resources -join "; "))
    }
}

function Remove-RyFrameV07DockerProjectResources {
    param(
        [Parameter(Mandatory = $true)][string]$ProjectName,
        [Parameter(Mandatory = $true)][string]$OwnershipToken,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context
    )

    Assert-RyFrameV07ProjectName -ProjectName $ProjectName
    Assert-RyFrameV07OwnershipToken -OwnershipToken $OwnershipToken
    Write-Host ("`n==> {0}" -f ($script:RyFrameV07SupportMessages.Cleanup -f $ProjectName))
    $cleanupFailures = New-Object System.Collections.Generic.List[string]
    foreach ($resourceKind in @("container", "network", "volume")) {
        try {
            $resourceIds = @(Get-RyFrameV07ProjectResourceIds `
                -ResourceKind $resourceKind `
                -ProjectName $ProjectName `
                -DockerExecutable $DockerExecutable `
                -Context $Context)
        }
        catch {
            $cleanupFailures.Add($_.Exception.Message)
            continue
        }
        foreach ($resourceId in $resourceIds) {
            try {
                Assert-RyFrameV07ResourceProjectLabel `
                    -ResourceKind $resourceKind `
                    -Identifier $resourceId `
                    -ProjectName $ProjectName `
                    -OwnershipToken $OwnershipToken `
                    -DockerExecutable $DockerExecutable `
                    -Context $Context
                $removeArguments = @($resourceKind, "rm")
                if ($resourceKind -eq "container") {
                    $removeArguments += "--force"
                }
                $removeArguments += $resourceId
                Invoke-RyFrameV07DockerChecked `
                    -DockerExecutable $DockerExecutable `
                    -Context $Context `
                    -Arguments $removeArguments `
                    -Description ($script:RyFrameV07SupportMessages.Cleanup -f $ProjectName)
            }
            catch {
                $cleanupFailures.Add($_.Exception.Message)
            }
        }
    }

    $remaining = @()
    foreach ($resourceKind in @("container", "network", "volume")) {
        try {
            $remaining += @(Get-RyFrameV07ProjectResourceIds `
                -ResourceKind $resourceKind `
                -ProjectName $ProjectName `
                -DockerExecutable $DockerExecutable `
                -Context $Context | ForEach-Object { "${resourceKind}:$_" })
        }
        catch {
            $cleanupFailures.Add($_.Exception.Message)
        }
    }
    $cleanupDetails = @($cleanupFailures) + @($remaining)
    if ($cleanupDetails.Count -gt 0) {
        throw ($script:RyFrameV07SupportMessages.CleanupRemaining -f $ProjectName, ($cleanupDetails -join "; "))
    }
}

function Write-RyFrameV07MetadataAtomically {
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
        $json = ($Metadata | ConvertTo-Json -Depth 8) + "`n"
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
        Write-Warning ($script:RyFrameV07SupportMessages.MetadataCleanup -f $cleanupError.Exception.Message)
    }
}
