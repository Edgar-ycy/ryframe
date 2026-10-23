"""保留的 CI 数据库客户端参数与凭据文件测试，不连接数据库。"""

import copy
import os
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import full_stack_migration_mysql as mysql
from workspace_directory import WorkspaceDirectory


class CiMysqlInvocationTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.binding = {
            "host": "127.0.0.1", "port": 3317, "database": "control_test",
            "username": "root", "password": "control-secret", "tls_mode": "required",
            "_scope_id": "ci-123-2", "_mysql_client": str(Path(sys.executable).resolve()),
            "_container_id": "abcdef012345", "_client_dir": str(self.root / "mysql-client"),
        }
        environment = mock.patch.dict(os.environ, {"APP_ENV": "test"}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)

    def test_native_credentials_use_managed_file_and_cleanup_even_on_error(self):
        with mysql.client_invocation(self.binding, "mysql") as (arguments, environment, prefix):
            option_file = Path(arguments[1].removeprefix("--defaults-file="))
            self.assertEqual(option_file.read_text(encoding="utf-8"), '[client]\npassword="control-secret"\n')
            for flag in ("--port=3317", "--host=127.0.0.1", "--ssl-mode=REQUIRED"):
                self.assertIn(flag, arguments)
            self.assertEqual(prefix, "")
            self.assertNotIn("MYSQL_PWD", environment)
            self.assertNotIn("secret", " ".join(arguments))
        self.assertFalse(option_file.exists())
        with self.assertRaisesRegex(RuntimeError, "caller"):
            with mysql.client_invocation(self.binding, "mysql") as (arguments, _, _):
                option_file = Path(arguments[1].removeprefix("--defaults-file="))
                raise RuntimeError("caller")
        self.assertFalse(option_file.exists())

    def test_ci_credentials_use_stdin_and_exact_job_container(self):
        with mock.patch.dict(os.environ, {
            "GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
            "MYSQL_PWD": "host-secret", "APP_DATABASE_PASSWORD": "host-secret",
        }):
            with mysql.client_invocation(self.binding, "mysql") as (arguments, environment, prefix):
                self.assertEqual(arguments[:4], ["docker", "exec", "-i", "abcdef012345"])
                self.assertIn("--port=3306", arguments)
                self.assertIn("--ssl-mode=REQUIRED", arguments)
                self.assertEqual(prefix, '[client]\npassword="control-secret"\n')
                self.assertNotIn("MYSQL_PWD", environment)
                self.assertNotIn("host-secret", str(environment))
                self.assertNotIn("secret", " ".join(arguments))
            for changed in ({"_scope_id": "ci-123-1"}, {"_container_id": "invalid"}):
                with self.subTest(changed=changed), self.assertRaisesRegex(ValueError, "当前 CI"):
                    with mysql.client_invocation({**self.binding, **changed}, "mysql"):
                        self.fail("错误 CI 归属不得创建客户端")
            for name, value in (("GITHUB_RUN_ID", "0"), ("GITHUB_RUN_ATTEMPT", "invalid")):
                with self.subTest(name=name), mock.patch.dict(os.environ, {name: value}):
                    with self.assertRaisesRegex(ValueError, "当前 CI"):
                        with mysql.client_invocation(self.binding, "mysql"):
                            self.fail("错误 CI 运行标识不得创建客户端")

    def test_invalid_connection_fields_reject_without_exposing_values(self):
        for field, value in (
            ("host", "db.example"), ("database", "db`;DROP DATABASE x"),
            ("port", 0), ("port", "3317"), ("port", True), ("port", 65536),
            ("username", ""), ("password", ""), ("password", "private\0value"),
            ("password_env", "PRIVATE"), ("tls_mode", "unknown"),
            ("tls_mode", "verify_identity"), ("tls_client_cert", "only-cert.pem"),
        ):
            with self.subTest(field=field, value=value), self.assertRaises(ValueError) as raised:
                mysql.validate_binding({**self.binding, field: value})
            self.assertNotIn("PRIVATE", str(raised.exception))
            self.assertNotIn("private\0value", str(raised.exception))
        self.assertFalse((self.root / "mysql-client").exists())
        with self.assertRaisesRegex(ValueError, "绝对路径"):
            with mysql.client_invocation({**self.binding, "_mysql_client": "mysql"}, "mysql"):
                self.fail("相对客户端路径不得被使用")
        with self.assertRaisesRegex(ValueError, "绝对路径"):
            with mysql.client_invocation({**self.binding, "_client_dir": "relative"}, "mysql"):
                self.fail("相对凭据目录不得被使用")

    def test_client_option_escaping_keeps_password_on_one_line(self):
        value = 'quote"slash\\line\nnext\rtab\tback\b中文#;'
        options = mysql.client_options(value)
        self.assertEqual(len(options.splitlines()), 2)
        for escaped in ('\\"', '\\\\', '\\n', '\\r', '\\t', '\\b', '中文#;'):
            self.assertIn(escaped, options)

    def test_verified_tls_files_are_preserved_in_client_arguments(self):
        binding = {**self.binding, "tls_mode": "verify_identity", "tls_ca": "ca.pem",
                   "tls_client_cert": "client.pem", "tls_client_key": "client-key.pem"}
        with mysql.client_invocation(binding, "mysql") as (arguments, _, _):
            for flag in ("--ssl-mode=VERIFY_IDENTITY", "--ssl-ca=ca.pem",
                         "--ssl-cert=client.pem", "--ssl-key=client-key.pem"):
                self.assertIn(flag, arguments)

    def test_configured_databases_returns_primary_copy_and_rejects_wrong_shapes(self):
        primary = {key: value for key, value in self.binding.items() if not key.startswith("_")}
        settings = {"database": {"primary": primary}, "tenant_data": {"targets": []}}
        control, targets = mysql.configured_databases(settings)
        self.assertEqual(control, primary)
        self.assertEqual(targets, [])
        control["database"] = "other"
        self.assertEqual(primary["database"], "control_test")
        for changed in ({"database": "private-value"}, {"database": {"primary": "private-value"}},
                        {"tenant_data": "private-value"}, {"tenant_data": {"targets": ["private-value"]}}):
            with self.subTest(changed=changed), self.assertRaises(ValueError) as raised:
                mysql.configured_databases({**copy.deepcopy(settings), **changed})
            self.assertNotIn("private-value", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
