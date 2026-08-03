from __future__ import annotations

import json
import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "scripts/file_a_acceptance.ps1"
RUST_TEST_PATH = ROOT / "crates/ryframe/tests/file_a_acceptance_test.rs"
HYGIENE_PATH = ROOT / "scripts/check_source_hygiene.py"


def source(path: Path) -> str:
    return path.read_text(encoding="utf-8")


class FileAAcceptanceGuardTest(unittest.TestCase):
    def test_entrypoint_is_unique_loopback_only_and_safely_cleaned(self) -> None:
        script = source(SCRIPT_PATH)
        self.assertIn('[Guid]::NewGuid().ToString("N")', script)
        self.assertIn('$projectName = "ryframe-file-a-$runId"', script)
        self.assertIn('$databaseName = "ryframe_file_a_$runId"', script)
        self.assertIn('"--project-name", $projectName', script)
        self.assertIn('"127.0.0.1:__MYSQL_PORT__:3306"', script)
        self.assertIn('"127.0.0.1:__RUSTFS_PORT__:9000"', script)
        self.assertIn('$runRoot.StartsWith($requiredPrefix', script)
        self.assertIn('$resolvedComposeFile.StartsWith($requiredPrefix', script)
        self.assertIn('"down", "--volumes", "--remove-orphans"', script)
        self.assertRegex(script, r"(?s)try\s*\{.*finally\s*\{")
        self.assertIn("$endpoint -notmatch '^(npipe|unix)://'", script)
        self.assertGreaterEqual(script.count('"--context", $dockerContextName, "compose"'), 3)
        self.assertNotIn("docker-compose.test.yml", script)
        self.assertNotIn("compose.prod", script)
        self.assertNotIn('APP_ENV = "prod"', script)

    def test_workflow_order_covers_both_guards_and_final_verification(self) -> None:
        script = source(SCRIPT_PATH)
        workflow_start = script.index("$metadata = [ordered]@{")
        workflow = script[workflow_start:]
        ordered_markers = (
            '-Name "seed_file_a_legacy_fixture"',
            '-ExpectedHint "backfill-sha256"',
            '-Command "backfill-sha256" -Mode "dry-run"',
            '-Command "backfill-sha256" -Mode "apply"',
            '-ExpectedHint "drain-legacy-reservations"',
            '-Command "drain-legacy-reservations" -Mode "dry-run"',
            '-Command "drain-legacy-reservations" -Mode "apply"',
            '$migrationUp = Invoke-NativeProcess',
            '$migrationStatus = Invoke-NativeProcess',
            '$migrationVerify = Invoke-NativeProcess',
            '-Name "assert_file_a_final_state"',
        )
        positions = [workflow.index(marker) for marker in ordered_markers]
        self.assertEqual(positions, sorted(positions))
        self.assertIn("remaining=0", workflow)
        self.assertIn("up_to_date=true", workflow)

    def test_binaries_are_built_once_and_evidence_survives_cleanup(self) -> None:
        script = source(SCRIPT_PATH)
        self.assertEqual(
            script.count(
                '"build", "--quiet", "--locked", "-p", "ryframe", "--features", "file-maintenance"'
            ),
            1,
        )
        self.assertNotIn('"run", "--quiet", "-p", "ryframe"', script)
        self.assertIn('-FilePath $migrateBinary -ArgumentList @("up")', script)
        self.assertIn('-FilePath $MaintenanceBinary', script)
        self.assertIn('$script:AcceptanceLogPath = Join-Path $runRoot "acceptance.log"', script)
        self.assertIn('$metadataPath = Join-Path $runRoot "run.json"', script)
        self.assertIn('if ($cleanupSucceeded -and [IO.File]::Exists($composeFile))', script)
        self.assertNotIn("Remove-Item -LiteralPath $runRoot", script)
        self.assertNotIn("Remove-Item -LiteralPath $resolvedRunRoot", script)

    def test_native_process_capture_is_utf8_and_keeps_stderr_out_of_error_stream(self) -> None:
        script = source(SCRIPT_PATH)
        for marker in (
            "[Diagnostics.ProcessStartInfo]::new()",
            "$startInfo.RedirectStandardOutput = $true",
            "$startInfo.RedirectStandardError = $true",
            "$startInfo.StandardOutputEncoding = $utf8NoBom",
            "$startInfo.StandardErrorEncoding = $utf8NoBom",
            "$process.StandardOutput.ReadToEndAsync()",
            "$process.StandardError.ReadToEndAsync()",
            "$inputBytes = $utf8NoBom.GetBytes($StandardInput)",
            "$standardInputStream = $process.StandardInput.BaseStream",
            "$standardInputStream.Write($inputBytes, 0, $inputBytes.Length)",
            "StandardOutput = $standardOutput",
            "StandardError = $standardError",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, script)
        self.assertNotIn("2>&1", script)
        self.assertNotIn("Write-Error", script)
        self.assertNotIn("[Console]::InputEncoding", script)
        self.assertIn(
            "$Output.IndexOf($Expected, [StringComparison]::Ordinal) -lt 0",
            script,
        )
        self.assertIn("-Output $seed.StandardOutput", script)
        self.assertIn("-Output $finalAssertion.StandardOutput", script)

    def test_real_fixture_proves_digest_and_collision_contract(self) -> None:
        script = source(SCRIPT_PATH)
        rust_test = source(RUST_TEST_PATH)
        self.assertNotIn("\ufffd", rust_test)
        message_json = script.split("$script:FileAMessages = ConvertFrom-Json @'", 1)[
            1
        ].split("'@", 1)[0]
        messages = json.loads(message_json)
        expected_markers = {
            "SeedExpected": "FILE-A 旧 schema、MD5 碰撞数据与 RustFS 对象种子已就绪",
            "FinalExpected": "FILE-A 最终 schema、SHA-256 与碰撞对象隔离断言已通过",
        }
        for key, marker in expected_markers.items():
            with self.subTest(message=key):
                self.assertEqual(messages[key], marker)
                self.assertIn(marker, rust_test)
        ignored_names = re.findall(
            r'#\[ignore\s*=\s*"[^"]+"\]\s*async\s+fn\s+([a-z0-9_]+)',
            rust_test,
        )
        self.assertEqual(
            ignored_names,
            ["seed_file_a_legacy_fixture", "assert_file_a_final_state"],
        )
        for marker in (
            "S3ObjectStorage",
            ".ensure_bucket(",
            ".put(",
            ".get(",
            "COLLISION_MD5",
            "FIRST_COLLISION_HEX",
            "SECOND_COLLISION_HEX",
            "assert_ne!(",
            "CREATE INDEX idx_file_upload_reservation",
            '"file_md5"',
            '"idx_file_upload_reservation"',
            '"idx_file_sha256"',
            "IS_NULLABLE = 'NO'",
            "127.0.0.1",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, rust_test)

    def test_ignored_test_allowlist_is_exact_not_generic(self) -> None:
        hygiene = source(HYGIENE_PATH)
        allowlist = hygiene.split("ALLOWED_IGNORED_TESTS = {", 1)[1].split(
            "IGNORED_TEST_PATTERN", 1
        )[0]
        expected_names = (
            "seed_file_a_legacy_fixture",
            "assert_file_a_final_state",
        )
        self.assertEqual(
            allowlist.count('"crates/ryframe/tests/file_a_acceptance_test.rs"'),
            2,
        )
        for name in expected_names:
            self.assertEqual(allowlist.count(f'"{name}"'), 1)
        self.assertNotRegex(
            allowlist,
            r"file_a_acceptance[^\n]*(?:\*|\.\*|startswith|endswith)",
        )


if __name__ == "__main__":
    unittest.main()
