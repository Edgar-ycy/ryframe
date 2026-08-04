[CmdletBinding()]
param(
    [string]$ConfirmRun = "",

    [Parameter(Mandatory = $true)]
    [string]$ProjectName,

    [Parameter(Mandatory = $true)]
    [string]$OwnershipToken,

    [Parameter(Mandatory = $true)]
    [string]$RunDirectory,

    [Parameter(Mandatory = $true)]
    [string]$DockerExecutable,

    [Parameter(Mandatory = $true)]
    [string]$DockerContext,

    [Parameter(Mandatory = $true)]
    [string]$DockerHelperPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:OtelAcceptanceMessages = ConvertFrom-Json @'
{
  "OptIn": "\u5fc5\u987b\u7531 v0.7 \u9a8c\u6536\u5165\u53e3\u4f20\u5165\u7cbe\u786e\u5b50\u9636\u6bb5\u786e\u8ba4\u4ee4\u724c",
  "PowerShellVersion": "OTel \u8fd0\u884c\u9a8c\u6536\u9700\u8981 PowerShell 5.1 \u6216\u66f4\u9ad8\u7248\u672c",
  "ScriptLocation": "OTel \u9a8c\u6536\u811a\u672c\u5fc5\u987b\u4f4d\u4e8e\u4ed3\u5e93 scripts \u76ee\u5f55",
  "HelperPath": "Docker \u652f\u6301\u811a\u672c\u8def\u5f84\u4e0e\u4ed3\u5e93\u56fa\u5b9a\u8def\u5f84\u4e0d\u4e00\u81f4\uff1a{0}",
  "ContextMismatch": "\u5f53\u524d\u672c\u673a Docker context\u201c{0}\u201d\u4e0e\u9a8c\u6536\u4f20\u5165\u503c\u201c{1}\u201d\u4e0d\u4e00\u81f4",
  "RunDirectory": "OTel \u8bc1\u636e\u76ee\u5f55\u5fc5\u987b\u4f4d\u4e8e v0.7 \u4e13\u7528 target \u6839\u76ee\u5f55\u5185\uff1a{0}",
  "EvidenceExists": "OTel \u9a8c\u6536\u8bc1\u636e\u5df2\u5b58\u5728\uff0c\u62d2\u7edd\u8986\u76d6\uff1a{0}",
  "MissingFile": "OTel \u9a8c\u6536\u7f3a\u5c11\u6587\u4ef6\uff1a{0}",
  "CommandFailed": "{0}\u5931\u8d25\uff0c\u9000\u51fa\u7801\u4e3a {1}",
  "PortUnavailable": "\u56de\u73af\u7aef\u53e3 {0} \u5df2\u88ab\u5360\u7528\u6216\u4e0d\u53ef\u7ed1\u5b9a",
  "MissingBinary": "OTel \u9a8c\u6536\u590d\u7528\u524d\u7f6e\u9636\u6bb5\u4e8c\u8fdb\u5236\u65f6\u4ecd\u7f3a\u5c11\u6587\u4ef6\uff1a{0}",
  "ImageEvidence": "OTel \u9a8c\u6536\u955c\u50cf\u8bc1\u636e\u5fc5\u987b\u7cbe\u786e\u5305\u542b mysql\u3001redis\u3001rustfs \u548c otel-collector\uff1a{0}",
  "ComposeValidate": "\u6821\u9a8c OTel \u9694\u79bb Compose \u914d\u7f6e",
  "ComposeStart": "\u542f\u52a8\u9694\u79bb MySQL\u3001Redis\u3001RustFS \u4e0e OpenTelemetry Collector",
  "ResetDatabase": "\u91cd\u7f6e OTel \u9694\u79bb\u6570\u636e\u5e93",
  "MigrationStatus": "\u68c0\u67e5 OTel \u9a8c\u6536\u8fc1\u79fb\u8d26\u672c",
  "MigrationVerify": "\u9a8c\u8bc1 OTel \u9a8c\u6536\u6570\u636e\u5e93\u7ed3\u6784",
  "ProcessExited": "{0}\u8fdb\u7a0b\u5728\u9a8c\u6536\u5b8c\u6210\u524d\u9000\u51fa\uff0cPID \u4e3a {1}",
  "ProcessIdentity": "{0}\u8fdb\u7a0b PID {1} \u7684\u53ef\u6267\u884c\u6587\u4ef6\u4e0e\u8bb0\u5f55\u4e0d\u4e00\u81f4",
  "ProcessStop": "{0}\u8fdb\u7a0b PID {1} \u672a\u80fd\u5728\u9650\u5b9a\u65f6\u95f4\u5185\u505c\u6b62",
  "Readiness": "{0}\u672a\u5728 {1} \u79d2\u5185\u5c31\u7eea",
  "HttpStatus": "{0}\u9884\u671f HTTP {1}\uff0c\u5b9e\u9645\u4e3a {2}\uff1a{3}",
  "ApiContract": "{0}\u54cd\u5e94\u4e0d\u7b26\u5408\u5f53\u524d API \u5951\u7ea6\uff1a{1}",
  "MetricMissing": "{0}\u7f3a\u5c11 OTel \u5bfc\u51fa\u5931\u8d25\u6307\u6807",
  "MetricNotIncreased": "Collector \u4e2d\u65ad\u540e {0} \u7684 OTel \u5bfc\u51fa\u5931\u8d25\u6307\u6807\u672a\u589e\u957f",
  "CollectorTraceTimeout": "Collector \u672a\u5728 {0} \u79d2\u5185\u5bfc\u51fa\u76ee\u6807 trace\uff1a{1}",
  "TraceFileMissing": "Collector \u8ddf\u8e2a\u8bc1\u636e\u4e0d\u5b58\u5728\u6216\u4e3a\u7a7a\uff1a{0}",
  "TraceJson": "Collector \u8ddf\u8e2a JSON \u65e0\u6cd5\u89e3\u6790\uff1a{0}",
  "TraceAssertion": "OTel \u7236\u5b50\u94fe\u65ad\u8a00\u5931\u8d25\uff1a{0}",
  "CsrfLabel": "CSRF \u6311\u6218",
  "LoginLabel": "\u7ba1\u7406\u5458\u767b\u5f55",
  "UploadLabel": "RustFS \u4e0a\u4f20",
  "CreateExportLabel": "\u521b\u5efa\u5f02\u6b65\u5bfc\u51fa\u4efb\u52a1",
  "QueryExportLabel": "\u67e5\u8be2\u5f02\u6b65\u5bfc\u51fa\u4efb\u52a1",
  "ExportLabel": "\u5f02\u6b65\u5bfc\u51fa\u4efb\u52a1",
  "MetricsLabel": "{0} \u6307\u6807",
  "EmptySpans": "Collector \u8bc1\u636e\u4e2d\u6ca1\u6709 span",
  "HttpRouteCount": "trace {0} \u7684 HTTP \u8def\u7531 {1} \u6570\u91cf\u4e3a {2}",
  "UploadParent": "\u4e0a\u4f20 HTTP span \u672a\u6062\u590d\u5916\u90e8 traceparent",
  "TraceState": "span \u672a\u5b8c\u6574\u4fdd\u7559\u5916\u90e8 tracestate",
  "UploadDependencies": "\u4e0a\u4f20 HTTP \u672a\u5f62\u6210 SQL/Redis/\u5bf9\u8c61\u5b58\u50a8\u5b8c\u6574\u5b50\u94fe",
  "TaskParent": "\u5bfc\u51fa HTTP span \u672a\u6062\u590d\u5916\u90e8 traceparent",
  "TaskChain": "\u521b\u5efa\u4efb\u52a1\u672a\u5f62\u6210 Worker \u4e0e Outbox \u8de8\u8fdb\u7a0b\u5b50\u94fe",
  "HealthyChainTimeout": "\u7b49\u5f85\u5065\u5eb7\u94fe\u8def\u8d85\u65f6\uff1a{0}",
  "RecoveryChainTimeout": "\u7b49\u5f85\u6062\u590d\u94fe\u8def\u8d85\u65f6\uff1a{0}",
  "OutageApiReady": "Collector \u4e2d\u65ad\u671f\u95f4 API \u5c31\u7eea",
  "OutageWorkerReady": "Collector \u4e2d\u65ad\u671f\u95f4 Worker \u5c31\u7eea",
  "CollectorRecovered": "\u6062\u590d\u540e OpenTelemetry Collector",
  "CopyTraces": "\u590d\u5236 Collector \u8ddf\u8e2a\u8bc1\u636e",
  "CollectorRestore": "Collector \u6545\u969c\u6062\u590d\u5931\u8d25\uff1a{0}",
  "CollectorLogEvidence": "Collector \u65e5\u5fd7\u4fdd\u5b58\u5931\u8d25\uff1a{0}",
  "CollectorTraceEvidence": "Collector trace \u4fdd\u5b58\u5931\u8d25\uff1a{0}",
  "ProcessCleanup": "{0}\u8fdb\u7a0b\u6e05\u7406\u5931\u8d25\uff1a{1}",
  "DockerCleanup": "OTel Docker \u8d44\u6e90\u6e05\u7406\u5931\u8d25\uff1a{0}",
  "DirectoryRestore": "\u5de5\u4f5c\u76ee\u5f55\u6062\u590d\u5931\u8d25\uff1a{0}",
  "EnvironmentRestore": "\u8fdb\u7a0b\u73af\u5883\u53d8\u91cf\u6062\u590d\u5931\u8d25\uff1a{0}",
  "TranscriptCleanup": "OTel \u9a8c\u6536\u65e5\u5fd7\u6536\u5c3e\u5931\u8d25\uff1a{0}",
  "MetadataWrite": "OTel \u9a8c\u6536\u8bc1\u636e\u5199\u5165\u5931\u8d25\uff1a{0}",
  "Success": "OTel \u5916\u90e8\u7236\u94fe\u3001\u4f9d\u8d56\u5b50\u94fe\u3001Worker/Outbox \u4f20\u64ad\u4e0e Collector \u6545\u969c\u6062\u590d\u9a8c\u6536\u901a\u8fc7\u3002\u8bc1\u636e\u76ee\u5f55\uff1a{0}"
}
'@

if ($ConfirmRun -cne "RUN-RYFRAME-V0-7-STAGE") {
    throw $script:OtelAcceptanceMessages.OptIn
}
if ($PSVersionTable.PSVersion -lt [version]"5.1") {
    throw $script:OtelAcceptanceMessages.PowerShellVersion
}

function Test-OtelAcceptanceSamePath {
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

function Invoke-OtelAcceptanceCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Description
    )

    Write-Host ("`n==> {0}" -f $Description)
    & $Executable @Arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw ($script:OtelAcceptanceMessages.CommandFailed -f $Description, $exitCode)
    }
}

