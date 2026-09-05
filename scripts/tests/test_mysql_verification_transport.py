"""固定只读核验选择唯一 argv 传输，业务 SQL、错误和失败关闭保持原行为。"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mysql_verification_fixture import mysql_output
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import DatabaseVerificationError, ExternalTools


class MysqlVerificationTransportTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.answer, self.error = b"value\n", None
        self.run = Mock(side_effect=self.reply)
        self.tools = ExternalTools(self.plan, self.work, self.run)
        self.database = self.plan["source"]["databases"][0]

    def reply(self, command, **options):
        if self.error is not None:
            if "--execute" in command:
                options["stdout"].write(getattr(self.error, "stdout", None) or b"")
            raise self.error
        return mysql_output(command, options, self.answer)

    def test_fixed_queries_use_only_exact_execute_arguments_without_stdin(self):
        queries = {
            "identity": "SELECT @@server_uuid, DATABASE();",
            "ownership": "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;",
        }
        for check, sql in queries.items():
            with self.subTest(check=check), patch.dict("os.environ", {"MYSQL_PWD": "ambient-secret"}):
                self.run.reset_mock()
                self.assertEqual(self.tools.database_verification_response(self.database, check), "value")
                command = self.run.call_args.args[0]
                options = self.run.call_args.kwargs
                self.assertEqual(command[-2:], ["--execute", sql])
                self.assertEqual(command.count("--execute"), 1)
                self.assertEqual(command[1], "--defaults-file=" + self.database["defaults_file"])
                self.assertIn("--database=source_control", command)
                self.assertNotIn("ambient-secret", " ".join(command))
                self.assertNotIn("MYSQL_PWD", options["env"])
                self.assertEqual(options["env"]["MYSQL_TEST_LOGIN_FILE"], str(self.work / "unused-login.cnf"))
                self.assertIsNone(options["input"])
                self.assertEqual(options["stdin"], subprocess.DEVNULL)
                self.assertTrue(options["check"])
                self.assertEqual(options["timeout"], 1800)
                self.assertEqual(self.run.call_count, 1)

    def test_arbitrary_check_and_sql_inputs_are_rejected_before_command_creation(self):
        for value in (None, [], {}, 1, "", "IDENTITY", "SELECT 1;", "ownership;DROP TABLE secret", "identity\n"):
            with self.subTest(type=type(value).__name__):
                with self.assertRaises(ValueError):
                    self.tools.database_verification_response(self.database, value)
        self.run.assert_not_called()

    def test_business_sql_keeps_stdin_and_never_places_secret_in_argv(self):
        sql = "START TRANSACTION; INSERT INTO `sys_post` (`name`) VALUES ('私密 business-secret'); COMMIT;\n"
        self.tools.mysql(self.database, sql)
        command, options = self.run.call_args.args[0], self.run.call_args.kwargs
        self.assertNotIn("--execute", command)
        self.assertNotIn("business-secret", " ".join(command))
        self.assertEqual(options["input"], sql.encode("utf-8"))
        self.assertNotIn("stdin", options)
        self.assertEqual(self.run.call_count, 1)

    def test_fixed_query_method_does_not_delegate_to_business_stdin(self):
        self.tools.mysql = Mock(side_effect=AssertionError("不能回退到 stdin"))
        self.tools.database_verification_response(self.database, "identity")
        self.tools.mysql.assert_not_called()
        self.assertEqual(self.run.call_count, 1)

    def test_empty_identity_success_retries_once_and_can_succeed(self):
        responses = iter((b"", b"uuid-1\tsource_control\n"))
        self.run.side_effect = lambda command, **options: mysql_output(command, options, next(responses))
        self.assertEqual(self.tools.database_verification_response(self.database, "identity"),
                         "uuid-1\tsource_control")
        self.assertEqual(self.run.call_count, 2)

    def test_empty_ownership_success_retries_once_and_can_succeed(self):
        responses = iter((b"", b"control\tscope\towner\n"))
        self.run.side_effect = lambda command, **options: mysql_output(command, options, next(responses))
        self.assertEqual(self.tools.database_verification_response(self.database, "ownership"),
                         "control\tscope\towner")
        self.assertEqual(self.run.call_count, 2)

    def test_double_empty_identity_fails_after_exactly_one_retry(self):
        self.answer = b""
        with self.assertRaises(DatabaseVerificationError) as caught:
            self.tools.database_verification_response(self.database, "identity")
        self.assertEqual(caught.exception.code, "database_identity_mismatch")
        self.assertEqual(caught.exception.response_bytes, 0)
        self.assertEqual(self.run.call_count, 2)
        records = [path.read_text(encoding="utf-8") for path in self.work.glob("mysql-*.json")]
        self.assertEqual(sum('"kind": "mysql-verification-output"' in value for value in records), 2)
        self.assertEqual(sum('"kind": "mysql-verification-retry"' in value for value in records), 1)

    def test_double_empty_ownership_fails_after_exactly_one_retry(self):
        self.answer = b""
        with self.assertRaises(DatabaseVerificationError) as caught:
            self.tools.database_verification_response(self.database, "ownership")
        self.assertEqual(caught.exception.code, "database_ownership_mismatch")
        self.assertEqual(caught.exception.response_bytes, 0)
        self.assertEqual(self.run.call_count, 2)

    def test_nonempty_malformed_identity_does_not_retry(self):
        self.answer = b"\n"
        with self.assertRaises(DatabaseVerificationError) as caught:
            self.tools.verify_databases("source")
        self.assertEqual(caught.exception.code, "database_identity_mismatch")
        self.assertEqual(self.run.call_count, 1)

    def test_empty_stdout_with_nonempty_stderr_does_not_retry_or_leak_stderr(self):
        secret = b"private-stderr"
        self.run.side_effect = lambda command, **options: mysql_output(
            command, options, b"", stderr=secret)
        with self.assertRaises(DatabaseVerificationError):
            self.tools.verify_databases("source")
        self.assertEqual(self.run.call_count, 1)
        records = list(self.work.glob("mysql-*.json"))
        self.assertEqual(len(records), 1)
        receipt = json.loads(records[0].read_text(encoding="utf-8"))
        self.assertEqual((receipt["stderr_bytes"], receipt["stderr_sha256"]),
                         (len(secret), hashlib.sha256(secret).hexdigest()))
        self.assertNotIn(secret.decode(), records[0].read_text(encoding="utf-8"))

    def test_empty_stdout_with_unknown_stderr_does_not_retry(self):
        self.run.side_effect = lambda command, **options: mysql_output(command, options, b"", stderr=None)
        with self.assertRaises(DatabaseVerificationError):
            self.tools.verify_databases("source")
        self.assertEqual(self.run.call_count, 1)
        receipt = json.loads(next(self.work.glob("mysql-*.json")).read_text(encoding="utf-8"))
        self.assertEqual((receipt["stderr_bytes"], receipt["stderr_sha256"]), (None, None))

    def test_nonzero_preserves_safe_error_and_does_not_retry_or_fallback(self):
        for check in ("identity", "ownership"):
            with self.subTest(check=check):
                self.run.reset_mock()
                self.error = subprocess.CalledProcessError(7, ["private-command"],
                                                                    output=b"private-output", stderr=b"private-secret")
                with self.assertRaises(DatabaseVerificationError) as caught:
                    self.tools.database_verification_response(self.database, check)
                self.assertEqual(caught.exception.code, f"database_{check}_command_failed")
                self.assertEqual(caught.exception.returncode, 7)
                self.assertNotIn("private", str(caught.exception))
                self.assertEqual(self.run.call_count, 1)

    def test_timeout_and_invalid_utf8_keep_original_error_types(self):
        original = subprocess.TimeoutExpired(["fixed-query"], 1800)
        self.error = original
        with self.assertRaises(subprocess.TimeoutExpired) as caught:
            self.tools.database_verification_response(self.database, "identity")
        self.assertIs(caught.exception, original)
        self.assertEqual(self.run.call_count, 1)
        self.run.reset_mock()
        self.error, self.answer = None, b"\xff"
        with self.assertRaises(UnicodeDecodeError):
            self.tools.database_verification_response(self.database, "ownership")
        self.assertEqual(self.run.call_count, 1)

    def test_bad_business_sql_type_does_not_turn_into_argv(self):
        with self.assertRaises(AttributeError):
            self.tools.mysql(self.database, None)
        self.run.assert_not_called()

    def test_shared_command_rejects_changed_defaults_before_both_transports(self):
        defaults = Path(self.database["defaults_file"])
        defaults.write_text(defaults.read_text() + "init-command=private-sql\n")
        for operation in (lambda: self.tools.mysql(self.database, "SELECT 'business-secret';"),
                          lambda: self.tools.database_verification_response(self.database, "identity")):
            with self.assertRaises(ValueError):
                operation()
        self.run.assert_not_called()

    def test_closed_stdin_cannot_silently_discard_input(self):
        with self.assertRaises(ValueError):
            self.tools.execute(["fixed-fixture"], data=b"private-sql", closed_stdin=True)
        self.run.assert_not_called()

    def test_other_external_commands_keep_existing_stdin_behavior(self):
        self.tools.execute(["fixed-fixture"])
        self.assertIsNone(self.run.call_args.kwargs["input"])
        self.assertNotIn("stdin", self.run.call_args.kwargs)


if __name__ == "__main__":
    unittest.main()
