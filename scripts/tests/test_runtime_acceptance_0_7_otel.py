import json
import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "runtime_acceptance_0_7_otel.ps1"
SUPPORT = ROOT / "scripts" / "runtime_acceptance_0_7_support.ps1"
COMPOSE = ROOT / "deploy" / "tests" / "runtime-acceptance-0-7-otel.compose.yml"
OWNERSHIP_COMPOSE = (
    ROOT / "deploy" / "tests" / "runtime-acceptance-0-7-ownership.compose.yml"
)
COLLECTOR = ROOT / "deploy" / "tests" / "otel-collector-runtime-acceptance-0-7.yaml"


class RuntimeAcceptanceV07OtelPolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.script = SCRIPT.read_text(encoding="utf-8")
        cls.support = SUPPORT.read_text(encoding="utf-8")
        cls.compose = COMPOSE.read_text(encoding="utf-8")
        cls.ownership_compose = OWNERSHIP_COMPOSE.read_text(encoding="utf-8")
        cls.collector = COLLECTOR.read_text(encoding="utf-8")

    def test_stage_contract_is_exact_and_opt_in_precedes_side_effects(self) -> None:
        parameter_block = self.script[
            self.script.index("param(") : self.script.index(")\n\nSet-StrictMode")
        ]
        for name in (
            "ConfirmRun",
            "ProjectName",
            "OwnershipToken",
            "RunDirectory",
            "DockerExecutable",
            "DockerContext",
            "DockerHelperPath",
        ):
            self.assertEqual(parameter_block.count(f"${name}"), 1)
        self.assertNotIn("[switch]", parameter_block)
        self.assertNotRegex(parameter_block, r"\b(?:Skip|Only|Stage)\w*")

        guard = 'if ($ConfirmRun -cne "RUN-RYFRAME-V0-7-STAGE")'
        self.assertEqual(self.script.count(guard), 1)
        guard_position = self.script.index(guard)
        for fragment in (
            ". $expectedHelperPath",
            "Write-RyFrameV07MetadataAtomically",
            "Start-Transcript",
        ):
            self.assertLess(guard_position, self.script.index(fragment))

    def test_paths_and_docker_resources_are_fail_closed_and_scoped(self) -> None:
        for fragment in (
            '"runtime_acceptance_0_7_support.ps1"',
            'Join-Path $targetDirectory "runtime-acceptance-0-7"',
            "$resolvedRunDirectory.StartsWith($targetPrefix, $runPathComparison)",
            'Join-Path $resolvedRunDirectory "otel-run.json"',
            "Assert-RyFrameV07ProjectName -ProjectName $ProjectName",
            "Assert-OtelAcceptancePortsAvailable -Ports $ports",
            "Get-RyFrameV07LocalDockerContext",
            "Resolve-RyFrameV07ServiceContainer",
            "Get-RyFrameV07ProjectImageEvidence",
            "Assert-RyFrameV07ProjectEmpty",
            "Remove-RyFrameV07DockerProjectResources",
        ):
            self.assertIn(fragment, self.script)
        self.assertIn("Assert-RyFrameV07ResourceProjectLabel", self.support)
        self.assertNotIn("Remove-Item", self.script)
        self.assertNotIn("docker system prune", self.script.lower())
        self.assertNotIn("toxiproxy", self.script.lower())
        self.assertIn("ContextMismatch", self.script)
        self.assertIn("images = @()", self.script)
        self.assertIn("$imageEvidence = @(Get-RyFrameV07ProjectImageEvidence", self.script)
        self.assertIn('$metadata["images"] = $imageEvidence', self.script)
        self.assertIn(
            '($imageServices -join ",") -cne "mysql,otel-collector,redis,rustfs"',
            self.script,
        )
        self.assertIn("-OwnershipToken $OwnershipToken", self.script)
        self.assertIn("ownership_token = $OwnershipToken", self.script)
        self.assertIn('"--file", $ownershipComposeFile', self.script)

    def test_every_compose_resource_has_an_ownership_label(self) -> None:
        label = "com.ryframe.runtime-acceptance-owner"
        self.assertEqual(self.ownership_compose.count(label), 5)
        self.assertEqual(self.compose.count(label), 2)
        self.assertEqual(
            (self.ownership_compose + self.compose).count(
                '"${RYFRAME_V07_OWNERSHIP_TOKEN:?}"'
            ),
            7,
        )
        for resource in (
            "mysql:",
            "redis:",
            "rustfs:",
            "default:",
            "redis-test-data:",
            "otel-collector:",
            "otel-runtime-traces:",
        ):
            self.assertIn(resource, self.ownership_compose + self.compose)

    def test_real_collector_is_pinned_and_exports_machine_readable_evidence(self) -> None:
        self.assertEqual(
            self.compose.count(
                "image: otel/opentelemetry-collector-contrib:0.132.0"
            ),
            1,
        )
        self.assertNotRegex(self.compose, r"(?m)^\s*image:\s*.*:latest\s*$")
        for fragment in (
            "RYFRAME_V07_OTEL_HTTP_PORT",
            "RYFRAME_V07_OTEL_HEALTH_PORT",
            "RYFRAME_V07_OTEL_COLLECTOR_CONFIG",
            "otel-runtime-traces:/var/lib/otel",
            'restart: "no"',
        ):
            self.assertIn(fragment, self.compose)

        for fragment in (
            "health_check:",
            "otlp:",
            "http:",
            "file/runtime:",
            "path: /var/lib/otel/traces.jsonl",
            "debug/runtime:",
            "verbosity: detailed",
            "receivers:",
            "processors:",
            "exporters:",
        ):
            self.assertIn(fragment, self.collector)

    def test_external_parent_and_dependency_chain_are_asserted_from_collector_data(self) -> None:
        upload_assertion = self.script[
            self.script.index("function Assert-OtelAcceptanceUploadChain") :
            self.script.index("function Assert-OtelAcceptanceTaskChain")
        ]
        for fragment in (
            'Route "/api/v1/common/upload"',
            "$http.ParentSpanId -ne $TraceContext.ParentSpanId",
            "$http.TraceState -cne $TraceContext.TraceState",
            '$_.Attributes.ContainsKey("db.system")',
            '[string]$_.Attributes["db.system"] -eq "mysql"',
            '[string]$_.Attributes["db.system"] -eq "redis"',
            '$_.Attributes.ContainsKey("storage.backend")',
            "Test-OtelAcceptanceDescendant",
        ):
            self.assertIn(fragment, upload_assertion)

        execution = self.script[
            self.script.index("try {", self.script.index("$locationChanged = $false")) :
        ]
        wait_position = execution.index("Wait-OtelAcceptanceCollectorTraces")
        stop_position = execution.index("Stop-RyFrameV07DockerService")
        parse_position = execution.index("Get-OtelAcceptanceSpans -Path $healthyTracePath")
        self.assertLess(wait_position, stop_position)
        self.assertLess(stop_position, parse_position)

    def test_tracestate_is_injected_and_verified_across_worker_and_outbox(self) -> None:
        for fragment in (
            'TraceState = "ryframe=v07$($traceId.Substring(0, 12))"',
            '$headers["tracestate"] = $Tracestate',
            'TryAddWithoutValidation("tracestate", $Tracestate)',
            'TraceState = [string](Get-OtelAcceptanceProperty -Object $span -Name "traceState")',
            '$_.TraceState -cne $TraceContext.TraceState',
            'external_tracestate_restored = $false',
            '$metadata["assertions"]["external_tracestate_restored"] = $true',
        ):
            self.assertIn(fragment, self.script)
        self.assertGreaterEqual(
            self.script.count("-Tracestate $"),
            4,
        )

    def test_job_worker_and_outbox_parent_chain_are_mandatory(self) -> None:
        task_assertion = self.script[
            self.script.index("function Assert-OtelAcceptanceTaskChain") :
            self.script.index("$scriptFile =")
        ]
        for fragment in (
            'Route "/api/v1/system/users/exports"',
            "$http.ParentSpanId -ne $TraceContext.ParentSpanId",
            "$http.TraceState -cne $TraceContext.TraceState",
            '$_.ServiceName -eq "ryframe-worker-v07"',
            '$_.Name -eq "background_job"',
            '$_.Name -eq "outbox_event"',
            "Test-OtelAcceptanceDescendant",
        ):
            self.assertIn(fragment, task_assertion)
        self.assertIn(
            'Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_SERVICE_NAME" -Value "ryframe-worker-v07"',
            self.script,
        )
        self.assertIn(
            'Set-OtelAcceptanceEnvironment -Name "APP_TELEMETRY_SERVICE_NAME" -Value "ryframe-api-v07"',
            self.script,
        )

    def test_async_export_uses_only_the_current_accepted_contract(self) -> None:
        export = self.script[
            self.script.index("function Invoke-OtelAcceptanceExport") :
            self.script.index("function Wait-OtelAcceptanceExport")
        ]
        for fragment in (
            "-ExpectedStatus 202",
            "$json.code -ne 202",
            '$json.data.status -cne "queued"',
            '$json.data.resource -cne "users"',
        ):
            self.assertIn(fragment, export)
        self.assertNotIn("$json.code -ne 200", export)

    def test_collector_outage_preserves_readiness_and_business_but_increments_each_process(self) -> None:
        execution = self.script[
            self.script.index("try {", self.script.index("$locationChanged = $false")) :
        ]
        stop = execution.index("$collectorFault = Stop-RyFrameV07DockerService")
        outage_export = execution.index('$outageTaskTrace = New-OtelAcceptanceTraceContext')
        readiness = execution.index('$metadata["assertions"]["outage_api_ready"] = $true')
        failures = execution.index("Wait-OtelAcceptanceFailureMetrics")
        restore = execution.index("Restore-RyFrameV07DockerFault", stop)
        self.assertLess(stop, outage_export)
        self.assertLess(outage_export, readiness)
        self.assertLess(readiness, failures)
        self.assertLess(failures, restore)

        metric_waiter = self.script[
            self.script.index("function Wait-OtelAcceptanceFailureMetrics") :
            self.script.index("function Wait-OtelAcceptanceCollectorTraces")
        ]
        self.assertIn("$apiMetrics.FailureCount -gt $ApiBefore", metric_waiter)
        self.assertIn("$workerMetrics.FailureCount -gt $WorkerBefore", metric_waiter)
        self.assertIn('MetricNotIncreased -f "API"', metric_waiter)
        self.assertIn('MetricNotIncreased -f "Worker"', metric_waiter)
        self.assertIn('outage_business_succeeded"] = $true', execution)
        self.assertIn('/readyz")', execution)

    def test_recovery_requires_a_new_exported_trace_and_restores_every_fault(self) -> None:
        execution = self.script[
            self.script.index("try {", self.script.index("$locationChanged = $false")) :
        ]
        first_restore = execution.index("Restore-RyFrameV07DockerFault")
        recovery_trace = execution.index(
            "$recoveryTaskTrace = New-OtelAcceptanceTraceContext"
        )
        recovery_wait = execution.index(
            "-TraceIds @($recoveryTaskTrace.TraceId)", recovery_trace
        )
        recovery_assert = execution.index(
            "$recoveredTaskChain = Wait-OtelAcceptanceTaskChain", recovery_wait
        )
        self.assertLess(first_restore, recovery_trace)
        self.assertLess(recovery_trace, recovery_wait)
        self.assertLess(recovery_wait, recovery_assert)
        self.assertIn(
            '$metadata["assertions"]["recovered_trace_exported"] = $true',
            execution,
        )

        terminal = self.script[self.script.rindex("finally {") :]
        self.assertIn("if ($null -ne $collectorFault)", terminal)
        self.assertIn("Restore-RyFrameV07DockerFault", terminal)
        self.assertIn("Remove-RyFrameV07DockerProjectResources", terminal)

    def test_evidence_and_cleanup_failures_cannot_report_success(self) -> None:
        for fragment in (
            'status = "starting"',
            '$metadata["status"] = "running"',
            '$metadata["status"] = "failed"',
            '$metadata["status"] = "cleanup_failed"',
            '$metadata["status"] = "passed"',
            '$metadata["completed_at"] = [DateTime]::UtcNow.ToString("o")',
            '$metadata["cleanup_errors"] = @($cleanupErrors)',
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath",
            "$runError = [System.InvalidOperationException]::new($metadataError)",
            "if ($cleanupErrors.Count -gt 0)",
        ):
            self.assertIn(fragment, self.script)
        terminal = self.script[self.script.rindex("finally {") :]
        cleanup = terminal.index("Remove-RyFrameV07DockerProjectResources")
        metadata = terminal.rindex(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        success = terminal.rindex("$script:OtelAcceptanceMessages.Success")
        self.assertLess(cleanup, metadata)
        self.assertLess(metadata, success)
        self.assertNotIn("AllowFailure", self.script)
        self.assertNotIn("cargo test", self.script)
        self.assertNotIn("cargo build", self.script)
        self.assertNotIn('Get-OtelAcceptanceCommand -Name "cargo"', self.script)

    def test_process_identity_and_environment_are_restored_safely(self) -> None:
        for fragment in (
            "Get-RyFrameV07OwnedProcess",
            "Stop-Process -InputObject $current",
            "$Process.WaitForExit(10000)",
            "$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot",
            "Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot",
            "EnvironmentRestore",
            "ConvertTo-RyFrameV07ProcessArgument -Value $_",
            ') -join " ")',
        ):
            self.assertIn(fragment, self.script)
        self.assertNotIn("Stop-Process -Id", self.script)
        self.assertNotIn("Wait-Process -Id", self.script)
        terminal = self.script[self.script.rindex("finally {") :]
        restore = terminal.index("Restore-RyFrameV07ProcessEnvironmentSnapshot")
        metadata = terminal.rindex(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        self.assertLess(restore, metadata)

    def test_runtime_messages_decode_to_chinese(self) -> None:
        match = re.search(r"ConvertFrom-Json @'\n(.*?)\n'@", self.script, re.DOTALL)
        self.assertIsNotNone(match)
        messages = json.loads(match.group(1))
        for message in messages.values():
            self.assertRegex(message, r"[\u4e00-\u9fff]")

    def test_powershell_ast_is_valid_when_host_is_available(self) -> None:
        executable_name = "powershell.exe" if os.name == "nt" else "pwsh"
        powershell = shutil.which(executable_name)
        if powershell is None:
            self.skipTest(f"未找到 {executable_name}，跳过 PowerShell AST 验证")

        quoted_paths = ",".join(
            "'" + str(path).replace("'", "''") + "'" for path in (SCRIPT, SUPPORT)
        )
        command = (
            f"$files=@({quoted_paths});$failed=$false;"
            "foreach($file in $files){$tokens=$null;$errors=$null;"
            "[void][System.Management.Automation.Language.Parser]::ParseFile("
            "$file,[ref]$tokens,[ref]$errors);"
            "if($errors.Count -gt 0){$failed=$true;$errors|ForEach-Object{"
            "Write-Error $_.Message}}};if($failed){exit 1}"
        )
        arguments = [powershell, "-NoLogo", "-NoProfile", "-NonInteractive"]
        if os.name == "nt":
            arguments.extend(["-ExecutionPolicy", "Bypass"])
        arguments.extend(["-Command", command])
        result = subprocess.run(
            arguments,
            cwd=ROOT,
            check=False,
            capture_output=True,
            timeout=30,
        )
        output = (result.stdout + result.stderr).decode(errors="replace")
        self.assertEqual(result.returncode, 0, output)


if __name__ == "__main__":
    unittest.main()
