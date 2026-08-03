import json
import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "runtime_acceptance.ps1"
COMPOSE = ROOT / "docker-compose.test.yml"
EXPORT_ACCEPTANCE = (
    ROOT / "crates" / "ryframe-service" / "tests" / "export_runtime_acceptance_test.rs"
)
METADATA_WRITER_SELF_TEST = ROOT / "scripts" / "tests" / "runtime_metadata_writer_test.ps1"


class RuntimeAcceptancePolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SCRIPT.read_text(encoding="utf-8")
        cls.compose = COMPOSE.read_text(encoding="utf-8")
        cls.export_acceptance = EXPORT_ACCEPTANCE.read_text(encoding="utf-8")

    def test_script_has_no_partial_execution_switch(self) -> None:
        self.assertIsNone(re.search(r"\$(?:Skip\w*|Stage\w*)\b", self.source, re.IGNORECASE))
        self.assertNotIn("cargo run", self.source)

    def test_runtime_is_pinned_to_test_and_loopback(self) -> None:
        required_fragments = [
            'Set-RuntimeEnvironmentVariable -Name "APP_ENV" -Value "test"',
            'Set-RuntimeEnvironmentVariable -Name "APP_CONFIG_DIR"',
            'Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_HOST" -Value "127.0.0.1"',
            'Set-RuntimeEnvironmentVariable -Name "APP_REDIS_HOST" -Value "127.0.0.1"',
            'Set-RuntimeEnvironmentVariable -Name "APP_JOBS_MODE" -Value "external"',
            'Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_REPLICAS" -Value "[]"',
            'Set-RuntimeEnvironmentVariable -Name "APP_DATABASE_SOURCES" -Value "[]"',
            "Assert-LocalDockerContext",
            "Assert-DockerDaemonAvailable",
            'if ($endpoint -notmatch "^(npipe|unix)://")',
        ]
        for fragment in required_fragments:
            self.assertIn(fragment, self.source)

        for port in ("3306", "6379", "9000"):
            self.assertRegex(
                self.compose,
                rf'"127\.0\.0\.1:\$\{{RYFRAME_TEST_[A-Z]+_PORT:-\d+\}}:{port}"',
            )

    def test_docker_daemon_is_checked_before_compose_ownership(self) -> None:
        function_start = self.source.index("function Assert-DockerDaemonAvailable")
        function_end = self.source.index("\nfunction ", function_start + 1)
        function_source = self.source[function_start:function_end]
        for fragment in (
            "--context $Context",
            "info",
            '--format "{{ .ServerVersion }}"',
            '$ErrorActionPreference = "Continue"',
            "$serverExitCode = $LASTEXITCODE",
            "$serverExitCode -ne 0",
            "[string]::IsNullOrWhiteSpace($serverVersion)",
            "$script:RuntimeMessages.DockerDaemonUnavailable",
        ):
            self.assertIn(fragment, function_source)

        daemon_probe = self.source.index(
            "$dockerServerVersion = Assert-DockerDaemonAvailable"
        )
        running_metadata = self.source.index('$metadata["status"] = "running"')
        compose_owned = self.source.index("$composeOwned = $true")
        compose_up = self.source.index(
            '-Arguments ($composeArguments + @("up", "-d", "--wait"))'
        )
        self.assertLess(daemon_probe, running_metadata)
        self.assertLess(running_metadata, compose_owned)
        self.assertLess(compose_owned, compose_up)
        self.assertIn("docker_server_version = $null", self.source)
        self.assertIn(
            '$metadata["docker_server_version"] = $dockerServerVersion',
            self.source,
        )

    def test_workspace_tests_run_before_runtime_app_overrides(self) -> None:
        workspace_test = self.source.index(
            '"test", "--locked", "--workspace", "--no-fail-fast", "--", "--test-threads=1"'
        )
        clear_app_environment = self.source.index("$existingAppVariables = @(")
        first_app_override = self.source.index(
            'Set-RuntimeEnvironmentVariable -Name "APP_'
        )
        rustfs_test_execution = self.source.index(
            "foreach ($testArguments in $rustFsTestCommands)"
        )
        app_environment = self.source.index(
            'Set-RuntimeEnvironmentVariable -Name "APP_ENV" -Value "test"'
        )

        self.assertLess(clear_app_environment, workspace_test)
        self.assertNotIn(
            'Set-RuntimeEnvironmentVariable -Name "APP_',
            self.source[:workspace_test],
        )
        self.assertLess(workspace_test, first_app_override)
        self.assertLess(first_app_override, rustfs_test_execution)
        self.assertLess(rustfs_test_execution, app_environment)
        self.assertIn(
            'Remove-RuntimeEnvironmentVariable -Name $name',
            self.source[:workspace_test],
        )

    def test_compose_project_and_cleanup_are_symmetric(self) -> None:
        self.assertIn('$projectName = "ryframe-runtime-$runId".ToLowerInvariant()', self.source)
        self.assertGreaterEqual(self.source.count('"--project-name", $projectName'), 2)
        self.assertGreaterEqual(self.source.count('"--file", $composeFile'), 2)
        self.assertIn('"down", "--volumes", "--remove-orphans"', self.source)
        self.assertIn("finally {", self.source)

    def test_ports_and_processes_are_fail_closed(self) -> None:
        self.assertGreaterEqual(self.source.count("Assert-LoopbackPortsAvailable"), 3)
        self.assertIn("Assert-RecordedProcessIdentity", self.source)
        self.assertIn("Test-SameExecutablePath", self.source)
        self.assertIn("Stop-Process -Id $RecordedProcess.Id", self.source)
        self.assertNotRegex(self.source, r"Stop-Process\s+-Name")
        self.assertNotRegex(self.source, r"Get-Process\s+(?:ryframe|\*)")

    def test_readiness_probe_uses_basic_parsing(self) -> None:
        self.assertEqual(
            self.source.count(
                "Invoke-WebRequest -Uri $Uri.AbsoluteUri -TimeoutSec 2 -UseBasicParsing"
            ),
            1,
        )

    def test_required_runtime_stages_are_present_and_serial(self) -> None:
        build = '"build", "--locked", "-p", "ryframe", "--bins"'
        self.assertEqual(self.source.count(build), 1)
        self.assertEqual(
            self.source.count(
                '"test", "--locked", "--workspace", "--no-fail-fast", "--", "--test-threads=1"'
            ),
            1,
        )
        additional_tests = [
            "refresh_session_redis_test",
            "stale_touch_cannot_resurrect_or_overwrite_online_user_index",
            "integration_test",
            "test_s3_integration_put_get_delete",
            "export_runtime_acceptance_covers_scale_takeover_storage_recovery_and_cleanup",
        ]
        positions = [self.source.index(name) for name in additional_tests]
        self.assertEqual(positions, sorted(positions))
        self.assertIn(
            '"test", "--locked", "-p", "ryframe-service", "--lib"',
            self.source,
        )
        self.assertIn(
            "system::online_user_service::redis_backend::tests::"
            "stale_touch_cannot_resurrect_or_overwrite_online_user_index",
            self.source,
        )
        self.assertNotIn("mysql_migration_test", self.source)
        self.assertNotIn("outbox_worker_test", self.source)
        self.assertNotIn("export_service_test", self.source)
        self.assertIn(
            "$totalTestCount = $testCommands.Count + $rustFsTestCommands.Count",
            self.source,
        )
        self.assertIn('"RESET-RYFRAME-DATABASE"', self.source)
        self.assertIn('-Arguments @("status")', self.source)
        self.assertIn('-Arguments @("verify")', self.source)
        self.assertIn("Start-RuntimeProcess", self.source)
        self.assertIn("smoke-test.js", self.source)

    def test_export_runtime_acceptance_has_one_exact_opt_in_invocation(self) -> None:
        command = re.compile(
            r'@\(\s*"test",\s*"--locked",\s*"-p",\s*"ryframe-service",\s*'
            r'"--test",\s*"export_runtime_acceptance_test",\s*'
            r'"export_runtime_acceptance_covers_scale_takeover_storage_recovery_and_cleanup",\s*'
            r'"--",\s*"--exact",\s*"--ignored",\s*"--test-threads=1"\s*\)',
            re.DOTALL,
        )
        matches = list(command.finditer(self.source))

        self.assertEqual(len(matches), 1)
        self.assertEqual(self.source.count('"export_runtime_acceptance_test"'), 1)
        self.assertEqual(
            self.source.count(
                '"export_runtime_acceptance_covers_scale_takeover_storage_recovery_and_cleanup"'
            ),
            1,
        )
        rustfs_test = self.source.index("test_s3_integration_put_get_delete")
        rustfs_test_execution = self.source.index(
            "foreach ($testArguments in $rustFsTestCommands)"
        )
        database_reset = self.source.index(
            "-Description $script:RuntimeMessages.ResetDatabase"
        )
        self.assertLess(rustfs_test, matches[0].start())
        self.assertLess(matches[0].end(), rustfs_test_execution)
        self.assertLess(rustfs_test_execution, database_reset)

    def test_export_runtime_rustfs_environment_is_isolated_and_loopback_only(self) -> None:
        workspace_test = self.source.index(
            '"test", "--locked", "--workspace", "--no-fail-fast", "--", "--test-threads=1"'
        )
        definitions = (
            '$rustFsEndpoint = "http://127.0.0.1:$RustFsPort"',
            '$rustFsAccessKey = "ryframe-test-access"',
            '$rustFsSecretKey = "ryframe-test-secret-2026"',
            '$rustFsRegion = "us-east-1"',
            '$loopbackNoProxy = "127.0.0.1,localhost"',
        )
        for definition in definitions:
            self.assertEqual(self.source.count(definition), 1)

        assignments = {
            "RYFRAME_TEST_RUSTFS_ENDPOINT": "$rustFsEndpoint",
            "RYFRAME_TEST_RUSTFS_ACCESS_KEY": "$rustFsAccessKey",
            "RYFRAME_TEST_RUSTFS_SECRET_KEY": "$rustFsSecretKey",
            "RYFRAME_TEST_RUSTFS_REGION": "$rustFsRegion",
            "NO_PROXY": "$loopbackNoProxy",
        }
        for name, value in assignments.items():
            assignment = (
                f'Set-RuntimeEnvironmentVariable -Name "{name}" -Value {value}'
            )
            self.assertEqual(self.source.count(assignment), 1)
            self.assertLess(self.source.index(assignment), workspace_test)

        setter_start = self.source.index("function Set-RuntimeEnvironmentVariable")
        setter_end = self.source.index("\nfunction ", setter_start + 1)
        setter = self.source[setter_start:setter_end]
        self.assertIn("Save-RuntimeEnvironmentVariable -Name $Name", setter)
        self.assertIn("Restore-RuntimeEnvironment", self.source)

        for fragment in (
            ".no_proxy()",
            "reqwest::Url::parse(endpoint)",
            'let expected = format!("http://127.0.0.1:{port}");',
            "if endpoint != expected",
            '"https://127.0.0.1:19000"',
            '"http://localhost:19000"',
            '"http://192.168.1.10:19000"',
            '"http://127.0.0.1:19000/path"',
        ):
            self.assertIn(fragment, self.export_acceptance)
        self.assertNotIn("APP_OBJECT_STORAGE_", self.export_acceptance)

    def test_logs_are_scoped_to_a_unique_run_directory(self) -> None:
        self.assertIn('Join-Path $targetDirectory "runtime-acceptance"', self.source)
        self.assertIn("[guid]::NewGuid()", self.source)
        self.assertIn('Join-Path $runDirectory "worker.stdout.log"', self.source)
        self.assertIn('Join-Path $runDirectory "api.stdout.log"', self.source)
        self.assertNotRegex(self.source, r"Remove-Item\b")

    def test_metadata_records_terminal_status_after_cleanup(self) -> None:
        for field in (
            'status = "starting"',
            "started_at =",
            "completed_at = $null",
            "error = $null",
            "cleanup_errors = @()",
            '$metadata["status"] = "running"',
            '$metadata["completed_at"] =',
            '$metadata["cleanup_errors"] = @($cleanupErrors)',
            "Resolve-RuntimeTerminalStatus",
            "$script:RuntimeMessages.MetadataWrite",
        ):
            self.assertIn(field, self.source)

        smoke = self.source.index("-Description $script:RuntimeMessages.RunSmoke")
        succeeded = self.source.index("$runSucceeded = $true")
        finally_block = self.source.rindex("finally {")
        terminal_write = self.source.rindex(
            "Write-RuntimeMetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        success_message = self.source.rindex(
            'Write-Host ("`n" + ($script:RuntimeMessages.Success'
        )
        self.assertLess(smoke, succeeded)
        self.assertLess(succeeded, finally_block)
        self.assertLess(finally_block, terminal_write)
        self.assertLess(terminal_write, success_message)
        self.assertEqual(
            self.source.count(
                "Write-RuntimeMetadataAtomically -Metadata $metadata -Path $metadataPath"
            ),
            3,
        )
        self.assertNotIn("Set-Content -LiteralPath $metadataPath", self.source)

    def test_terminal_status_and_metadata_failure_preserve_primary_error(self) -> None:
        status_start = self.source.index("function Resolve-RuntimeTerminalStatus")
        status_end = self.source.index("\nfunction ", status_start + 1)
        status_source = self.source[status_start:status_end]
        run_error = status_source.index("if ($HasRunError)")
        cleanup_error = status_source.index("if ($CleanupErrorCount -gt 0)")
        succeeded = status_source.index("if ($RunSucceeded)")
        self.assertLess(run_error, cleanup_error)
        self.assertLess(cleanup_error, succeeded)
        self.assertIn('return "cleanup_failed"', status_source)
        self.assertGreaterEqual(status_source.count('return "failed"'), 2)
        self.assertIn('return "passed"', status_source)

        terminal_write = self.source.rindex(
            "Write-RuntimeMetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        terminal_source = self.source[terminal_write:]
        self.assertIn("catch {", terminal_source)
        self.assertIn("if ($null -eq $runError)", terminal_source)
        self.assertIn("$cleanupErrors.Add($metadataWriteError)", terminal_source)
        self.assertLess(
            terminal_source.index("if ($null -ne $runError)"),
            terminal_source.index("if ($cleanupErrors.Count -gt 0)"),
        )

    def test_metadata_replacement_is_atomic_and_cleans_temporary_artifacts(self) -> None:
        writer_start = self.source.index("function Write-RuntimeMetadataAtomically")
        writer_end = self.source.index("\nfunction ", writer_start + 1)
        writer_source = self.source[writer_start:writer_end]
        for fragment in (
            "[System.IO.FileStream]::new(",
            "$stream.Flush($true)",
            "[System.IO.Path]::GetFullPath($Path)",
            "$backupPath =",
            "[System.IO.File]::Replace($temporaryPath, $destinationPath, $backupPath, $true)",
            "[System.IO.File]::Move($temporaryPath, $destinationPath)",
            "$committed = $true",
            "foreach ($cleanupPath in @($temporaryPath, $backupPath))",
            "[System.IO.File]::Delete($ArtifactPath)",
            "& $ArtifactDeleter $cleanupPath",
            "[System.Text.UTF8Encoding]::new($false)",
            "$primaryError = $_",
            "throw $primaryError",
            "throw $cleanupError",
            "$null -ne $cleanupError -and -not $committed",
            "$script:RuntimeMessages.MetadataArtifactCleanup",
        ):
            self.assertIn(fragment, writer_source)
        self.assertNotIn("[System.IO.File]::Replace($temporaryPath, $Path, $null)", writer_source)
        self.assertLess(
            writer_source.index("if ($null -ne $primaryError)"),
            writer_source.index("if ($null -ne $cleanupError -and -not $committed)"),
        )
        self.assertLess(
            writer_source.index("if ($null -ne $cleanupError -and -not $committed)"),
            writer_source.rindex("if ($null -ne $cleanupError)"),
        )

    def test_metadata_writer_runs_twice_in_available_powershell(self) -> None:
        self_test = METADATA_WRITER_SELF_TEST.read_text(encoding="utf-8")
        for fragment in (
            '$PSVersionTable.PSEdition -ne "Desktop"',
            "$PSVersionTable.PSVersion.Major -ne 5",
            "$PSVersionTable.PSVersion.Minor -ne 1",
            'node.Name -eq "Write-RuntimeMetadataAtomically"',
            ". $writerDefinition",
            "Write-RuntimeMetadataAtomically -Metadata $firstMetadata -Path $metadataPath",
            "Write-RuntimeMetadataAtomically -Metadata $secondMetadata -Path $metadataPath",
            "$raw | ConvertFrom-Json",
            "$hasBom",
            "$bytes[$bytes.Length - 1] -ne 0x0A",
            "$files.Count -ne 1",
            '$files[0].Name -ne "run.json"',
            '-ArtifactDeleter $artifactDeleter',
            '-WarningVariable cleanupWarnings',
            '$cleanupWarnings.Count -ne 1',
            '".bak"',
        ):
            self.assertIn(fragment, self_test)

        executable_name = "powershell.exe" if os.name == "nt" else "pwsh"
        powershell = shutil.which(executable_name)
        if powershell is None:
            self.skipTest(f"未找到 {executable_name}，跳过动态 PowerShell 回归")

        result = subprocess.run(
            [
                powershell,
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(METADATA_WRITER_SELF_TEST),
            ],
            cwd=ROOT,
            check=False,
            capture_output=True,
            timeout=30,
        )
        output = (result.stdout + result.stderr).decode(errors="replace")
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("runtime metadata writer self-test passed", output)

    def test_runtime_messages_are_chinese_without_source_encoding_risk(self) -> None:
        match = re.search(
            r"\$script:RuntimeMessages = ConvertFrom-Json @'\n(.*?)\n'@",
            self.source,
            re.DOTALL,
        )
        self.assertIsNotNone(match)
        messages = json.loads(match.group(1))
        for name in (
            "ValidateCompose",
            "StartDependencies",
            "BuildBinaries",
            "RunTest",
            "ResetDatabase",
            "MigrationStatus",
            "MigrationVerify",
            "RunSmoke",
            "DockerCleanup",
            "MetadataWrite",
            "DockerDaemonUnavailable",
        ):
            self.assertRegex(messages[name], r"[\u4e00-\u9fff]")
        self.assertNotRegex(self.source, r'-Description\s+"[A-Za-z]')

    def test_repository_paths_are_composed_by_segment(self) -> None:
        self.assertNotIn('"deploy\\tests', self.source)
        self.assertNotIn('"target\\runtime-acceptance', self.source)
        self.assertIn('Join-Path $deployDirectory "tests"', self.source)
        self.assertIn(
            'Join-Path $serviceTestsDirectory "export_runtime_acceptance_test.rs"',
            self.source,
        )


if __name__ == "__main__":
    unittest.main()
