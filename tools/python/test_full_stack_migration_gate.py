import io
import json
import os
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import full_stack_migration_gate as gate
import full_stack_migration_mysql as mysql


TENANT, MIGRATION, SCOPE = "tenant-0123abcd", "123456789", "migration-gate-test"
PENDING = {
    "migration_id": MIGRATION,
    "tenant_id": TENANT,
    "source": "shared-control",
    "target": "shared",
    "state": "prechecking",
    "retention_hours": 168,
    "job_status": "pending",
    "attempts": 0,
    "job_migration_id": MIGRATION,
}


class FixtureSession:
    def __init__(self, pending=None, counts=None, ownership_scope=SCOPE):
        self.pending = PENDING if pending is None else pending
        self.counts = {"source": 3, "target": 0} if counts is None else counts
        self.scope = ownership_scope
        self.queries, self.waits, self.closed = (
            [],
            [[{"waiting_connection_id": 81}]],
            False,
        )

    def execute(self, sql):
        self.queries.append(sql)
        if "ryframe_resource_ownership" in sql:
            kind = "control" if "resource_kind='control'" in sql else "tenant-data"
            return [
                {
                    "scope": self.scope,
                    "marker": f"ryframe-owner:v1:{self.scope}:{kind}",
                    "server": "same-server",
                }
            ]
        if "j.payload" in sql:
            return [self.pending]
        if "COUNT(*)" in sql:
            return [self.counts]
        if "CONNECTION_ID()" in sql:
            return [{"connection_id": 42}]
        if "data_lock_waits" in sql:
            return self.waits.pop(0)
        if sql == "ROLLBACK":
            return []
        raise AssertionError(f"非预期 fixture 查询：{sql}")

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.closed = True


