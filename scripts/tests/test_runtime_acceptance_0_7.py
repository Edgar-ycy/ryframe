import json
import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / "scripts" / "runtime_acceptance_0_7.ps1"
SUPPORT = ROOT / "scripts" / "runtime_acceptance_0_7_support.ps1"


class RuntimeAcceptanceV07PolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.entry = ENTRY.read_text(encoding="utf-8")
        cls.support = SUPPORT.read_text(encoding="utf-8")

    def test_entry_requires_exact_opt_in_before_side_effects(self) -> None:
        confirmation = '$requiredConfirmation = "RUN-RYFRAME-V0-7-ACCEPTANCE"'
        guard = 'if ($ConfirmRun -cne $requiredConfirmation)'
        self.assertEqual(self.entry.count(confirmation), 1)
        self.assertEqual(self.entry.count(guard), 1)
        guard_position = self.entry.index(guard)
        for fragment in (
            "New-Item -ItemType Directory",
            "Start-Transcript",
            "Get-Command docker",
            "$dockerOwnershipAcquired = $true",
        ):
            self.assertLess(guard_position, self.entry.index(fragment))

        parameter_block = self.entry[
            self.entry.index("param(") : self.entry.index(")\n\nSet-StrictMode")
        ]
        self.assertIn('[string]$ConfirmRun = ""', parameter_block)
        self.assertNotIn("[switch]", parameter_block)
        self.assertNotRegex(parameter_block, r"\b(?:Skip|Only|Stage)\w*")

    def test_stages_are_explicit_serial_and_fail_closed(self) -> None:
        stage_names = ['name = "message"', 'name = "replica"', 'name = "otel"']
        stage_positions = [self.entry.index(name) for name in stage_names]
        self.assertEqual(stage_positions, sorted(stage_positions))

        for script_name in (
            "runtime_acceptance_0_7_message.ps1",
            "runtime_acceptance_0_7_replica.ps1",
            "runtime_acceptance_0_7_otel.ps1",
        ):
            self.assertEqual(self.entry.count(script_name), 1)

        self.assertIn('status = "not_run"', self.entry)
        for evidence_file in ("message-run.json", "replica-run.json", "otel-run.json"):
            self.assertEqual(self.entry.count(evidence_file), 1)
        self.assertIn('$evidence.status -ceq "passed"', self.entry)
        self.assertIn('$evidence.docker_project -ceq $ProjectName', self.entry)
        self.assertIn('$evidence.ownership_token -ceq $OwnershipToken', self.entry)
        self.assertIn('@($evidence.cleanup_errors).Count -eq 0', self.entry)
        self.assertIn('$metadata["stages"][$index]["status"] = "running"', self.entry)
        self.assertIn('$metadata["stages"][$index]["status"] = "failed"', self.entry)
        self.assertIn('$metadata["stages"][$index]["status"] = "passed"', self.entry)
        missing_check = self.entry.index(
            "if (-not (Test-Path -LiteralPath $stage.script_path -PathType Leaf))"
        )
        stage_loop = self.entry.index(
            "for ($index = 0; $index -lt $stageDefinitions.Count; $index++)"
        )
        self.assertLess(missing_check, stage_loop)

        loop_source = self.entry[stage_loop : self.entry.index("$runSucceeded = $true")]
        invocation = loop_source.index("Invoke-RyFrameV07Stage")
        failed = loop_source.index(
            '$metadata["stages"][$index]["status"] = "failed"'
        )
        passed = loop_source.index(
            '$metadata["stages"][$index]["status"] = "passed"'
        )
        self.assertLess(invocation, failed)
        self.assertLess(failed, passed)
        self.assertNotIn("AllowFailure", loop_source)

    def test_project_and_evidence_are_unique_and_scoped(self) -> None:
        for fragment in (
            '$projectName = "ryframe-v07-$runId".ToLowerInvariant()',
            '$ownershipToken = "ryframe-v07-owner-{0}"',
            '"^ryframe-v07-[a-z0-9-]+$"',
            'Join-Path $targetDirectory "runtime-acceptance-0-7"',
            "$runDirectory.StartsWith($runPrefix, $pathComparison)",
            'Join-Path $runDirectory "run.json"',
            'Join-Path $runDirectory "acceptance-transcript.log"',
            'fault_injection = "docker_native"',
        ):
            self.assertIn(fragment, self.entry)
        self.assertIn("[guid]::NewGuid()", self.entry)
        self.assertNotIn("Remove-Item", self.entry)

    def test_release_evidence_is_bound_to_a_clean_commit_lockfile_and_binaries(self) -> None:
        for fragment in (
            '@("rev-parse", "--verify", "HEAD")',
            '@("status", "--porcelain=v1", "--untracked-files=all")',
            'git_commit = $gitCommit',
            'worktree_clean = $true',
            'cargo_lock_sha256 = (Get-FileHash',
            'binaries = $null',
            '$metadata["binaries"] = Get-RyFrameV07BinaryEvidence',
            '$metadata["stages"][$index]["binaries"] = Get-RyFrameV07BinaryEvidence',
            "Assert-RyFrameV07SourceIdentity",
            'foreach ($name in @("ryframe", "ryframe-worker", "ryframe-db-reset", "ryframe-migrate"))',
            'sha256 = (Get-FileHash',
        ):
            self.assertIn(fragment, self.entry)
        dirty_guard = self.entry.index("if ($worktreeStatus.Count -gt 0)")
        run_directory_create = self.entry.index("New-Item -ItemType Directory")
        docker_lookup = self.entry.index("Get-Command docker")
        self.assertLess(dirty_guard, run_directory_create)
        self.assertLess(dirty_guard, docker_lookup)
        binary_evidence = self.entry.index(
            '$metadata["binaries"] = Get-RyFrameV07BinaryEvidence'
        )
        run_succeeded = self.entry.index("$runSucceeded = $true")
        self.assertLess(binary_evidence, run_succeeded)
        stage_invocation = self.entry.index("Invoke-RyFrameV07Stage", self.entry.index("for ($index"))
        stage_source_check = self.entry.index("Assert-RyFrameV07SourceIdentity", stage_invocation)
        stage_passed = self.entry.index(
            '$metadata["stages"][$index]["status"] = "passed"',
            stage_source_check,
        )
        self.assertLess(stage_invocation, stage_source_check)
        self.assertLess(stage_source_check, stage_passed)

    def test_metadata_is_atomic_and_terminal_state_follows_cleanup(self) -> None:
        writer_start = self.support.index("function Write-RyFrameV07MetadataAtomically")
        writer = self.support[writer_start:]
        for fragment in (
            "[System.IO.FileStream]::new(",
            "$stream.Flush($true)",
            "[System.Text.UTF8Encoding]::new($false)",
            "[System.IO.File]::Replace($temporaryPath, $destinationPath, $backupPath, $true)",
            "[System.IO.File]::Move($temporaryPath, $destinationPath)",
            "foreach ($cleanupPath in @($temporaryPath, $backupPath))",
            "[System.IO.File]::Delete($ArtifactPath)",
            "$null -ne $cleanupError -and -not $committed",
        ):
            self.assertIn(fragment, writer)

        for fragment in (
            'status = "starting"',
            '$metadata["status"] = "running"',
            "Resolve-RyFrameV07TerminalStatus",
            '$metadata["completed_at"] =',
            '$metadata["cleanup_errors"] = @($cleanupErrors)',
        ):
            self.assertIn(fragment, self.entry)
        cleanup = self.entry.rindex("Remove-RyFrameV07DockerProjectResources")
        terminal_write = self.entry.rindex(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath"
        )
        success = self.entry.rindex("$script:RyFrameV07Messages.Success")
        self.assertLess(cleanup, terminal_write)
        self.assertLess(terminal_write, success)

    def test_docker_fault_helpers_are_native_scoped_and_recoverable(self) -> None:
        for function_name in (
            "Stop-RyFrameV07DockerService",
            "Start-RyFrameV07DockerService",
            "Disconnect-RyFrameV07DockerServiceNetwork",
            "Restore-RyFrameV07DockerServiceNetwork",
            "Restore-RyFrameV07DockerFault",
            "Remove-RyFrameV07DockerProjectResources",
            "Get-RyFrameV07ProjectImageEvidence",
            "Get-RyFrameV07OwnedProcess",
            "Get-RyFrameV07ProcessEnvironmentSnapshot",
            "Restore-RyFrameV07ProcessEnvironmentSnapshot",
            "Assert-RyFrameV07ProjectEmpty",
            "Assert-RyFrameV07OwnershipToken",
        ):
            self.assertIn(f"function {function_name}", self.support)

        for command in (
            '@("container", "stop", "--time", "10", $containerId)',
            '@("container", "start", $containerId)',
            '@("network", "disconnect", $networkId, $containerId)',
            '@("network", "connect", $networkId, $containerId)',
        ):
            self.assertIn(command, self.support)

        for fragment in (
            "label=com.docker.compose.project=$ProjectName",
            "Assert-RyFrameV07ResourceProjectLabel",
            '$ResourceKind -eq "container"',
            '$removeArguments += "--force"',
            "$cleanupFailures.Add($_.Exception.Message)",
            'if ($cleanupDetails.Count -gt 0)',
            'if ($endpoint -notmatch "^(npipe|unix)://")',
            '$inspectJson | ConvertFrom-Json',
            '$labels.PSObject.Properties["com.docker.compose.project"]',
            '$labels.PSObject.Properties["com.ryframe.runtime-acceptance-owner"]',
            'configured_image = [string]$container.Config.Image',
            'image_id = $imageId',
            'repo_digests = @($imageDocuments[0].RepoDigests | Sort-Object)',
            "$Process.HasExited",
            "$Process.StartTime.ToUniversalTime().Ticks",
            "$current.StartTime.ToUniversalTime().Ticks",
            "$actualStartedAt -ne $expectedStartedAt",
        ):
            self.assertIn(fragment, self.support)
        self.assertNotIn("{{ index .Config.Labels", self.support)
        self.assertNotIn("{{ index .Labels", self.support)
        self.assertNotIn("docker system prune", self.support.lower())
        self.assertNotIn("toxiproxy", (self.entry + self.support).lower())
        self.assertNotIn("Remove-Item", self.support)

        ownership_guard = self.entry.index("Assert-RyFrameV07ProjectEmpty")
        ownership_acquired = self.entry.index("$dockerOwnershipAcquired = $true")
        self.assertLess(ownership_guard, ownership_acquired)
        self.assertIn("-OwnershipToken $ownershipToken", self.entry)

    def test_native_process_captures_stderr_and_exit_code_without_powershell_error_promotion(self) -> None:
        function_start = self.support.index("function Invoke-RyFrameV07ProcessLines")
        function_end = self.support.index("\nfunction ", function_start + 1)
        function_source = self.support[function_start:function_end]
        for fragment in (
            "[System.Diagnostics.ProcessStartInfo]::new()",
            "$startInfo.UseShellExecute = $false",
            "$startInfo.RedirectStandardOutput = $true",
            "$startInfo.RedirectStandardError = $true",
            "ConvertTo-RyFrameV07ProcessArgument",
            "$process.StandardOutput.ReadToEndAsync()",
            "$process.StandardError.ReadToEndAsync()",
            "$exitCode = $process.ExitCode",
            "$process.Dispose()",
        ):
            self.assertIn(fragment, function_source)
        self.assertNotIn("& $DockerExecutable", self.support)

    def test_cleanup_and_primary_failure_have_deterministic_precedence(self) -> None:
        resolver_start = self.entry.index("function Resolve-RyFrameV07TerminalStatus")
        resolver_end = self.entry.index("\nfunction ", resolver_start + 1)
        resolver = self.entry[resolver_start:resolver_end]
        self.assertLess(resolver.index("if ($HasRunError)"), resolver.index("if ($CleanupErrorCount -gt 0)"))
        self.assertLess(resolver.index("if ($CleanupErrorCount -gt 0)"), resolver.index("if ($RunSucceeded)"))
        self.assertIn('return "cleanup_failed"', resolver)
        self.assertIn('return "passed"', resolver)

        terminal = self.entry[self.entry.rindex("finally {") :]
        self.assertLess(terminal.index("if ($null -ne $runError)"), terminal.index("if ($cleanupErrors.Count -gt 0)"))
        self.assertIn("if ($null -eq $runError)", terminal)
        self.assertIn("$cleanupErrors.Add($metadataWriteError)", terminal)

    def test_runtime_messages_decode_to_chinese(self) -> None:
        pattern = re.compile(r"ConvertFrom-Json @'\n(.*?)\n'@", re.DOTALL)
        for source in (self.entry, self.support):
            match = pattern.search(source)
            self.assertIsNotNone(match)
            messages = json.loads(match.group(1))
            for message in messages.values():
                self.assertRegex(message, r"[\u4e00-\u9fff]")
        self.assertNotRegex(self.entry + self.support, r'-Description\s+"[A-Za-z]')

    def test_powershell_ast_is_valid_when_host_is_available(self) -> None:
        executable_name = "powershell.exe" if os.name == "nt" else "pwsh"
        powershell = shutil.which(executable_name)
        if powershell is None:
            self.skipTest(f"未找到 {executable_name}，跳过 PowerShell AST 验证")

        quoted_paths = ",".join(
            "'" + str(path).replace("'", "''") + "'" for path in (ENTRY, SUPPORT)
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