function Get-OtelAcceptanceFreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function Get-OtelAcceptancePorts {
    param([Parameter(Mandatory = $true)][string[]]$Names)

    $ports = [ordered]@{}
    $used = New-Object System.Collections.Generic.HashSet[int]
    foreach ($name in $Names) {
        do {
            $port = Get-OtelAcceptanceFreePort
        } while (-not $used.Add($port))
        $ports[$name] = $port
    }
    return $ports
}

function Assert-OtelAcceptancePortsAvailable {
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
            throw ($script:OtelAcceptanceMessages.PortUnavailable -f $port)
        }
        finally {
            $listener.Stop()
        }
    }
}

function Set-OtelAcceptanceEnvironment {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Value
    )

    [System.Environment]::SetEnvironmentVariable($Name, $Value, "Process")
}

function Start-OtelAcceptanceProcess {
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

function Assert-OtelAcceptanceProcess {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $current = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $current) {
        throw ($script:OtelAcceptanceMessages.ProcessExited -f $Label, $Process.Id)
    }
}

function Stop-OtelAcceptanceProcess {
    param(
        [AllowNull()][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $current = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $current) {
        return
    }
    Stop-Process -InputObject $current -ErrorAction Stop
    if ($Process.WaitForExit(10000)) {
        return
    }
    $current = Get-RyFrameV07OwnedProcess `
        -Process $Process `
        -ExpectedExecutable $ExpectedExecutable
    if ($null -eq $current) {
        return
    }
    Stop-Process -InputObject $current -Force -ErrorAction Stop
    if (-not $Process.WaitForExit(10000)) {
        throw ($script:OtelAcceptanceMessages.ProcessStop -f $Label, $Process.Id)
    }
}

function Wait-OtelAcceptanceProcessReadiness {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)][string]$ExpectedExecutable,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 120
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        Assert-OtelAcceptanceProcess -Process $Process -ExpectedExecutable $ExpectedExecutable -Label $Label
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
    throw ($script:OtelAcceptanceMessages.Readiness -f $Label, $TimeoutSeconds)
}

function Wait-OtelAcceptanceUri {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 60
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
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
    throw ($script:OtelAcceptanceMessages.Readiness -f $Label, $TimeoutSeconds)
}

function Write-OtelAcceptanceText {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content
    )

    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Invoke-OtelAcceptanceWebRequest {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [ValidateSet("GET", "POST")][string]$Method = "GET",
        [System.Collections.IDictionary]$Headers = @{},
        [AllowNull()][object]$Body = $null,
        [AllowNull()][string]$ContentType = $null,
        [AllowNull()][Microsoft.PowerShell.Commands.WebRequestSession]$WebSession = $null,
        [Parameter(Mandatory = $true)][int]$ExpectedStatus,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $request = @{
        Uri = $Uri.AbsoluteUri
        Method = $Method
        Headers = $Headers
        UseBasicParsing = $true
        TimeoutSec = 15
    }
    if ($null -ne $Body) {
        $request.Body = $Body
    }
    if (-not [string]::IsNullOrWhiteSpace($ContentType)) {
        $request.ContentType = $ContentType
    }
    if ($null -ne $WebSession) {
        $request.WebSession = $WebSession
    }
    try {
        $response = Invoke-WebRequest @request
    }
    catch {
        $actualStatus = 0
        $responseBody = $_.Exception.Message
        if ($null -ne $_.Exception.Response) {
            try {
                $actualStatus = [int]$_.Exception.Response.StatusCode
                $reader = [System.IO.StreamReader]::new($_.Exception.Response.GetResponseStream())
                try {
                    $responseBody = $reader.ReadToEnd()
                }
                finally {
                    $reader.Dispose()
                }
            }
            catch {
            }
        }
        throw ($script:OtelAcceptanceMessages.HttpStatus -f $Label, $ExpectedStatus, $actualStatus, $responseBody)
    }
    if ([int]$response.StatusCode -ne $ExpectedStatus) {
        throw ($script:OtelAcceptanceMessages.HttpStatus -f $Label, $ExpectedStatus, $response.StatusCode, $response.Content)
    }
    return $response
}

function New-OtelAcceptanceTraceContext {
    $traceId = [guid]::NewGuid().ToString("N").ToLowerInvariant()
    do {
        $parentSpanId = [guid]::NewGuid().ToString("N").Substring(0, 16).ToLowerInvariant()
    } while ($parentSpanId -eq "0000000000000000")
    return [pscustomobject]@{
        TraceId = $traceId
        ParentSpanId = $parentSpanId
        Header = "00-$traceId-$parentSpanId-01"
        TraceState = "ryframe=v07$($traceId.Substring(0, 12))"
    }
}

function Invoke-OtelAcceptanceLogin {
    param([Parameter(Mandatory = $true)][uri]$ApiBase)

    $session = New-Object Microsoft.PowerShell.Commands.WebRequestSession
    $csrfResponse = Invoke-OtelAcceptanceWebRequest `
        -Uri ([uri]::new($ApiBase, "/api/v1/auth/csrf")) `
        -WebSession $session `
        -ExpectedStatus 200 `
        -Label $script:OtelAcceptanceMessages.CsrfLabel
    $csrfJson = $csrfResponse.Content | ConvertFrom-Json
    $csrfToken = $csrfJson.data.csrf_token
    if ([string]::IsNullOrWhiteSpace($csrfToken)) {
        throw ($script:OtelAcceptanceMessages.ApiContract -f $script:OtelAcceptanceMessages.CsrfLabel, $csrfResponse.Content)
    }
    $loginResponse = Invoke-OtelAcceptanceWebRequest `
        -Uri ([uri]::new($ApiBase, "/api/v1/auth/login")) `
        -Method "POST" `
        -Headers @{
            "X-Tenant-Id" = "system"
            "X-CSRF-Token" = $csrfToken
        } `
        -Body '{"username":"admin","password":"123456"}' `
        -ContentType "application/json" `
        -WebSession $session `
        -ExpectedStatus 200 `
        -Label $script:OtelAcceptanceMessages.LoginLabel
    $loginJson = $loginResponse.Content | ConvertFrom-Json
    $accessToken = $loginJson.data.access_token
    if ($loginJson.code -ne 200 -or [string]::IsNullOrWhiteSpace($accessToken)) {
        throw ($script:OtelAcceptanceMessages.ApiContract -f $script:OtelAcceptanceMessages.LoginLabel, $loginResponse.Content)
    }
    return [pscustomobject]@{
        AccessToken = $accessToken
        Session = $session
    }
}

