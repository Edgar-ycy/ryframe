from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / "scripts" / "runtime_acceptance_0_7.ps1"
STAGE = ROOT / "scripts" / "runtime_acceptance_0_7_replica.ps1"
SUPPORT = ROOT / "scripts" / "runtime_acceptance_0_7_support.ps1"
CLIENT = ROOT / "scripts" / "replica_runtime_acceptance_client.mjs"
COMPOSE = ROOT / "scripts" / "runtime_acceptance_0_7_replica.compose.yml"
DATASOURCE = ROOT / "crates" / "ryframe" / "src" / "boot" / "datasource.rs"
CLUSTER = ROOT / "crates" / "ryframe-db" / "src" / "cluster.rs"
CORE_MONITOR = ROOT / "crates" / "ryframe-core" / "src" / "database_monitor.rs"
API_ROUTER = ROOT / "crates" / "ryframe-api" / "src" / "router.rs"
OPENAPI = ROOT / "openapi" / "openapi.json"


class RuntimeAcceptanceV07ReplicaPolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.entry = ENTRY.read_text(encoding="utf-8")
        cls.stage = STAGE.read_text(encoding="utf-8")
        cls.support = SUPPORT.read_text(encoding="utf-8")
        cls.client = CLIENT.read_text(encoding="utf-8")
        cls.compose = COMPOSE.read_text(encoding="utf-8")
        cls.datasource = DATASOURCE.read_text(encoding="utf-8")
        cls.cluster = CLUSTER.read_text(encoding="utf-8")
        cls.core_monitor = CORE_MONITOR.read_text(encoding="utf-8")
        cls.api_router = API_ROUTER.read_text(encoding="utf-8")
        cls.openapi = json.loads(OPENAPI.read_text(encoding="utf-8"))

    def test_entry_wires_the_replica_stage_once(self) -> None:
        self.assertEqual(
            self.entry.count("runtime_acceptance_0_7_replica.ps1"),
            1,
        )
        self.assertIn('name = "replica"', self.entry)

    def test_stage_requires_exact_opt_in_before_side_effects(self) -> None:
        guard = 'if ($ConfirmRun -cne "RUN-RYFRAME-V0-7-STAGE")'
        self.assertEqual(self.stage.count(guard), 1)
        guard_position = self.stage.index(guard)
        for fragment in (
            "Write-RyFrameV07MetadataAtomically",
            "Start-Transcript",
            "Start-Process",
            '"up", "-d", "--wait"',
        ):
            self.assertLess(guard_position, self.stage.index(fragment))

    def test_compose_uses_project_scoped_durable_volumes_and_loopback_ports(self) -> None:
        for service in ("mysql-primary:", "mysql-replica:"):
            self.assertEqual(self.compose.count(service), 1)
        self.assertEqual(self.compose.count("image: mysql:8.4"), 2)
        self.assertIn("- mysql-primary-data:/var/lib/mysql", self.compose)
        self.assertIn("- mysql-replica-data:/var/lib/mysql", self.compose)
        self.assertIn('"127.0.0.1:${RYFRAME_V07_PRIMARY_PORT}:3306"', self.compose)
        self.assertIn('"127.0.0.1:${RYFRAME_V07_REPLICA_PORT}:3306"', self.compose)
        self.assertNotIn("container_name", self.compose)
        self.assertNotIn("deploy/compose.prod", self.compose)
        self.assertRegex(self.compose, r"(?m)^volumes:\s*$")
        self.assertIn("  mysql-primary-data:", self.compose)
        self.assertIn("  mysql-replica-data:", self.compose)
        self.assertIn("networks:\n  default:", self.compose)
        owner_label = (
            'com.ryframe.runtime-acceptance-owner: '
            '"${RYFRAME_V07_OWNERSHIP_TOKEN:?}"'
        )
        self.assertEqual(self.compose.count(owner_label), 5)
        self.assertNotIn("external:", self.compose)

    def test_stage_reuses_binaries_without_compiling(self) -> None:
        for binary in ("ryframe", "ryframe-db-reset", "ryframe-migrate"):
            self.assertIn(binary, self.stage)
        self.assertNotIn('Get-ReplicaAcceptanceCommand -Name "cargo"', self.stage)
        self.assertNotRegex(self.stage, r'(?i)\bcargo\s+(?:build|run|test|check)\b')
        self.assertNotIn('"build", "--locked"', self.stage)

    def test_fault_injection_is_docker_native_scoped_and_restored(self) -> None:
        for fragment in (
            "Stop-RyFrameV07DockerService",
            "Restore-RyFrameV07DockerFault",
            '-Service "mysql-replica"',
            "Remove-RyFrameV07DockerProjectResources",
            'method = "docker_stop_start"',
        ):
            self.assertIn(fragment, self.stage)
        self.assertIn(
            '@("container", "stop", "--time", "10", $containerId)',
            self.support,
        )
        self.assertIn('@("container", "start", $containerId)', self.support)
        self.assertNotIn("toxiproxy", self.stage.lower())

    def test_all_five_replica_states_are_fail_closed(self) -> None:
        calls = re.findall(
            r'\["phases"\]\["([^"]+)"\]\s*=\s*Invoke-ReplicaAcceptanceClient',
            self.stage,
        )
        self.assertEqual(
            calls,
            [
                "initial_healthy",
                "replica_stopped",
                "replica_recovered",
                "ledger_lag",
                "ledger_repaired",
            ],
        )
        expected_states = re.findall(
            r'-ExpectedState\s+"(healthy|fallback)"',
            self.stage,
        )
        self.assertEqual(
            expected_states,
            ["healthy", "fallback", "healthy", "fallback", "healthy"],
        )
        self.assertEqual(self.stage.count("-StabilitySeconds 12"), 1)
        for fragment in (
            'if ($exitCode -ne 0)',
            "-not (Test-Path -LiteralPath $EvidencePath -PathType Leaf)",
            '$evidence.status -ne "passed"',
            '$metadata["status"] = "failed"',
        ):
            self.assertIn(fragment, self.stage)

    def test_eventual_reads_prove_replica_data_and_metric_deltas(self) -> None:
        for fragment in (
            "/api/v1/system/loginlogs?",
            "options.sentinelUser",
            "options.sentinelId",
            'delta(after, before, "replica", "replica")',
            'delta(after, before, "primary", "fallback")',
            "fallback_total_delta",
            "routing.sentinel_count !== 1",
            "routing.sentinel_count !== 0",
        ):
            self.assertIn(fragment, self.client)
        for fragment in (
            "INSERT INTO sys_login_info",
            "ryframe_v07_replica_marker",
            "2099-12-31 23:59:59",
        ):
            self.assertIn(fragment, self.stage)

    def test_strong_reads_prove_primary_data_and_never_select_replica(self) -> None:
        for fragment in (
            "/api/v1/auth/profile",
            "nickname === options.replicaNickname",
            'delta(after, before, "primary", "strong")',
            "routing.primary_strong_delta < 2",
            "routing.replica_delta !== 0",
        ):
            self.assertIn(fragment, self.client)
        self.assertIn(
            "UPDATE sys_user SET nickname = '$replicaNickname'",
            self.stage,
        )

    def test_ledger_only_lag_is_rejected_then_repaired(self) -> None:
        for fragment in (
            "SELECT version FROM seaql_migrations",
            "DELETE FROM seaql_migrations WHERE version = '$ledgerVersion'",
            "INSERT INTO seaql_migrations (version, applied_at)",
            '$metadata["ledger_lag"]["rejected"] = $true',
            '$metadata["ledger_lag"]["repaired"] = $true',
            '$metadata["ledger_lag"]["rejoined"] = $true',
        ):
            self.assertIn(fragment, self.stage)
        self.assertNotRegex(self.stage, r"(?i)ALTER\s+TABLE")
        self.assertIn("ryframe_db_migration::verify(db)", self.datasource)
        self.assertNotIn("verify_current_schema(db)", self.datasource)

    def test_stability_observation_cannot_pass_without_continuous_eviction(self) -> None:
        for fragment in (
            "options.stabilitySeconds * 1000",
            "while (Date.now() < deadline)",
            "if (!topologyMatches(database, options.expectedState))",
            "Math.max(3, Math.floor(options.stabilitySeconds / 2))",
            "observations < minimumObservations",
        ):
            self.assertIn(fragment, self.client)

    def test_probe_streak_snapshot_is_read_only_and_sources_are_zeroed(self) -> None:
        for fragment in (
            "pub consecutive_failures: usize",
            "pub consecutive_successes: usize",
        ):
            self.assertIn(fragment, self.core_monitor)
        for fragment in (
            "consecutive_failures: self.consecutive_failures.load(Ordering::Acquire)",
            "consecutive_successes: self.consecutive_successes.load(Ordering::Acquire)",
            "consecutive_failures: 0",
            "consecutive_successes: 0",
        ):
            self.assertIn(fragment, self.cluster)
        snapshot_body = self.cluster.split("fn health(&self) -> DatabaseNodeHealth", 1)[1].split("}\n", 2)[0]
        self.assertNotIn("record_probe", snapshot_body)
        clear_body = self.cluster.split("fn clear_connection(&self)", 1)[1].split(
            "fn is_healthy", 1
        )[0]
        self.assertIn(
            "self.consecutive_successes.store(0, Ordering::Release)",
            clear_body,
        )
        for fragment in (
            "replica.clear_connection();",
            "assert_eq!(replica.health().consecutive_successes, 0);",
            "replica.replace_connection(DatabaseConnection::default());",
            "assert_eq!(replica.health().consecutive_successes, 1);",
        ):
            self.assertIn(fragment, self.cluster)

    def test_runtime_api_and_openapi_expose_both_replica_streaks(self) -> None:
        for fragment in (
            "consecutive_failures: replica.consecutive_failures",
            "consecutive_successes: replica.consecutive_successes",
            "consecutive_failures: usize",
            "consecutive_successes: usize",
        ):
            self.assertIn(fragment, self.api_router)
        schema = self.openapi["components"]["schemas"]["RuntimeDatabaseReplicaStatus"]
        for field in ("consecutive_failures", "consecutive_successes"):
            self.assertIn(field, schema["required"])
            self.assertEqual(schema["properties"][field]["type"], "integer")
            self.assertEqual(schema["properties"][field]["minimum"], 0)

    def test_failure_and_recovery_thresholds_are_observed_before_fault_actions(self) -> None:
        failure_start = self.stage.index('-ExpectedState "failure-threshold"')
        failure_ready = self.stage.index(
            "Wait-ReplicaAcceptanceThresholdObserverReady",
            failure_start,
        )
        replica_stop = self.stage.index("$replicaFault = Stop-RyFrameV07DockerService")
        failure_complete = self.stage.index(
            'Complete-ReplicaAcceptanceThresholdObserver `',
            replica_stop,
        )
        self.assertLess(failure_start, failure_ready)
        self.assertLess(failure_ready, replica_stop)
        self.assertLess(replica_stop, failure_complete)

        recovery_start = self.stage.index('-ExpectedState "recovery-threshold"')
        recovery_ready = self.stage.index(
            "Wait-ReplicaAcceptanceThresholdObserverReady",
            recovery_start,
        )
        replica_restore = self.stage.index("Restore-RyFrameV07DockerFault", recovery_ready)
        recovery_complete = self.stage.index(
            'Complete-ReplicaAcceptanceThresholdObserver `',
            replica_restore,
        )
        self.assertLess(recovery_start, recovery_ready)
        self.assertLess(recovery_ready, replica_restore)
        self.assertLess(replica_restore, recovery_complete)

        for fragment in (
            'const steps = kind === "failure" ? [1, 2, 3] : [1, 2]',
            'snapshot.connected === (step < 3)',
            'snapshot.connected === (step >= 2)',
            "错过副本 ${kind} 连续阈值",
            "连续探测在达到阈值前被中断",
            "if (thresholdStepMatches(kind, expectedStep, snapshot))",
            "+ JSON.stringify(lastSnapshot)",
        ):
            self.assertIn(fragment, self.client)
        self.assertNotIn("阈值状态不符合预期", self.client)

    def test_node_client_is_bound_to_exact_local_inputs_and_exclusive_evidence(self) -> None:
        for fragment in (
            'const INTERNAL_TOKEN = "RUN-RYFRAME-V0-7-REPLICA-CLIENT"',
            "fileURLToPath(import.meta.url)",
            '"target", "runtime-acceptance-0-7"',
            'rawApiBase !== apiBase.origin',
            'apiBase.hostname !== "127.0.0.1"',
            "assertPathWithinAcceptanceTarget(evidenceRoot",
            "realpath(ACCEPTANCE_TARGET_ROOT)",
            "证据文件必须是指定证据根目录的直接子文件",
            "await link(temporaryPath, filePath)",
            'error?.code === "EEXIST"',
        ):
            self.assertIn(fragment, self.client)
        self.assertIn('"--internal-token", $script:ReplicaClientInternalToken', self.stage)
        self.assertIn('"--evidence-root", $evidenceRoot', self.stage)

    def test_process_lifecycle_uses_captured_owned_process_objects(self) -> None:
        self.assertIn("ConvertTo-RyFrameV07ProcessArgument -Value $_", self.stage)
        self.assertIn(') -join " ")', self.stage)
        self.assertNotIn("ArgumentList = $arguments", self.stage)
        for forbidden in (
            "Get-Process -Id",
            "Stop-Process -Id",
            "Wait-Process -Id",
        ):
            self.assertNotIn(forbidden, self.stage)
        assert_body = self.stage.split(
            "function Assert-ReplicaAcceptanceProcessIdentity", 1
        )[1].split("function Stop-ReplicaAcceptanceProcess", 1)[0]
        self.assertIn("Get-RyFrameV07OwnedProcess", assert_body)
        self.assertIn("return $current", assert_body)

        stop_body = self.stage.split("function Stop-ReplicaAcceptanceProcess", 1)[1].split(
            "function Wait-ReplicaAcceptanceReadiness", 1
        )[0]
        first_validation = stop_body.index("Get-RyFrameV07OwnedProcess")
        graceful_stop = stop_body.index("Stop-Process -InputObject $ownedProcess")
        graceful_wait = stop_body.index("$Process.WaitForExit(10000)")
        second_validation = stop_body.index(
            "Get-RyFrameV07OwnedProcess",
            first_validation + 1,
        )
        forced_stop = stop_body.index(
            "Stop-Process -InputObject $ownedProcess -Force",
        )
        forced_wait = stop_body.rindex("$Process.WaitForExit(10000)")
        self.assertLess(first_validation, graceful_stop)
        self.assertLess(graceful_stop, graceful_wait)
        self.assertLess(graceful_wait, second_validation)
        self.assertLess(second_validation, forced_stop)
        self.assertLess(forced_stop, forced_wait)

    def test_process_environment_is_fully_restored_before_terminal_metadata(self) -> None:
        snapshot = self.stage.index(
            "$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot"
        )
        stage_try = self.stage.index("\ntry {", snapshot)
        first_mutation = self.stage.index(
            "[System.Environment]::SetEnvironmentVariable",
            stage_try,
        )
        restore = self.stage.rindex(
            "Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot"
        )
        cleanup_errors = self.stage.rindex('$metadata["cleanup_errors"] = @($cleanupErrors)')
        terminal_write = self.stage.rindex(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        self.assertLess(snapshot, stage_try)
        self.assertLess(stage_try, first_mutation)
        self.assertLess(first_mutation, restore)
        self.assertLess(restore, cleanup_errors)
        self.assertLess(cleanup_errors, terminal_write)
        self.assertIn(
            "$script:ReplicaAcceptanceMessages.EnvironmentRestore -f $_.Exception.Message",
            self.stage,
        )

    def test_context_mismatch_and_project_images_are_recorded_fail_closed(self) -> None:
        self.assertIn(
            "$script:ReplicaAcceptanceMessages.ContextMismatch -f $contextInfo.Name, $DockerContext",
            self.stage,
        )
        self.assertNotIn(
            "$script:ReplicaAcceptanceMessages.HelperPath -f $DockerContext",
            self.stage,
        )
        compose_up = self.stage.index(
            '@("compose", "--project-name", $ProjectName, "--file", $composeFile, "up", "-d", "--wait")'
        )
        image_evidence = self.stage.index(
            "$imageEvidence = @(Get-RyFrameV07ProjectImageEvidence",
            compose_up,
        )
        image_guard = self.stage.index(
            '$imageEvidence.Count -ne 2',
            image_evidence,
        )
        image_metadata = self.stage.index(
            '$metadata["images"] = $imageEvidence',
            image_guard,
        )
        atomic_write = self.stage.index(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath",
            image_metadata,
        )
        self.assertIn("images = @()", self.stage)
        self.assertLess(compose_up, image_evidence)
        self.assertLess(image_evidence, image_guard)
        self.assertLess(image_guard, image_metadata)
        self.assertLess(image_metadata, atomic_write)
        self.assertIn('"mysql-primary,mysql-replica"', self.stage)

    def test_docker_project_resources_require_one_validated_ownership_token(self) -> None:
        parameter = re.search(
            r"\[Parameter\(Mandatory = \$true\)\]\s*\[string\]\$OwnershipToken",
            self.stage,
        )
        self.assertIsNotNone(parameter)
        token_validation = self.stage.index(
            "Assert-RyFrameV07OwnershipToken -OwnershipToken $OwnershipToken"
        )
        snapshot = self.stage.index(
            "$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot"
        )
        token_environment = self.stage.index(
            'Set-ReplicaAcceptanceEnvironment -Name "RYFRAME_V07_OWNERSHIP_TOKEN" '
            "-Value $OwnershipToken"
        )
        compose_config = self.stage.index(
            '@("compose", "--project-name", $ProjectName, "--file", $composeFile, "config", "--quiet")'
        )
        assert_empty = self.stage.index("Assert-RyFrameV07ProjectEmpty", compose_config)
        docker_owned = self.stage.index("$dockerOwned = $true", assert_empty)
        compose_up = self.stage.index(
            '@("compose", "--project-name", $ProjectName, "--file", $composeFile, "up", "-d", "--wait")',
            docker_owned,
        )
        self.assertLess(token_validation, snapshot)
        self.assertLess(snapshot, token_environment)
        self.assertLess(token_environment, compose_config)
        self.assertLess(compose_config, assert_empty)
        self.assertLess(assert_empty, docker_owned)
        self.assertLess(docker_owned, compose_up)

        for command in (
            "Get-RyFrameV07ProjectImageEvidence",
            "Resolve-RyFrameV07ServiceContainer",
            "Stop-RyFrameV07DockerService",
            "Restore-RyFrameV07DockerFault",
            "Remove-RyFrameV07DockerProjectResources",
        ):
            matches = list(re.finditer(rf"{command}\s+`", self.stage))
            self.assertGreater(len(matches), 0, command)
            for match in matches:
                call = self.stage[match.start() : match.start() + 500]
                self.assertIn("-OwnershipToken $OwnershipToken", call, command)

    def test_metadata_and_cleanup_preserve_failure_evidence(self) -> None:
        for fragment in (
            'stage = "replica"',
            'status = "starting"',
            '$metadata["status"] = "running"',
            '$metadata["completed_at"] =',
            '$metadata["cleanup_errors"] = @($cleanupErrors)',
            "$ledgerRemoved = $false",
            "if ($null -ne $replicaFault)",
        ):
            self.assertIn(fragment, self.stage)
        self.assertNotIn("Remove-Item", self.stage)
        cleanup = self.stage.rindex("Remove-RyFrameV07DockerProjectResources")
        terminal_write = self.stage.rindex(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        self.assertLess(cleanup, terminal_write)

    def test_messages_decode_to_chinese(self) -> None:
        match = re.search(r"ConvertFrom-Json @'\n(.*?)\n'@", self.stage, re.DOTALL)
        self.assertIsNotNone(match)
        messages = json.loads(match.group(1))
        for message in messages.values():
            self.assertRegex(message, r"[\u4e00-\u9fff]")

    def test_powershell_ast_is_valid_when_host_is_available(self) -> None:
        executable_name = "powershell.exe" if os.name == "nt" else "pwsh"
        powershell = shutil.which(executable_name)
        if powershell is None:
            self.skipTest(f"未找到 {executable_name}，跳过 PowerShell AST 验证")
        quoted = "'" + str(STAGE).replace("'", "''") + "'"
        command = (
            "$tokens=$null;$errors=$null;"
            "[void][System.Management.Automation.Language.Parser]::ParseFile("
            f"{quoted},[ref]$tokens,[ref]$errors);"
            "$errors|ForEach-Object{Write-Error $_.Message};"
            "if($errors.Count -gt 0){exit 1}"
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

    def test_node_client_syntax_is_valid_when_node_is_available(self) -> None:
        node = shutil.which("node")
        if node is None:
            self.skipTest("未找到 node，跳过客户端语法验证")
        result = subprocess.run(
            [node, "--check", str(CLIENT)],
            cwd=ROOT,
            check=False,
            capture_output=True,
            timeout=30,
        )
        output = (result.stdout + result.stderr).decode(errors="replace")
        self.assertEqual(result.returncode, 0, output)

    def test_node_client_rejects_unsafe_inputs_before_network_access(self) -> None:
        node = shutil.which("node")
        if node is None:
            self.skipTest("未找到 node，跳过客户端输入边界验证")

        run_root = (
            ROOT / "target" / "runtime-acceptance-0-7" / "client-policy-test"
        ).resolve()
        evidence = run_root / "evidence.json"
        common = [
            node,
            str(CLIENT),
            "--api-base",
            "http://127.0.0.1:9",
            "--evidence",
            str(evidence),
            "--evidence-root",
            str(run_root),
            "--expected-state",
            "healthy",
            "--internal-token",
            "RUN-RYFRAME-V0-7-REPLICA-CLIENT",
            "--sentinel-user",
            "sentinel",
            "--sentinel-id",
            "1",
            "--replica-nickname",
            "replica",
        ]

        cases = []
        invalid_token = common.copy()
        invalid_token[invalid_token.index("RUN-RYFRAME-V0-7-REPLICA-CLIENT")] = "wrong"
        cases.append((invalid_token, "内部确认令牌不匹配"))
        invalid_url = common.copy()
        invalid_url[invalid_url.index("http://127.0.0.1:9")] = "http://localhost:9"
        cases.append((invalid_url, "API 地址必须精确"))
        outside = common.copy()
        outside[outside.index(str(evidence))] = str(ROOT / "outside-evidence.json")
        cases.append((outside, "必须位于 target/runtime-acceptance-0-7 内"))

        for arguments, expected in cases:
            result = subprocess.run(
                arguments,
                cwd=ROOT,
                check=False,
                capture_output=True,
                timeout=10,
            )
            output = (result.stdout + result.stderr).decode(errors="replace")
            self.assertNotEqual(result.returncode, 0, output)
            self.assertIn(expected, output)


if __name__ == "__main__":
    unittest.main()
