"""数据库核验只发布固定安全字段，成功、失败及未知异常均不改变接受条件。"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_inventory as inventory
from restore_reference_io import DatabaseVerificationError, ExternalTools
from mysql_verification_fixture import mysql_input, mysql_output
import test_devex_clone_inventory as fixture


class InventoryDiagnosticTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixture.InventoryTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.mysql_calls = []

    def inject(self, change):
        original = self.fixture.tools.run

        def run(command, **kwargs):
            if command[0] == str(self.fixture.external):
                sql = mysql_input(command, kwargs)
                check = "identity" if sql == "SELECT @@server_uuid, DATABASE();" else "ownership"
                self.mysql_calls.append(check)
                try:
                    response = change(check, len(self.mysql_calls), command)
                except subprocess.CalledProcessError as error:
                    kwargs["stdout"].write(error.stdout or b"")
                    raise
                if response is not None:
                    return mysql_output(command, kwargs, response)
            return original(command, **kwargs)

        self.fixture.tools.run = run

    def failure(self, stage="inventory_before"):
        failure = self.fixture.assert_failed()
        self.assertEqual(failure["stage"], stage)
        self.assertFalse((self.fixture.output / "inventory.json").exists())
        text = json.dumps(failure)
        for secret in ("fixture-secret", "secret-fixture", "private-sql", "private-command", str(self.fixture.client)):
            self.assertNotIn(secret, text)
        return failure

    def assert_diagnostic(self, failure, check, *, rows, size, code=None, returncode=None, measurement="mysql_utf8_after_strip"):
        self.assertEqual(failure["error_type"], "DatabaseVerificationError")
        self.assertEqual(failure["database_verification"], {
            "code": code or f"database_{check}_mismatch", "check": check,
            "target_key": "shared-control", "expected_rows": 1 if check == "identity" else 2,
            "actual_rows": rows, "response_bytes": size, "response_measurement": measurement,
            "returncode": returncode,
        })

    def test_successful_empty_identity_response_fails_after_one_retry_with_zero_counts(self):
        self.inject(lambda check, _count, _command: b"" if check == "identity" else None)
        self.assert_diagnostic(self.failure(), "identity", rows=0, size=0)
        self.assertEqual(self.mysql_calls, ["identity", "identity"])
        self.assertEqual(self.fixture.cli_count, 0)

    def test_successful_empty_owner_response_fails_after_one_retry_without_cli(self):
        self.inject(lambda check, _count, _command: b"" if check == "ownership" else None)
        self.assert_diagnostic(self.failure(), "ownership", rows=0, size=0)
        self.assertEqual(self.mysql_calls, ["identity", "ownership", "ownership"])
        self.assertEqual(self.fixture.cli_count, 0)

    def test_observed_final_owner_empty_failure_retains_four_inventory_files(self):
        self.inject(lambda check, count, _command: b"" if check == "ownership" and count in (14, 15) else None)
        self.assert_diagnostic(self.failure(), "ownership", rows=0, size=0)
        self.assertEqual(len(self.mysql_calls), 15)
        self.assertEqual(self.fixture.cli_count, 4)
        self.assertEqual(len(list(self.fixture.output.glob("before-target-*.json"))), 4)

    def test_second_inventory_empty_identity_reports_exact_stage(self):
        self.inject(lambda check, count, _command: b"" if check == "identity" and count in (21, 22) else None)
        self.assert_diagnostic(self.failure("inventory_after"), "identity", rows=0, size=0)
        self.assertEqual(len(self.mysql_calls), 22)
        self.assertEqual(self.fixture.cli_count, 4)

    def test_wrong_identity_has_safe_code_without_identity_contents(self):
        response = b"fixture-secret\tprivate-sql\n"
        self.inject(lambda check, _count, _command: response if check == "identity" else None)
        self.assert_diagnostic(self.failure(), "identity", rows=1, size=len(response.strip()))
        self.assertEqual(len(self.mysql_calls), 1)

    def test_wrong_owner_with_expected_row_count_still_fails(self):
        response = b"control\tfixture-secret\twrong\ntenant-data\tfixture-secret\twrong\n"
        self.inject(lambda check, _count, _command: response if check == "ownership" else None)
        self.assert_diagnostic(self.failure(), "ownership", rows=2, size=len(response.strip()))
        self.assertEqual(len(self.mysql_calls), 2)

    def test_nonzero_identity_exit_reports_raw_stdout_counts_without_retry(self):
        def change(_check, _count, _command):
            raise subprocess.CalledProcessError(7, ["private-command", "private-sql"],
                                                output=b"fixture-secret\n", stderr=b"secret-fixture")
        self.inject(change)
        self.assert_diagnostic(self.failure(), "identity", rows=1, size=15,
                               code="database_identity_command_failed", returncode=7, measurement="raw_stdout")
        self.assertEqual(len(self.mysql_calls), 1)

    def test_nonzero_owner_exit_reports_distinct_safe_code(self):
        def change(check, _count, _command):
            if check == "ownership":
                raise subprocess.CalledProcessError(9, ["private-command"], output=b"", stderr=b"secret-fixture")
        self.inject(change)
        self.assert_diagnostic(self.failure(), "ownership", rows=0, size=0,
                               code="database_ownership_command_failed", returncode=9, measurement="raw_stdout")
        self.assertEqual(len(self.mysql_calls), 2)

    def test_post_cli_owner_malformed_or_wrong_order_remains_rejected(self):
        expected = inventory.expected_owners(self.fixture.scope, True)
        reordered = "\n".join("\t".join(row[field] for field in ("resource_kind", "scope_id", "marker"))
                               for row in reversed(expected)).encode()
        for suffix, response in (("columns", b"private-sql\n"), ("order", reordered)):
            with self.subTest(suffix=suffix):
                self.fixture.output = self.fixture.local / ("after-cli-" + suffix)
                original = self.fixture.tools.run
                self.mysql_calls.clear()
                self.inject(lambda check, count, _command: response if check == "ownership" and count == 9 else None)
                self.assert_diagnostic(self.failure(), "ownership", rows=len(response.splitlines()), size=len(response.strip()))
                self.assertEqual(len(self.mysql_calls), 9)
                self.fixture.tools.run = original

    def test_unknown_exception_records_only_type_even_with_diagnostic_like_attributes(self):
        def change(_check, _count, _command):
            error = ValueError("fixture-secret private-sql private-command")
            error.safe_details = lambda: {"password": "secret-fixture"}
            raise error
        self.inject(change)
        failure = self.failure()
        self.assertEqual(failure["error_type"], "ValueError")
        self.assertNotIn("database_verification", failure)
        self.assertEqual(len(self.mysql_calls), 1)

    def test_verification_set_comparison_and_exact_calls_are_unchanged(self):
        selected = self.fixture.selected
        tools = ExternalTools(self.fixture.tools.plan, self.fixture.local)
        responses = []
        for database in selected["databases"]:
            responses.append(database["server_uuid"] + "\t" + database["database"])
            owners = inventory.expected_owners(selected["scope_id"], database["kind"] == "combined")
            lines = ["\t".join(row[field] for field in ("resource_kind", "scope_id", "marker")) for row in owners]
            responses.append("\n".join(lines + lines))
        responses = iter(responses)
        tools.run = Mock(side_effect=lambda command, **kwargs: mysql_output(command, kwargs, next(responses).encode()))
        tools.verify_databases("source")
        self.assertEqual(tools.run.call_count, 8)

    def test_invalid_diagnostic_kind_cannot_publish_arbitrary_code_or_message(self):
        with self.assertRaisesRegex(ValueError, "诊断类别"):
            DatabaseVerificationError("private-sql", "shared-control", 1, "fixture-secret")

    def test_sensitive_stdin_is_forwarded_unchanged_and_records_only_digest(self):
        payload = b"SELECT 'fixture-secret private-sql';"
        capture = self.capture_runner("stdin-sensitive")
        capture.run(["fake-mysql"], input=payload)
        self.assertIs(capture.original.run.call_args.kwargs["input"], payload)
        paths = list(capture.output.glob("command-*.json"))
        self.assertEqual(len(paths), 1)
        receipt = json.loads(paths[0].read_text(encoding="utf-8"))
        self.assertEqual(receipt["stdin"], {"bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()})
        self.assertNotIn("private-sql", paths[0].read_text(encoding="utf-8"))
        self.assertNotIn("fixture-secret", paths[0].read_text(encoding="utf-8"))

    def test_empty_none_and_absent_stdin_have_distinct_safe_receipts(self):
        for name, arguments in (("empty", {"input": b""}), ("none", {"input": None}), ("absent", {}),
                                ("text", {"input": "private-sql"})):
            with self.subTest(name=name):
                capture = self.capture_runner("stdin-" + name)
                capture.run(["fake-mysql"], **arguments)
                forwarded = capture.original.run.call_args.kwargs
                self.assertEqual("input" in forwarded, "input" in arguments)
                if "input" in arguments:
                    self.assertIs(forwarded["input"], arguments["input"])
                receipt = json.loads(next(capture.output.glob("command-*.json")).read_text(encoding="utf-8"))
                expected = {"bytes": 0, "sha256": hashlib.sha256(b"").hexdigest()} if name == "empty" else None
                self.assertEqual(receipt["stdin"], expected)
                self.assertNotIn("private-sql", json.dumps(receipt))

    def capture_runner(self, name):
        capture = object.__new__(inventory._Capture)
        capture.environment = self.fixture.environment
        capture.output = self.fixture.local / name
        capture.output.mkdir()
        capture.redaction = self.fixture.environment
        capture.original = Mock(run=Mock(return_value=subprocess.CompletedProcess([], 0, b"", b"")))
        return capture


if __name__ == "__main__":
    unittest.main()