function Get-OtelAcceptanceAuthHeaders {
    param(
        [Parameter(Mandatory = $true)][string]$AccessToken,
        [AllowNull()][string]$Traceparent = $null,
        [AllowNull()][string]$Tracestate = $null
    )

    $headers = @{
        Authorization = "Bearer $AccessToken"
        "X-Tenant-Id" = "system"
    }
    if (-not [string]::IsNullOrWhiteSpace($Traceparent)) {
        $headers["traceparent"] = $Traceparent
    }
    if (-not [string]::IsNullOrWhiteSpace($Tracestate)) {
        $headers["tracestate"] = $Tracestate
    }
    return $headers
}

function Invoke-OtelAcceptanceUpload {
    param(
        [Parameter(Mandatory = $true)][uri]$ApiBase,
        [Parameter(Mandatory = $true)][string]$AccessToken,
        [Parameter(Mandatory = $true)][string]$Traceparent,
        [Parameter(Mandatory = $true)][string]$Tracestate,
        [Parameter(Mandatory = $true)][string]$EvidencePath
    )

    Add-Type -AssemblyName System.Net.Http
    $client = [System.Net.Http.HttpClient]::new()
    $request = [System.Net.Http.HttpRequestMessage]::new(
        [System.Net.Http.HttpMethod]::Post,
        [uri]::new($ApiBase, "/api/v1/common/upload")
    )
    $multipart = [System.Net.Http.MultipartFormDataContent]::new()
    try {
        $request.Headers.Authorization = [System.Net.Http.Headers.AuthenticationHeaderValue]::new(
            "Bearer",
            $AccessToken
        )
        [void]$request.Headers.TryAddWithoutValidation("X-Tenant-Id", "system")
        [void]$request.Headers.TryAddWithoutValidation("traceparent", $Traceparent)
        [void]$request.Headers.TryAddWithoutValidation("tracestate", $Tracestate)
        $payload = [System.Text.Encoding]::UTF8.GetBytes("ryframe-v07-otel-upload")
        $fileContent = [System.Net.Http.ByteArrayContent]::new($payload)
        $fileContent.Headers.ContentType = [System.Net.Http.Headers.MediaTypeHeaderValue]::new(
            "text/plain"
        )
        $multipart.Add($fileContent, "file", "otel-runtime-acceptance.txt")
        $request.Content = $multipart
        $response = $client.SendAsync($request).GetAwaiter().GetResult()
        $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        Write-OtelAcceptanceText -Path $EvidencePath -Content $body
        if ([int]$response.StatusCode -ne 200) {
            throw ($script:OtelAcceptanceMessages.HttpStatus -f $script:OtelAcceptanceMessages.UploadLabel, 200, [int]$response.StatusCode, $body)
        }
        $json = $body | ConvertFrom-Json
        if ($json.code -ne 200 -or @($json.data).Count -ne 1) {
            throw ($script:OtelAcceptanceMessages.ApiContract -f $script:OtelAcceptanceMessages.UploadLabel, $body)
        }
        return $json.data[0]
    }
    finally {
        $request.Dispose()
        $client.Dispose()
    }
}