class MigrationGateTests(unittest.TestCase):
    def setUp(self):
        self.observer, self.lock = FixtureSession(), FixtureSession()
        self.sessions = mock.patch.object(
            gate, "MysqlSession", side_effect=[self.observer, self.lock]
        )
        self.connect = self.sessions.start()
        self.addCleanup(self.sessions.stop)
        self.patch("verify_runtime", return_value={"scope_id": SCOPE})
        self.worker = self.patch("worker_identity", return_value=None)
        self.patch(
            "bindings",
            return_value=({"database": "control_test"}, {"database": "shared_test"}),
        )
        self.output = self.patch("emit")
        self.patch_time = mock.patch.object(gate.time, "sleep")
        self.patch_time.start()
        self.addCleanup(self.patch_time.stop)

    def patch(self, name, **kwargs):
        patched = mock.patch.object(gate, name, **kwargs)
        self.addCleanup(patched.stop)
        return patched.start()

    def hold(self, commands="", tenant=TENANT, migration=MIGRATION):
        with mock.patch.object(gate.sys, "stdin", io.StringIO(commands)):
            gate.hold(Path("backend"), Path("runtime"), tenant, migration)

    def test_waits_for_actual_lock_evidence_and_releases_exact_transaction(self):
        self.observer.waits.insert(0, [])
        self.hold('{"operation":"wait-blocked"}\n{"operation":"release"}\n')
        receipts = [call.args[0] for call in self.output.call_args_list]
        self.assertEqual(
            [value["state"] for value in receipts], ["held", "blocked", "released"]
        )
        self.assertEqual(receipts[1]["proof"]["waiting_connection_id"], 81)
        lock_query = self.lock.queries[1]
        self.assertIn(f"WHERE tenant_id='{TENANT}' FOR UPDATE", lock_query)
        self.assertIn("`shared_test`.biz_order", lock_query)
        self.assertEqual(self.lock.queries[-1], "ROLLBACK")
        proof = self.observer.queries[-1]
        for selector in (
            "b.PROCESSLIST_ID=42",
            "l.OBJECT_SCHEMA='shared_test'",
            f"m.id={MIGRATION} AND m.tenant_id='{TENANT}'",
            "i.table_name='biz_order'",
            "m.state='copying'",
            "i.state='copying'",
            "j.status='running'",
        ):
            self.assertIn(selector, proof)
        self.assertTrue(self.observer.closed and self.lock.closed)

    def test_invalid_identifiers_are_rejected_before_runtime_or_database_access(self):
        for tenant, migration in (
            ("system", MIGRATION),
            ("tenant-abcd';--", MIGRATION),
            (TENANT, "1 OR 1=1"),
            (TENANT, str(2**63)),
        ):
            with (
                self.subTest(tenant=tenant, migration=migration),
                self.assertRaises(ValueError),
            ):
                self.hold(tenant=tenant, migration=migration)
        self.connect.assert_not_called()

    def test_running_worker_or_changed_runtime_cannot_acquire_database_lock(self):
        self.worker.return_value = {"pid": 123}
        with self.assertRaisesRegex(ValueError, "停止"):
            self.hold()
        self.connect.assert_not_called()
        with mock.patch.object(
            gate, "verify_runtime", side_effect=ValueError("收据不匹配")
        ):
            with self.assertRaisesRegex(ValueError, "收据不匹配"):
                self.hold()
        self.connect.assert_not_called()

    def test_wrong_owner_or_preexisting_target_data_is_never_locked(self):
        self.lock.scope = "different-test"
        with self.assertRaisesRegex(ValueError, "ownership"):
            self.hold()
        self.assertEqual(len(self.lock.queries), 1)
        self.connect.side_effect = [self.observer, self.lock]
        self.lock.scope = SCOPE
        self.observer.counts["target"] = 1
        with self.assertRaisesRegex(ValueError, "目标数据为空"):
            self.hold()
        self.assertFalse(any("FOR UPDATE" in sql for sql in self.lock.queries))

    def test_started_or_other_migration_cannot_reuse_gate(self):
        for key, value in (
            ("state", "copying"),
            ("attempts", 1),
            ("job_migration_id", "999"),
            ("retention_hours", 1),
            ("target", "dedicated-a"),
        ):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "未执行"):
                gate.validate_pending([{**PENDING, key: value}], TENANT, MIGRATION)

    def test_ambiguous_evidence_and_timeout_close_both_sessions(self):
        self.observer.waits = [[{}, {}]]
        with self.assertRaisesRegex(ValueError, "多个请求"):
            self.hold('{"operation":"wait-blocked"}\n')
        self.assertTrue(self.observer.closed and self.lock.closed)
        self.connect.side_effect = [self.observer, self.lock]
        self.observer.waits = [[]]
        with mock.patch.object(gate.time, "monotonic", side_effect=[0, 0, 31]):
            with self.assertRaisesRegex(TimeoutError, "没有观察到"):
                self.hold('{"operation":"wait-blocked"}\n')
        self.assertTrue(self.observer.closed and self.lock.closed)

    def test_eof_or_unknown_command_closes_without_mutating_migration(self):
        self.hold()
        self.assertTrue(self.observer.closed and self.lock.closed)
        self.connect.side_effect = [self.observer, self.lock]
        with self.assertRaisesRegex(ValueError, "只支持"):
            self.hold('{"operation":"delete"}\n')
        self.assertTrue(self.observer.closed and self.lock.closed)
        self.assertFalse(
            any(
                sql.lstrip().startswith(("UPDATE ", "DELETE ", "INSERT "))
                for sql in self.observer.queries + self.lock.queries
            )
        )


class MigrationMysqlTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.configuration = self.root / "app.test.toml"
        self.configuration.write_text(
            """[[tenant_data.targets]]
key = "shared"
kind = "mysql"
mode = "shared"
host = "127.0.0.1"
port = 3317
database = "shared_test"
username = "root"
password_env = "TARGET_TEST_PASSWORD"
tls_mode = "disabled"
""",
            encoding="utf-8",
        )
        self.environment = mock.patch.dict(
            os.environ,
            {
                "APP_CONFIG_DIR": str(self.root),
                "APP_DATABASE_HOST": "127.0.0.1",
                "APP_DATABASE_PORT": "3317",
                "APP_DATABASE_NAME": "control_test",
                "APP_DATABASE_USERNAME": "root",
                "APP_DATABASE_TLS_MODE": "required",
                "APP_DATABASE_PASSWORD": "control-secret",
                "TARGET_TEST_PASSWORD": "target-secret",
                "RYFRAME_E2E_MYSQL_CLIENT": str(Path(sys.executable).resolve()),
            },
            clear=True,
        )
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def test_native_and_ci_keep_credentials_in_environment_and_bind_explicit_target(
        self,
    ):
        control, target = mysql.bindings(self.root)
        arguments, environment = mysql.invocation(target)
        self.assertEqual(arguments[1], "--no-defaults")
        self.assertIn("--port=3317", arguments)
        self.assertEqual(environment["MYSQL_PWD"], "target-secret")
        self.assertNotIn("secret", " ".join(arguments))
        with mock.patch.dict(
            os.environ,
            {
                "GITHUB_ACTIONS": "true",
                "GITHUB_RUN_ID": "123",
                "GITHUB_RUN_ATTEMPT": "2",
                "APP_SCOPE_ID": "ci-123-2",
                "RYFRAME_CI_MYSQL_CONTAINER_ID": "abcdef012345",
            },
        ):
            arguments, environment = mysql.invocation(control)
            self.assertEqual(
                arguments[:7],
                ["docker", "exec", "-i", "--env", "MYSQL_PWD", "abcdef012345", "mysql"],
            )
            self.assertIn("--port=3306", arguments)
            self.assertEqual(environment["MYSQL_PWD"], "control-secret")
            self.assertNotIn("secret", " ".join(arguments))
            with mock.patch.dict(os.environ, {"APP_SCOPE_ID": "ci-123-1"}):
                with self.assertRaisesRegex(ValueError, "当前 CI"):
                    mysql.invocation(control)

    def test_remote_shared_database_and_unverified_client_are_rejected(self):
        for name, value in (
            ("APP_DATABASE_HOST", "db.example"),
            ("APP_DATABASE_NAME", "shared_test"),
            ("APP_DATABASE_NAME", "db`;DROP DATABASE x"),
            ("APP_DATABASE_PORT", "3306"),
            ("APP_DATABASE_TLS_MODE", "verify_identity"),
        ):
            with (
                self.subTest(name=name, value=value),
                mock.patch.dict(os.environ, {name: value}),
            ):
                with self.assertRaises(ValueError):
                    mysql.bindings(self.root)
        control, _ = mysql.bindings(self.root)
        with mock.patch.dict(os.environ, {"RYFRAME_E2E_MYSQL_CLIENT": "mysql"}):
            with self.assertRaisesRegex(ValueError, "绝对路径"):
                mysql.invocation(control)

    def test_child_protocol_flushes_each_statement_and_rolls_back_on_exit(self):
        log = self.root / "statements.jsonl"
        script = """import json, re, sys
with open(sys.argv[1], 'w', encoding='utf-8') as output:
    for line in sys.stdin:
        output.write(json.dumps(line) + '\\n'); output.flush()
        marker = re.fullmatch(r"SELECT '(gate-[a-f0-9]+)';\\n", line)
        if marker: print(marker.group(1), flush=True)
        elif line == 'JSON;\\n': print('{"value":168}', flush=True)
"""
        with mock.patch.object(
            mysql,
            "invocation",
            return_value=(
                [sys.executable, "-u", "-c", script, str(log)],
                dict(os.environ),
            ),
        ):
            with mysql.MysqlSession({}) as session:
                self.assertEqual(session.execute("JSON", timeout=5), [{"value": 168}])
                self.assertEqual(session.execute("JSON", timeout=5), [{"value": 168}])
            self.assertIsNotNone(session.process.poll())
            self.assertFalse(any(reader.is_alive() for reader in session.readers))
        statements = [
            json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()
        ]
        self.assertIn("ROLLBACK;\n", statements)


if __name__ == "__main__":
    unittest.main()