function Invoke-OtelAcceptanceExport {
    param(
        [Parameter(Mandatory = $true)][uri]$ApiBase,
        [Parameter(Mandatory = $true)][string]$AccessToken,
        [Parameter(Mandatory = $true)][string]$Traceparent,
        [Parameter(Mandatory = $true)][string]$Tracestate,
        [Parameter(Mandatory = $true)][string]$IdempotencyKey,
        [Parameter(Mandatory = $true)][string]$EvidencePath
    )

    $headers = Get-OtelAcceptanceAuthHeaders `
        -AccessToken $AccessToken `
        -Traceparent $Traceparent `
        -Tracestate $Tracestate
    $headers["Idempotency-Key"] = $IdempotencyKey
    $response = Invoke-OtelAcceptanceWebRequest `
        -Uri ([uri]::new($ApiBase, "/api/v1/system/users/exports")) `
        -Method "POST" `
        -Headers $headers `
        -Body "{}" `
        -ContentType "application/json" `
        -ExpectedStatus 202 `
        -Label $script:OtelAcceptanceMessages.CreateExportLabel
    Write-OtelAcceptanceText -Path $EvidencePath -Content $response.Content
    $json = $response.Content | ConvertFrom-Json
    $jobId = [string]$json.data.id
    if (
        $json.code -ne 202 `
        -or [string]::IsNullOrWhiteSpace($jobId) `
        -or $json.data.status -cne "queued" `
        -or $json.data.resource -cne "users"
    ) {
        throw ($script:OtelAcceptanceMessages.ApiContract -f $script:OtelAcceptanceMessages.CreateExportLabel, $response.Content)
    }
    return $jobId
}

function Wait-OtelAcceptanceExport {
    param(
        [Parameter(Mandatory = $true)][uri]$ApiBase,
        [Parameter(Mandatory = $true)][string]$AccessToken,
        [Parameter(Mandatory = $true)][string]$JobId,
        [Parameter(Mandatory = $true)][string]$EvidencePath,
        [int]$TimeoutSeconds = 45
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $response = Invoke-OtelAcceptanceWebRequest `
            -Uri ([uri]::new($ApiBase, "/api/v1/common/jobs/$JobId")) `
            -Headers (Get-OtelAcceptanceAuthHeaders -AccessToken $AccessToken) `
            -ExpectedStatus 200 `
            -Label $script:OtelAcceptanceMessages.QueryExportLabel
        $json = $response.Content | ConvertFrom-Json
        $status = [string]$json.data.status
        if ($status -eq "succeeded") {
            Write-OtelAcceptanceText -Path $EvidencePath -Content $response.Content
            return $json.data
        }
        if ($status -in @("failed", "cancelled", "expired")) {
            throw ($script:OtelAcceptanceMessages.ApiContract -f $script:OtelAcceptanceMessages.ExportLabel, $response.Content)
        }
        Start-Sleep -Milliseconds 300
    }
    throw ($script:OtelAcceptanceMessages.Readiness -f $script:OtelAcceptanceMessages.ExportLabel, $TimeoutSeconds)
}

function Get-OtelAcceptanceMetricValue {
    param(
        [Parameter(Mandatory = $true)][string]$Metrics,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $match = [regex]::Match(
        $Metrics,
        "(?m)^ryframe_otel_exporter_runtime_failures_total(?:\{[^}]*\})?\s+([0-9.eE+\-]+)\s*$"
    )
    if (-not $match.Success) {
        throw ($script:OtelAcceptanceMessages.MetricMissing -f $Label)
    }
    return [double]::Parse(
        $match.Groups[1].Value,
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function Get-OtelAcceptanceMetrics {
    param(
        [Parameter(Mandatory = $true)][uri]$Uri,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $response = Invoke-OtelAcceptanceWebRequest `
        -Uri $Uri `
        -ExpectedStatus 200 `
        -Label ($script:OtelAcceptanceMessages.MetricsLabel -f $Label)
    return [pscustomobject]@{
        Text = $response.Content
        FailureCount = Get-OtelAcceptanceMetricValue -Metrics $response.Content -Label $Label
    }
}

function Wait-OtelAcceptanceFailureMetrics {
    param(
        [Parameter(Mandatory = $true)][uri]$ApiMetricsUri,
        [Parameter(Mandatory = $true)][uri]$WorkerMetricsUri,
        [Parameter(Mandatory = $true)][double]$ApiBefore,
        [Parameter(Mandatory = $true)][double]$WorkerBefore,
        [int]$TimeoutSeconds = 45
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $apiMetrics = $null
    $workerMetrics = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $apiMetrics = Get-OtelAcceptanceMetrics -Uri $ApiMetricsUri -Label "API"
        $workerMetrics = Get-OtelAcceptanceMetrics -Uri $WorkerMetricsUri -Label "Worker"
        if (
            $apiMetrics.FailureCount -gt $ApiBefore `
            -and $workerMetrics.FailureCount -gt $WorkerBefore
        ) {
            return [pscustomobject]@{
                Api = $apiMetrics
                Worker = $workerMetrics
            }
        }
        Start-Sleep -Seconds 1
    }
    if ($null -eq $apiMetrics -or $apiMetrics.FailureCount -le $ApiBefore) {
        throw ($script:OtelAcceptanceMessages.MetricNotIncreased -f "API")
    }
    throw ($script:OtelAcceptanceMessages.MetricNotIncreased -f "Worker")
}

function Wait-OtelAcceptanceCollectorTraces {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string[]]$TraceIds,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context,
        [int]$TimeoutSeconds = 45
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastLog = ""
    while ([DateTime]::UtcNow -lt $deadline) {
        $lastLog = @(Invoke-RyFrameV07DockerLines `
            -DockerExecutable $DockerExecutable `
            -Context $Context `
            -Arguments @("container", "logs", $ContainerId)) -join [Environment]::NewLine
        $normalizedLog = $lastLog.ToLowerInvariant()
        $missing = @($TraceIds | Where-Object {
            -not $normalizedLog.Contains($_.ToLowerInvariant())
        })
        if ($missing.Count -eq 0) {
            return $lastLog
        }
        Start-Sleep -Milliseconds 500
    }
    throw ($script:OtelAcceptanceMessages.CollectorTraceTimeout -f $TimeoutSeconds, ($TraceIds -join ","))
}

function Copy-OtelAcceptanceTraces {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context,
        [bool]$Quiet = $false
    )

    $arguments = @(
        "container", "cp",
        ("{0}:/var/lib/otel/traces.jsonl" -f $ContainerId),
        $Destination
    )
    if ($Quiet) {
        [void]@(Invoke-RyFrameV07DockerLines `
            -DockerExecutable $DockerExecutable `
            -Context $Context `
            -Arguments $arguments)
    }
    else {
        Invoke-RyFrameV07DockerChecked `
            -DockerExecutable $DockerExecutable `
            -Context $Context `
            -Arguments $arguments `
            -Description $script:OtelAcceptanceMessages.CopyTraces
    }
    if (
        -not (Test-Path -LiteralPath $Destination -PathType Leaf) `
        -or (Get-Item -LiteralPath $Destination).Length -eq 0
    ) {
        throw ($script:OtelAcceptanceMessages.TraceFileMissing -f $Destination)
    }
}

function Get-OtelAcceptanceProperty {
    param(
        [AllowNull()][object]$Object,
        [Parameter(Mandatory = $true)][string]$Name
    )

    if ($null -eq $Object) {
        return $null
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Get-OtelAcceptanceJsonDocuments {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw ($script:OtelAcceptanceMessages.TraceFileMissing -f $Path)
    }
    $text = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw ($script:OtelAcceptanceMessages.TraceFileMissing -f $Path)
    }
    $documents = New-Object System.Collections.Generic.List[string]
    $depth = 0
    $start = -1
    $insideString = $false
    $escaped = $false
    for ($index = 0; $index -lt $text.Length; $index++) {
        $character = $text[$index]
        if ($insideString) {
            if ($escaped) {
                $escaped = $false
            }
            elseif ([int]$character -eq 92) {
                $escaped = $true
            }
            elseif ([int]$character -eq 34) {
                $insideString = $false
            }
            continue
        }
        if ([int]$character -eq 34) {
            $insideString = $true
            continue
        }
        if ([int]$character -eq 123) {
            if ($depth -eq 0) {
                $start = $index
            }
            $depth += 1
            continue
        }
        if ([int]$character -eq 125) {
            $depth -= 1
            if ($depth -lt 0) {
                throw ($script:OtelAcceptanceMessages.TraceJson -f $Path)
            }
            if ($depth -eq 0 -and $start -ge 0) {
                $documents.Add($text.Substring($start, $index - $start + 1))
                $start = -1
            }
        }
    }
    if ($depth -ne 0 -or $insideString -or $documents.Count -eq 0) {
        throw ($script:OtelAcceptanceMessages.TraceJson -f $Path)
    }
    return $documents.ToArray()
}

function ConvertTo-OtelAcceptanceAttributeMap {
    param([AllowNull()][object]$Attributes)

    $map = @{}
    foreach ($attribute in @($Attributes)) {
        if ($null -eq $attribute) {
            continue
        }
        $key = [string](Get-OtelAcceptanceProperty -Object $attribute -Name "key")
        if ([string]::IsNullOrWhiteSpace($key)) {
            continue
        }
        $valueObject = Get-OtelAcceptanceProperty -Object $attribute -Name "value"
        $value = $null
        foreach ($propertyName in @("stringValue", "intValue", "doubleValue", "boolValue")) {
            $candidate = Get-OtelAcceptanceProperty -Object $valueObject -Name $propertyName
            if ($null -ne $candidate) {
                $value = $candidate
                break
            }
        }
        if ($null -eq $value -and $null -ne $valueObject) {
            $value = $valueObject | ConvertTo-Json -Depth 12 -Compress
        }
        $map[$key] = $value
    }
    return $map
}

function Get-OtelAcceptanceSpans {
    param([Parameter(Mandatory = $true)][string]$Path)

    $spans = New-Object System.Collections.Generic.List[object]
    foreach ($document in @(Get-OtelAcceptanceJsonDocuments -Path $Path)) {
        try {
            $payload = $document | ConvertFrom-Json
        }
        catch {
            throw ($script:OtelAcceptanceMessages.TraceJson -f $_.Exception.Message)
        }
        foreach ($resourceSpans in @(Get-OtelAcceptanceProperty -Object $payload -Name "resourceSpans")) {
            if ($null -eq $resourceSpans) {
                continue
            }
            $resource = Get-OtelAcceptanceProperty -Object $resourceSpans -Name "resource"
            $resourceAttributes = ConvertTo-OtelAcceptanceAttributeMap `
                -Attributes (Get-OtelAcceptanceProperty -Object $resource -Name "attributes")
            $serviceName = if ($resourceAttributes.ContainsKey("service.name")) {
                [string]$resourceAttributes["service.name"]
            }
            else {
                ""
            }
            foreach ($scopeSpans in @(Get-OtelAcceptanceProperty -Object $resourceSpans -Name "scopeSpans")) {
                if ($null -eq $scopeSpans) {
                    continue
                }
                foreach ($span in @(Get-OtelAcceptanceProperty -Object $scopeSpans -Name "spans")) {
                    if ($null -eq $span) {
                        continue
                    }
                    $attributes = ConvertTo-OtelAcceptanceAttributeMap `
                        -Attributes (Get-OtelAcceptanceProperty -Object $span -Name "attributes")
                    $spans.Add([pscustomobject]@{
                        TraceId = ([string](Get-OtelAcceptanceProperty -Object $span -Name "traceId")).ToLowerInvariant()
                        SpanId = ([string](Get-OtelAcceptanceProperty -Object $span -Name "spanId")).ToLowerInvariant()
                        ParentSpanId = ([string](Get-OtelAcceptanceProperty -Object $span -Name "parentSpanId")).ToLowerInvariant()
                        TraceState = [string](Get-OtelAcceptanceProperty -Object $span -Name "traceState")
                        Name = [string](Get-OtelAcceptanceProperty -Object $span -Name "name")
                        ServiceName = $serviceName
                        Attributes = $attributes
                    })
                }
            }
        }
    }
    if ($spans.Count -eq 0) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.EmptySpans)
    }
    return $spans.ToArray()
}

function Test-OtelAcceptanceDescendant {
    param(
        [Parameter(Mandatory = $true)][object[]]$Spans,
        [Parameter(Mandatory = $true)][object]$Candidate,
        [Parameter(Mandatory = $true)][string]$AncestorSpanId
    )

    $index = @{}
    foreach ($span in $Spans) {
        $index[$span.SpanId] = $span
    }
    $visited = New-Object System.Collections.Generic.HashSet[string]
    $parentId = $Candidate.ParentSpanId
    while (-not [string]::IsNullOrWhiteSpace($parentId) -and $visited.Add($parentId)) {
        if ($parentId -eq $AncestorSpanId) {
            return $true
        }
        if (-not $index.ContainsKey($parentId)) {
            return $false
        }
        $parentId = $index[$parentId].ParentSpanId
    }
    return $false
}

function Find-OtelAcceptanceHttpSpan {
    param(
        [Parameter(Mandatory = $true)][object[]]$Spans,
        [Parameter(Mandatory = $true)][string]$TraceId,
        [Parameter(Mandatory = $true)][string]$Route
    )

    $matches = @($Spans | Where-Object {
        $_.TraceId -eq $TraceId `
        -and $_.ServiceName -eq "ryframe-api-v07" `
        -and $_.Attributes.ContainsKey("http.route") `
        -and [string]$_.Attributes["http.route"] -eq $Route
    })
    if ($matches.Count -ne 1) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f (
            $script:OtelAcceptanceMessages.HttpRouteCount -f $TraceId, $Route, $matches.Count
        ))
    }
    return $matches[0]
}

function Assert-OtelAcceptanceUploadChain {
    param(
        [Parameter(Mandatory = $true)][object[]]$Spans,
        [Parameter(Mandatory = $true)][object]$TraceContext
    )

    $traceSpans = @($Spans | Where-Object { $_.TraceId -eq $TraceContext.TraceId })
    $http = Find-OtelAcceptanceHttpSpan `
        -Spans $traceSpans `
        -TraceId $TraceContext.TraceId `
        -Route "/api/v1/common/upload"
    if ($http.ParentSpanId -ne $TraceContext.ParentSpanId) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.UploadParent)
    }
    if ($http.TraceState -cne $TraceContext.TraceState) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.TraceState)
    }
    $mysql = @($traceSpans | Where-Object {
        $_.Attributes.ContainsKey("db.system") `
        -and [string]$_.Attributes["db.system"] -eq "mysql" `
        -and (Test-OtelAcceptanceDescendant -Spans $traceSpans -Candidate $_ -AncestorSpanId $http.SpanId)
    })
    $redis = @($traceSpans | Where-Object {
        $_.Attributes.ContainsKey("db.system") `
        -and [string]$_.Attributes["db.system"] -eq "redis" `
        -and (Test-OtelAcceptanceDescendant -Spans $traceSpans -Candidate $_ -AncestorSpanId $http.SpanId)
    })
    $storage = @($traceSpans | Where-Object {
        $_.Attributes.ContainsKey("storage.backend") `
        -and (Test-OtelAcceptanceDescendant -Spans $traceSpans -Candidate $_ -AncestorSpanId $http.SpanId)
    })
    if ($mysql.Count -eq 0 -or $redis.Count -eq 0 -or $storage.Count -eq 0) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.UploadDependencies)
    }
    return [ordered]@{
        trace_id = $TraceContext.TraceId
        tracestate = $TraceContext.TraceState
        external_parent_span_id = $TraceContext.ParentSpanId
        http_span_id = $http.SpanId
        mysql_span_count = $mysql.Count
        redis_span_count = $redis.Count
        storage_span_count = $storage.Count
    }
}

function Assert-OtelAcceptanceTaskChain {
    param(
        [Parameter(Mandatory = $true)][object[]]$Spans,
        [Parameter(Mandatory = $true)][object]$TraceContext
    )

    $traceSpans = @($Spans | Where-Object { $_.TraceId -eq $TraceContext.TraceId })
    $http = Find-OtelAcceptanceHttpSpan `
        -Spans $traceSpans `
        -TraceId $TraceContext.TraceId `
        -Route "/api/v1/system/users/exports"
    if ($http.ParentSpanId -ne $TraceContext.ParentSpanId) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.TaskParent)
    }
    if ($http.TraceState -cne $TraceContext.TraceState) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.TraceState)
    }
    $backgroundJobs = @($traceSpans | Where-Object {
        $_.ServiceName -eq "ryframe-worker-v07" `
        -and $_.Name -eq "background_job" `
        -and (Test-OtelAcceptanceDescendant -Spans $traceSpans -Candidate $_ -AncestorSpanId $http.SpanId)
    })
    $outboxEvents = @($traceSpans | Where-Object {
        $_.ServiceName -eq "ryframe-worker-v07" `
        -and $_.Name -eq "outbox_event" `
        -and (Test-OtelAcceptanceDescendant -Spans $traceSpans -Candidate $_ -AncestorSpanId $http.SpanId)
    })
    if ($backgroundJobs.Count -eq 0 -or $outboxEvents.Count -eq 0) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.TaskChain)
    }
    $traceStateDrift = @($backgroundJobs + $outboxEvents | Where-Object {
        $_.TraceState -cne $TraceContext.TraceState
    })
    if ($traceStateDrift.Count -gt 0) {
        throw ($script:OtelAcceptanceMessages.TraceAssertion -f $script:OtelAcceptanceMessages.TraceState)
    }
    return [ordered]@{
        trace_id = $TraceContext.TraceId
        tracestate = $TraceContext.TraceState
        external_parent_span_id = $TraceContext.ParentSpanId
        http_span_id = $http.SpanId
        background_job_span_count = $backgroundJobs.Count
        outbox_event_span_count = $outboxEvents.Count
    }
}

function Wait-OtelAcceptanceHealthyChains {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][object]$UploadTraceContext,
        [Parameter(Mandatory = $true)][object]$TaskTraceContext,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context,
        [int]$TimeoutSeconds = 45
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = $script:OtelAcceptanceMessages.EmptySpans
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            Copy-OtelAcceptanceTraces `
                -ContainerId $ContainerId `
                -Destination $Destination `
                -DockerExecutable $DockerExecutable `
                -Context $Context `
                -Quiet $true
            $spans = @(Get-OtelAcceptanceSpans -Path $Destination)
            $upload = Assert-OtelAcceptanceUploadChain `
                -Spans $spans `
                -TraceContext $UploadTraceContext
            $task = Assert-OtelAcceptanceTaskChain `
                -Spans $spans `
                -TraceContext $TaskTraceContext
            return [pscustomobject]@{
                Upload = $upload
                Task = $task
            }
        }
        catch {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 500
    }
    throw ($script:OtelAcceptanceMessages.TraceAssertion -f (
        $script:OtelAcceptanceMessages.HealthyChainTimeout -f $lastError
    ))
}

function Wait-OtelAcceptanceTaskChain {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerId,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][object]$TraceContext,
        [Parameter(Mandatory = $true)][string]$DockerExecutable,
        [Parameter(Mandatory = $true)][string]$Context,
        [int]$TimeoutSeconds = 45
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = $script:OtelAcceptanceMessages.EmptySpans
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            Copy-OtelAcceptanceTraces `
                -ContainerId $ContainerId `
                -Destination $Destination `
                -DockerExecutable $DockerExecutable `
                -Context $Context `
                -Quiet $true
            $spans = @(Get-OtelAcceptanceSpans -Path $Destination)
            return Assert-OtelAcceptanceTaskChain -Spans $spans -TraceContext $TraceContext
        }
        catch {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 500
    }
    throw ($script:OtelAcceptanceMessages.TraceAssertion -f (
        $script:OtelAcceptanceMessages.RecoveryChainTimeout -f $lastError
    ))
}

$scriptFile = (Resolve-Path -LiteralPath $PSCommandPath).Path
$scriptsDirectory = Split-Path -Parent $scriptFile
$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDirectory "..")).Path
$expectedScriptsDirectory = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "scripts"))
if (-not (Test-OtelAcceptanceSamePath -Actual $scriptsDirectory -Expected $expectedScriptsDirectory)) {
    throw $script:OtelAcceptanceMessages.ScriptLocation
}

$expectedHelperPath = [System.IO.Path]::GetFullPath(
    (Join-Path $scriptsDirectory "runtime_acceptance_0_7_support.ps1")
)
if (-not (Test-Path -LiteralPath $DockerHelperPath -PathType Leaf) -or -not (
    Test-OtelAcceptanceSamePath -Actual $DockerHelperPath -Expected $expectedHelperPath
)) {
    throw ($script:OtelAcceptanceMessages.HelperPath -f $DockerHelperPath)
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
    throw ($script:OtelAcceptanceMessages.RunDirectory -f $resolvedRunDirectory)
}
if (-not (Test-Path -LiteralPath $resolvedRunDirectory -PathType Container)) {
    throw ($script:OtelAcceptanceMessages.RunDirectory -f $resolvedRunDirectory)
}

$composeFile = Join-Path $repositoryRoot "docker-compose.test.yml"
$ownershipComposeFile = Join-Path $repositoryRoot "deploy/tests/runtime-acceptance-0-7-ownership.compose.yml"
$otelComposeFile = Join-Path $repositoryRoot "deploy/tests/runtime-acceptance-0-7-otel.compose.yml"
$collectorConfigFile = Join-Path $repositoryRoot "deploy/tests/otel-collector-runtime-acceptance-0-7.yaml"
$configDirectory = Join-Path $repositoryRoot "config"
foreach ($requiredPath in @(
    $composeFile,
    $ownershipComposeFile,
    $otelComposeFile,
    $collectorConfigFile,
    (Join-Path $repositoryRoot "Cargo.toml")
)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw ($script:OtelAcceptanceMessages.MissingFile -f $requiredPath)
    }
}

$metadataPath = Join-Path $resolvedRunDirectory "otel-run.json"
$transcriptPath = Join-Path $resolvedRunDirectory "otel-transcript.log"
$apiOutput = Join-Path $resolvedRunDirectory "api.stdout.log"
$apiError = Join-Path $resolvedRunDirectory "api.stderr.log"
$workerOutput = Join-Path $resolvedRunDirectory "worker.stdout.log"
$workerError = Join-Path $resolvedRunDirectory "worker.stderr.log"
$uploadEvidence = Join-Path $resolvedRunDirectory "healthy-upload.json"
$healthyExportEvidence = Join-Path $resolvedRunDirectory "healthy-export.json"
$healthyJobEvidence = Join-Path $resolvedRunDirectory "healthy-export-job.json"
$outageExportEvidence = Join-Path $resolvedRunDirectory "outage-export.json"
$outageJobEvidence = Join-Path $resolvedRunDirectory "outage-export-job.json"
$recoveryExportEvidence = Join-Path $resolvedRunDirectory "recovery-export.json"
$recoveryJobEvidence = Join-Path $resolvedRunDirectory "recovery-export-job.json"
$apiMetricsBeforePath = Join-Path $resolvedRunDirectory "api-metrics-before.prom"
$workerMetricsBeforePath = Join-Path $resolvedRunDirectory "worker-metrics-before.prom"
$apiMetricsOutagePath = Join-Path $resolvedRunDirectory "api-metrics-outage.prom"
$workerMetricsOutagePath = Join-Path $resolvedRunDirectory "worker-metrics-outage.prom"
$apiReadinessOutagePath = Join-Path $resolvedRunDirectory "api-readiness-outage.txt"
$workerReadinessOutagePath = Join-Path $resolvedRunDirectory "worker-readiness-outage.txt"
$healthyTracePath = Join-Path $resolvedRunDirectory "traces-healthy.jsonl"
$finalTracePath = Join-Path $resolvedRunDirectory "traces-recovered.jsonl"
$collectorLogPath = Join-Path $resolvedRunDirectory "collector.log"
foreach ($evidencePath in @(
    $metadataPath,
    $transcriptPath,
    $apiOutput,
    $apiError,
    $workerOutput,
    $workerError,
    $uploadEvidence,
    $healthyExportEvidence,
    $healthyJobEvidence,
    $outageExportEvidence,
    $outageJobEvidence,
    $recoveryExportEvidence,
    $recoveryJobEvidence,
    $apiMetricsBeforePath,
    $workerMetricsBeforePath,
    $apiMetricsOutagePath,
    $workerMetricsOutagePath,
    $apiReadinessOutagePath,
    $workerReadinessOutagePath,
    $healthyTracePath,
    $finalTracePath,
    $collectorLogPath
)) {
    if (Test-Path -LiteralPath $evidencePath) {
        throw ($script:OtelAcceptanceMessages.EvidenceExists -f $evidencePath)
    }
}

$ports = Get-OtelAcceptancePorts -Names @(
    "mysql",
    "redis",
    "rustfs",
    "collector_http",
    "collector_health",
    "api",
    "worker"
)
$metadata = [ordered]@{
    schema_version = 1
    stage = "otel"
    status = "starting"
    started_at = [DateTime]::UtcNow.ToString("o")
    completed_at = $null
    docker_project = $ProjectName
    ownership_token = $OwnershipToken
    docker_context = $DockerContext
    run_directory = $resolvedRunDirectory
    collector_image = "otel/opentelemetry-collector-contrib:0.132.0"
    images = @()
    ports = $ports
    traces = [ordered]@{
        upload = $null
        healthy_task = $null
        outage_task_id = $null
        recovered_task = $null
    }
    exporter_failures = [ordered]@{
        api_before = $null
        api_outage = $null
        worker_before = $null
        worker_outage = $null
    }
    collector_fault = [ordered]@{
        method = "docker_stop_start"
        interrupted = $false
        restored = $false
    }
    assertions = [ordered]@{
        external_traceparent_restored = $false
        external_tracestate_restored = $false
        http_sql_redis_storage_chain = $false
        task_worker_outbox_chain = $false
        outage_api_ready = $false
        outage_worker_ready = $false
        outage_business_succeeded = $false
        exporter_failure_metrics_increased = $false
        recovered_trace_exported = $false
    }
    evidence = [ordered]@{
        healthy_traces = $healthyTracePath
        recovered_traces = $finalTracePath
        collector_log = $collectorLogPath
    }
    evidence_capture_errors = @()
    error = $null
    cleanup_errors = @()
}
Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

$runError = $null
$runSucceeded = $false
$cleanupErrors = New-Object System.Collections.Generic.List[string]
$evidenceCaptureErrors = New-Object System.Collections.Generic.List[string]
$transcriptStarted = $false
$dockerOwned = $false
$collectorFault = $null
$apiProcess = $null
$workerProcess = $null
$apiBinary = $null
$workerBinary = $null
$resolvedDockerExecutable = $null
$collectorContainerId = $null
$originalLocation = (Get-Location).Path
$locationChanged = $false
$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot

try {
    Start-Transcript -LiteralPath $transcriptPath | Out-Null
    $transcriptStarted = $true
    Set-Location -LiteralPath $repositoryRoot
    $locationChanged = $true
    Assert-OtelAcceptancePortsAvailable -Ports $ports

    $resolvedDockerExecutable = (Resolve-Path -LiteralPath $DockerExecutable).Path
    $contextInfo = Get-RyFrameV07LocalDockerContext -DockerExecutable $resolvedDockerExecutable
    if ($contextInfo.Name -cne $DockerContext) {
        throw ($script:OtelAcceptanceMessages.ContextMismatch -f $contextInfo.Name, $DockerContext)
    }
    $metadata["docker_server_version"] = Get-RyFrameV07DockerServerVersion `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["status"] = "running"
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $existingAppVariables = @(
        [System.Environment]::GetEnvironmentVariables("Process").Keys |
            Where-Object {
                $_ -is [string] `
                -and $_.StartsWith("APP_", [System.StringComparison]::Ordinal)
            }
    )
    foreach ($name in $existingAppVariables) {
        [System.Environment]::SetEnvironmentVariable($name, $null, "Process")
    }
    $existingOtelVariables = @(
        [System.Environment]::GetEnvironmentVariables("Process").Keys |
            Where-Object {
                $_ -is [string] `
                -and $_.StartsWith("OTEL_", [System.StringComparison]::Ordinal)
            }
    )
    foreach ($name in $existingOtelVariables) {
        [System.Environment]::SetEnvironmentVariable($name, $null, "Process")
    }
    foreach ($name in @("ADMIN_USER", "ADMIN_PASS", "TENANT_ID", "SNOWFLAKE_WORKER_ID")) {
        [System.Environment]::SetEnvironmentVariable($name, $null, "Process")
    }
    Set-OtelAcceptanceEnvironment -Name "RUST_LOG" -Value "info"
    Set-OtelAcceptanceEnvironment -Name "OTEL_BSP_SCHEDULE_DELAY" -Value "1000"

    Set-OtelAcceptanceEnvironment -Name "RYFRAME_TEST_MYSQL_PORT" -Value $ports.mysql.ToString()
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_TEST_REDIS_PORT" -Value $ports.redis.ToString()
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_TEST_RUSTFS_PORT" -Value $ports.rustfs.ToString()
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_TEST_MYSQL_ADMIN_URL" `
        -Value "mysql://root:ryframe_test_password@127.0.0.1:$($ports.mysql)/mysql"
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_V07_OTEL_HTTP_PORT" `
        -Value $ports.collector_http.ToString()
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_V07_OTEL_HEALTH_PORT" `
        -Value $ports.collector_health.ToString()
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_V07_OTEL_COLLECTOR_CONFIG" `
        -Value ([System.IO.Path]::GetFullPath($collectorConfigFile))
    Set-OtelAcceptanceEnvironment -Name "RYFRAME_V07_OWNERSHIP_TOKEN" -Value $OwnershipToken
    Set-OtelAcceptanceEnvironment -Name "NO_PROXY" -Value "127.0.0.1,localhost"

    $composeArguments = @(
        "compose",
        "--project-name", $ProjectName,
        "--file", $composeFile,
        "--file", $ownershipComposeFile,
        "--file", $otelComposeFile
    )
    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments ($composeArguments + @("config", "--quiet")) `
        -Description $script:OtelAcceptanceMessages.ComposeValidate
    Assert-RyFrameV07ProjectEmpty `
        -ProjectName $ProjectName `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $dockerOwned = $true
    Invoke-RyFrameV07DockerChecked `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext `
        -Arguments ($composeArguments + @(
            "up", "-d", "--wait", "mysql", "redis", "rustfs", "otel-collector"
        )) `
        -Description $script:OtelAcceptanceMessages.ComposeStart
    $imageEvidence = @(Get-RyFrameV07ProjectImageEvidence `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext)
    $imageServices = @($imageEvidence | ForEach-Object { [string]$_.service } | Sort-Object)
    if (
        $imageEvidence.Count -ne 4 `
        -or ($imageServices -join ",") -cne "mysql,otel-collector,redis,rustfs"
    ) {
        throw ($script:OtelAcceptanceMessages.ImageEvidence -f ($imageServices -join ","))
    }
    $metadata["images"] = $imageEvidence
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath
    $collectorContainerId = Resolve-RyFrameV07ServiceContainer `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -ComposeFile $otelComposeFile `
        -Service "otel-collector" `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["collector_container_id"] = $collectorContainerId
    Wait-OtelAcceptanceUri `
        -Uri "http://127.0.0.1:$($ports.collector_health)/" `
        -Label "OpenTelemetry Collector"

    $binarySuffix = if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) { ".exe" } else { "" }
    $debugDirectory = Join-Path $targetDirectory "debug"
    $apiBinary = Join-Path $debugDirectory "ryframe$binarySuffix"
    $workerBinary = Join-Path $debugDirectory "ryframe-worker$binarySuffix"
    $resetBinary = Join-Path $debugDirectory "ryframe-db-reset$binarySuffix"
    $migrateBinary = Join-Path $debugDirectory "ryframe-migrate$binarySuffix"
    foreach ($binary in @($apiBinary, $workerBinary, $resetBinary, $migrateBinary)) {
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
            throw ($script:OtelAcceptanceMessages.MissingBinary -f $binary)
        }
    }
    $apiBinary = (Resolve-Path -LiteralPath $apiBinary).Path
    $workerBinary = (Resolve-Path -LiteralPath $workerBinary).Path
    $resetBinary = (Resolve-Path -LiteralPath $resetBinary).Path
    $migrateBinary = (Resolve-Path -LiteralPath $migrateBinary).Path

    Set-OtelAcceptanceEnvironment -Name "APP_CONFIG_DIR" -Value $configDirectory
    Set-OtelAcceptanceEnvironment -Name "APP_ENV" -Value "test"
    Set-OtelAcceptanceEnvironment -Name "APP_APP_HOST" -Value "127.0.0.1"
    Set-OtelAcceptanceEnvironment -Name "APP_APP_PORT" -Value $ports.api.ToString()
    Set-OtelAcceptanceEnvironment -Name "APP_API_DOCS_ENABLED" -Value "false"
    Set-OtelAcceptanceEnvironment -Name "APP_MONITOR_METRICS_BEARER_TOKEN" -Value ""
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_HOST" -Value "127.0.0.1"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_PORT" -Value $ports.mysql.ToString()
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_NAME" -Value "ryframe_test"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_USERNAME" -Value "root"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_PASSWORD" -Value "ryframe_test_password"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_TLS_MODE" -Value "disabled"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_MIGRATION_MODE" -Value "verify"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_REPLICAS" -Value "[]"
    Set-OtelAcceptanceEnvironment -Name "APP_DATABASE_SOURCES" -Value "[]"
    Set-OtelAcceptanceEnvironment -Name "APP_REDIS_MODE" -Value "required"
    Set-OtelAcceptanceEnvironment -Name "APP_REDIS_HOST" -Value "127.0.0.1"
    Set-OtelAcceptanceEnvironment -Name "APP_REDIS_PORT" -Value $ports.redis.ToString()
    Set-OtelAcceptanceEnvironment -Name "APP_REDIS_PASSWORD" -Value ""
    Set-OtelAcceptanceEnvironment -Name "APP_REDIS_DATABASE" -Value "0"
    Set-OtelAcceptanceEnvironment -Name "APP_REDIS_TLS" -Value "false"
    Set-OtelAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_BACKEND" -Value "rustfs"
    Set-OtelAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_ENDPOINT" `
        -Value "http://127.0.0.1:$($ports.rustfs)"
    Set-OtelAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_ACCESS_KEY" -Value "ryframe-test-access"
    Set-OtelAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_SECRET_KEY" `
        -Value "ryframe-test-secret-2026"
    Set-OtelAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_USE_SSL" -Value "false"
    Set-OtelAcceptanceEnvironment -Name "APP_OBJECT_STORAGE_REGION" -Value "us-east-1"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_MODE" -Value "external"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_POLL_INTERVAL_MS" -Value "100"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_LEASE_SECONDS" -Value "30"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_HEARTBEAT_SECONDS" -Value "5"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_CONCURRENCY" -Value "1"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_HEALTH_HOST" -Value "127.0.0.1"
    Set-OtelAcceptanceEnvironment -Name "APP_JOBS_HEALTH_PORT" -Value $ports.worker.ToString()
    Set-OtelAcceptanceEnvironment -Name "APP_AUTH_JWT_SECRET" `
        -Value "ryframe-v07-otel-acceptance-jwt-secret-2026"
    Set-OtelAcceptanceEnvironment -Name "APP_MESSAGING_ENABLED" -Value "false"
    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_ENABLED" -Value "true"
    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_ENDPOINT" `
        -Value "http://127.0.0.1:$($ports.collector_http)/v1/traces"
    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_SAMPLE_RATIO" -Value "1"
    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_EXPORT_TIMEOUT_SECS" -Value "1"
    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_MAX_QUEUE_SIZE" -Value "2048"
    Set-OtelAcceptanceEnvironment -Name "APP_LOGGER_OUTPUT" -Value "stdout"
    Set-OtelAcceptanceEnvironment -Name "APP_LOGGER_FORMAT" -Value "text"
    Set-OtelAcceptanceEnvironment -Name "APP_LOGGER_LEVEL" -Value "info"
    Set-OtelAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "0"

    Invoke-OtelAcceptanceCommand `
        -Executable $resetBinary `
        -Arguments @("--database", "ryframe_test", "--confirm-reset", "RESET-RYFRAME-DATABASE") `
        -Description $script:OtelAcceptanceMessages.ResetDatabase
    Invoke-OtelAcceptanceCommand `
        -Executable $migrateBinary `
        -Arguments @("status") `
        -Description $script:OtelAcceptanceMessages.MigrationStatus
    Invoke-OtelAcceptanceCommand `
        -Executable $migrateBinary `
        -Arguments @("verify") `
        -Description $script:OtelAcceptanceMessages.MigrationVerify

    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_SERVICE_NAME" -Value "ryframe-worker-v07"
    Set-OtelAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "921"
    $workerProcess = Start-OtelAcceptanceProcess `
        -Executable $workerBinary `
        -Arguments @() `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $workerOutput `
        -StandardErrorLog $workerError
    Wait-OtelAcceptanceProcessReadiness `
        -Uri "http://127.0.0.1:$($ports.worker)/readyz" `
        -Process $workerProcess `
        -ExpectedExecutable $workerBinary `
        -Label "Worker"

    Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_SERVICE_NAME" -Value "ryframe-api-v07"
    Set-OtelAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "922"
    $apiProcess = Start-OtelAcceptanceProcess `
        -Executable $apiBinary `
        -Arguments @() `
        -WorkingDirectory $repositoryRoot `
        -StandardOutputLog $apiOutput `
        -StandardErrorLog $apiError
    Wait-OtelAcceptanceProcessReadiness `
        -Uri "http://127.0.0.1:$($ports.api)/readyz" `
        -Process $apiProcess `
        -ExpectedExecutable $apiBinary `
        -Label "API"

    $apiBase = [uri]"http://127.0.0.1:$($ports.api)/"
    $apiMetricsUri = [uri]"http://127.0.0.1:$($ports.api)/api/v1/monitor/metrics"
    $workerMetricsUri = [uri]"http://127.0.0.1:$($ports.worker)/metrics"
    $login = Invoke-OtelAcceptanceLogin -ApiBase $apiBase

    $uploadTrace = New-OtelAcceptanceTraceContext
    [void](Invoke-OtelAcceptanceUpload `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -Traceparent $uploadTrace.Header `
        -Tracestate $uploadTrace.TraceState `
        -EvidencePath $uploadEvidence)

    $healthyTaskTrace = New-OtelAcceptanceTraceContext
    $healthyJobId = Invoke-OtelAcceptanceExport `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -Traceparent $healthyTaskTrace.Header `
        -Tracestate $healthyTaskTrace.TraceState `
        -IdempotencyKey ("otel-healthy-{0}" -f [guid]::NewGuid().ToString("N")) `
        -EvidencePath $healthyExportEvidence
    [void](Wait-OtelAcceptanceExport `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -JobId $healthyJobId `
        -EvidencePath $healthyJobEvidence)

    [void](Wait-OtelAcceptanceCollectorTraces `
        -ContainerId $collectorContainerId `
        -TraceIds @($uploadTrace.TraceId, $healthyTaskTrace.TraceId) `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext)
    [void](Wait-OtelAcceptanceHealthyChains `
        -ContainerId $collectorContainerId `
        -Destination $healthyTracePath `
        -UploadTraceContext $uploadTrace `
        -TaskTraceContext $healthyTaskTrace `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext)

    $apiMetricsBefore = Get-OtelAcceptanceMetrics -Uri $apiMetricsUri -Label "API"
    $workerMetricsBefore = Get-OtelAcceptanceMetrics -Uri $workerMetricsUri -Label "Worker"
    Write-OtelAcceptanceText -Path $apiMetricsBeforePath -Content $apiMetricsBefore.Text
    Write-OtelAcceptanceText -Path $workerMetricsBeforePath -Content $workerMetricsBefore.Text
    $metadata["exporter_failures"]["api_before"] = $apiMetricsBefore.FailureCount
    $metadata["exporter_failures"]["worker_before"] = $workerMetricsBefore.FailureCount

    $collectorFault = Stop-RyFrameV07DockerService `
        -ProjectName $ProjectName `
        -OwnershipToken $OwnershipToken `
        -ComposeFile $otelComposeFile `
        -Service "otel-collector" `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["collector_fault"]["interrupted"] = $true
    Copy-OtelAcceptanceTraces `
        -ContainerId $collectorContainerId `
        -Destination $healthyTracePath `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $healthySpans = @(Get-OtelAcceptanceSpans -Path $healthyTracePath)
    $uploadChain = Assert-OtelAcceptanceUploadChain -Spans $healthySpans -TraceContext $uploadTrace
    $healthyTaskChain = Assert-OtelAcceptanceTaskChain `
        -Spans $healthySpans `
        -TraceContext $healthyTaskTrace
    $metadata["traces"]["upload"] = $uploadChain
    $metadata["traces"]["healthy_task"] = $healthyTaskChain
    $metadata["assertions"]["external_traceparent_restored"] = $true
    $metadata["assertions"]["external_tracestate_restored"] = $true
    $metadata["assertions"]["http_sql_redis_storage_chain"] = $true
    $metadata["assertions"]["task_worker_outbox_chain"] = $true
    Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath

    $outageTaskTrace = New-OtelAcceptanceTraceContext
    $outageJobId = Invoke-OtelAcceptanceExport `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -Traceparent $outageTaskTrace.Header `
        -Tracestate $outageTaskTrace.TraceState `
        -IdempotencyKey ("otel-outage-{0}" -f [guid]::NewGuid().ToString("N")) `
        -EvidencePath $outageExportEvidence
    [void](Wait-OtelAcceptanceExport `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -JobId $outageJobId `
        -EvidencePath $outageJobEvidence)
    $metadata["traces"]["outage_task_id"] = $outageTaskTrace.TraceId
    $metadata["assertions"]["outage_business_succeeded"] = $true

    $apiReadiness = Invoke-OtelAcceptanceWebRequest `
        -Uri ([uri]"http://127.0.0.1:$($ports.api)/readyz") `
        -ExpectedStatus 200 `
        -Label $script:OtelAcceptanceMessages.OutageApiReady
    $workerReadiness = Invoke-OtelAcceptanceWebRequest `
        -Uri ([uri]"http://127.0.0.1:$($ports.worker)/readyz") `
        -ExpectedStatus 200 `
        -Label $script:OtelAcceptanceMessages.OutageWorkerReady
    Write-OtelAcceptanceText -Path $apiReadinessOutagePath -Content $apiReadiness.Content
    Write-OtelAcceptanceText -Path $workerReadinessOutagePath -Content $workerReadiness.Content
    $metadata["assertions"]["outage_api_ready"] = $true
    $metadata["assertions"]["outage_worker_ready"] = $true

    $outageMetrics = Wait-OtelAcceptanceFailureMetrics `
        -ApiMetricsUri $apiMetricsUri `
        -WorkerMetricsUri $workerMetricsUri `
        -ApiBefore $apiMetricsBefore.FailureCount `
        -WorkerBefore $workerMetricsBefore.FailureCount
    Write-OtelAcceptanceText -Path $apiMetricsOutagePath -Content $outageMetrics.Api.Text
    Write-OtelAcceptanceText -Path $workerMetricsOutagePath -Content $outageMetrics.Worker.Text
    $metadata["exporter_failures"]["api_outage"] = $outageMetrics.Api.FailureCount
    $metadata["exporter_failures"]["worker_outage"] = $outageMetrics.Worker.FailureCount
    $metadata["assertions"]["exporter_failure_metrics_increased"] = $true

    Restore-RyFrameV07DockerFault `
        -Fault $collectorFault `
        -OwnershipToken $OwnershipToken `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $collectorFault = $null
    Wait-OtelAcceptanceUri `
        -Uri "http://127.0.0.1:$($ports.collector_health)/" `
        -Label $script:OtelAcceptanceMessages.CollectorRecovered
    $metadata["collector_fault"]["restored"] = $true

    $recoveryTaskTrace = New-OtelAcceptanceTraceContext
    $recoveryJobId = Invoke-OtelAcceptanceExport `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -Traceparent $recoveryTaskTrace.Header `
        -Tracestate $recoveryTaskTrace.TraceState `
        -IdempotencyKey ("otel-recovery-{0}" -f [guid]::NewGuid().ToString("N")) `
        -EvidencePath $recoveryExportEvidence
    [void](Wait-OtelAcceptanceExport `
        -ApiBase $apiBase `
        -AccessToken $login.AccessToken `
        -JobId $recoveryJobId `
        -EvidencePath $recoveryJobEvidence)
    $collectorLog = Wait-OtelAcceptanceCollectorTraces `
        -ContainerId $collectorContainerId `
        -TraceIds @($recoveryTaskTrace.TraceId) `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    Write-OtelAcceptanceText -Path $collectorLogPath -Content $collectorLog
    $recoveredTaskChain = Wait-OtelAcceptanceTaskChain `
        -ContainerId $collectorContainerId `
        -Destination $finalTracePath `
        -TraceContext $recoveryTaskTrace `
        -DockerExecutable $resolvedDockerExecutable `
        -Context $DockerContext
    $metadata["traces"]["recovered_task"] = $recoveredTaskChain
    $metadata["assertions"]["recovered_trace_exported"] = $true
    $runSucceeded = $true
}
catch {
    $runError = $_
    $metadata["error"] = $_.Exception.Message
}
finally {
    if ($null -ne $collectorFault) {
        try {
            Restore-RyFrameV07DockerFault `
                -Fault $collectorFault `
                -OwnershipToken $OwnershipToken `
                -DockerExecutable $DockerExecutable `
                -Context $DockerContext
        }
        catch {
            $cleanupErrors.Add(($script:OtelAcceptanceMessages.CollectorRestore -f $_.Exception.Message))
        }
    }

    if ($dockerOwned -and $null -ne $collectorContainerId) {
        if (-not (Test-Path -LiteralPath $collectorLogPath -PathType Leaf)) {
            try {
                $collectorLog = @(Invoke-RyFrameV07DockerLines `
                    -DockerExecutable $DockerExecutable `
                    -Context $DockerContext `
                    -Arguments @("container", "logs", $collectorContainerId)) -join `
                    [Environment]::NewLine
                Write-OtelAcceptanceText -Path $collectorLogPath -Content $collectorLog
            }
            catch {
                $evidenceCaptureErrors.Add(($script:OtelAcceptanceMessages.CollectorLogEvidence -f $_.Exception.Message))
            }
        }
        if (-not (Test-Path -LiteralPath $healthyTracePath -PathType Leaf)) {
            try {
                Copy-OtelAcceptanceTraces `
                    -ContainerId $collectorContainerId `
                    -Destination $healthyTracePath `
                    -DockerExecutable $DockerExecutable `
                    -Context $DockerContext `
                    -Quiet $true
            }
            catch {
                $evidenceCaptureErrors.Add(($script:OtelAcceptanceMessages.CollectorTraceEvidence -f $_.Exception.Message))
            }
        }
    }

    foreach ($processInfo in @(
        @($apiProcess, $apiBinary, "API"),
        @($workerProcess, $workerBinary, "Worker")
    )) {
        if ($null -eq $processInfo[0] -or $null -eq $processInfo[1]) {
            continue
        }
        try {
            Stop-OtelAcceptanceProcess `
                -Process $processInfo[0] `
                -ExpectedExecutable $processInfo[1] `
                -Label $processInfo[2]
        }
        catch {
            $cleanupErrors.Add(($script:OtelAcceptanceMessages.ProcessCleanup -f $processInfo[2], $_.Exception.Message))
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
            $cleanupErrors.Add(($script:OtelAcceptanceMessages.DockerCleanup -f $_.Exception.Message))
        }
    }

    if ($locationChanged) {
        try {
            Set-Location -LiteralPath $originalLocation
        }
        catch {
            $cleanupErrors.Add(($script:OtelAcceptanceMessages.DirectoryRestore -f $_.Exception.Message))
        }
    }
    try {
        Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot
    }
    catch {
        $cleanupErrors.Add(($script:OtelAcceptanceMessages.EnvironmentRestore -f $_.Exception.Message))
    }
    if ($transcriptStarted) {
        try {
            Stop-Transcript | Out-Null
        }
        catch {
            $cleanupErrors.Add(($script:OtelAcceptanceMessages.TranscriptCleanup -f $_.Exception.Message))
        }
    }

    $metadata["completed_at"] = [DateTime]::UtcNow.ToString("o")
    $metadata["evidence_capture_errors"] = @($evidenceCaptureErrors)
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
        $metadataError = $script:OtelAcceptanceMessages.MetadataWrite -f $_.Exception.Message
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
Write-Host ("`n" + ($script:OtelAcceptanceMessages.Success -f $resolvedRunDirectory))
