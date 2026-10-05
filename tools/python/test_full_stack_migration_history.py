import copy
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import full_stack_migration_backup as backup
import full_stack_migration_history as history
import full_stack_migration_history_sql as sql
import full_stack_migration_mysql as migration_mysql
import full_stack_runtime as runtime
import full_stack_worker as worker

TENANT, MIGRATION = "tenant-0123abcd", "123456789"
CONTRACT = {
    "scope_id": "history-test",
    "tenant_id": TENANT,
    "migration_id": MIGRATION,
    "server_uuid": "server",
    "schemas": ["control_test", "source_test", "target_test"],
    "configuration_sha256": "a" * 64,
}


def timestamp(value):
    return value.isoformat(timespec="microseconds").replace("+00:00", "Z")


def snapshot():
    moment = datetime(2026, 9, 4, tzinfo=timezone.utc)
    row = {
        "migration_id": MIGRATION,
        "tenant_id": TENANT,
        "state": "retention_pending",
        "source": "dedicated-a",
        "target": "dedicated-b",
        "source_mode": "dedicated",
        "target_mode": "dedicated",
        "source_generation": "1",
        "target_generation": "2",
        "fingerprint": "c" * 64,
        "retention_hours": 168,
        "retention_seconds": 168 * 3600,
        "remaining_seconds": 168 * 3600 - 10,
        "intent_clear": 1,
        "job_id": "123456790",
        "job_status": "succeeded",
        "job_attempts": 1,
        "job_tenant_id": TENANT,
        "job_migration_id": MIGRATION,
        "job_lease_clear": 1,
        "item_id": "123456791",
        "item_state": "verified",
        "item_source_count": 3,
        "item_target_count": 3,
        "source_digest": "b" * 64,
        "target_digest": "b" * 64,
        "cleanup_state": "pending",
        "cleanup_count": 0,
        "placement_matches": 1,
        "target_fence_matches": 1,
        "target_slot_matches": 1,
        "source_fence_matches": 1,
        "source_slot_matches": 1,
        "source_fences": 1,
        "source_slots": 1,
        "source_count": 3,
        "target_count": 3,
        "source_total": 3,
        "target_total": 3,
        "other_migrations": 0,
        "operation_leases": 0,
    }
    for key, columns in (
        ("migration_times", sql.MIGRATION_TIMES),
        ("item_times", sql.ITEM_TIMES),
        ("job_times", sql.JOB_TIMES),
    ):
        row[key] = {column: timestamp(moment) for column in columns}
    row["migration_times"]["retention_until"] = timestamp(moment + timedelta(hours=168))
    row["job_claim_sequence"] = "1"
    row["attempt_records"] = [
        {
            "job_id": row["job_id"],
            "sequence": "1",
            "outcome": "succeeded",
            "times": {field: timestamp(moment) for field in sql.ATTEMPT_TIMES},
        }
    ]
    return row


def shifted(row):
    after = copy.deepcopy(row)
    after["remaining_seconds"] -= 169 * 3600
    for key in ("migration_times", "item_times", "job_times"):
        after[key] = {
            field: (datetime.fromisoformat(value) - timedelta(hours=169))
            .isoformat(timespec="microseconds")
            .replace("+00:00", "Z")
            for field, value in row[key].items()
        }
    for item in after["attempt_records"]:
        item["times"] = {
            field: (datetime.fromisoformat(value) - timedelta(hours=169))
            .isoformat(timespec="microseconds")
            .replace("+00:00", "Z")
            if value is not None
            else None
            for field, value in item["times"].items()
        }
    return after


class HistoryTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def test_only_write_operations_accept_explicit_write_intent(self):
        for operation in history.WRITE_OPERATIONS:
            history.validate_write_intent(operation, True)
            with self.assertRaisesRegex(ValueError, "--write"):
                history.validate_write_intent(operation, False)
        for operation in ("inspect", "plan-history", "verify-cleaned"):
            history.validate_write_intent(operation, False)
            with self.assertRaisesRegex(ValueError, "不接受"):
                history.validate_write_intent(operation, True)

    def test_complete_shift_retains_policy_state_and_original_evidence(self):
        before, after = snapshot(), shifted(snapshot())
        session = mock.Mock()
        session.execute.side_effect = [[before], [], [after], []]
        result = history.apply_history(
            session,
            "control_test",
            "snapshot",
            self.directory,
            CONTRACT,
            history.history_plan(CONTRACT, snapshot())["plan_sha256"],
        )
        self.assertEqual(result["state"], "historical-expired")
        self.assertFalse(result["natural_elapsed_7_days"])
        self.assertEqual(result["retention_hours"], 168)
        recorded = json.loads(
            (self.directory / f"business-history-{MIGRATION}.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(recorded["before"], before)
        update = session.execute.call_args_list[1].args[0]
        for table, key in (
            ("sys_tenant_data_migration", "migration_id"),
            ("sys_tenant_data_migration_item", "item_id"),
            ("sys_background_job", "job_id"),
        ):
            self.assertIn(f"`control_test`.{table}", update)
            self.assertIn(f"WHERE id={before[key]}", update)
        self.assertNotIn("retention_hours=", update)
        self.assertNotIn("state=", update)
        self.assertEqual(session.execute.call_args.args, ("COMMIT",))
        session.reset_mock(side_effect=True)
        session.execute.return_value = [before]
        with self.assertRaises(FileExistsError):
            history.apply_history(
                session,
                "control_test",
                "snapshot",
                self.directory,
                CONTRACT,
                history.history_plan(CONTRACT, snapshot())["plan_sha256"],
            )
        self.assertEqual(session.execute.call_count, 1)

    def test_shared_target_stale_job_foreign_owner_and_partial_cleanup_are_rejected(
        self,
    ):
        changes = {
            "target_mode": "shared",
            "job_tenant_id": "other-tenant",
            "job_migration_id": "999",
            "job_status": "running",
            "job_lease_clear": 0,
            "target_slot_matches": 0,
            "source_fence_matches": 0,
            "source_slot_matches": 0,
            "target_total": 4,
            "other_migrations": 1,
            "operation_leases": 1,
            "retention_hours": 1,
            "cleanup_state": "cleaned",
        }
        for key, value in changes.items():
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "ownership"):
                sql.validate_snapshot([{**snapshot(), key: value}], TENANT, MIGRATION)

    def test_partial_time_shift_and_unrelated_changes_fail_before_commit(self):
        before, after = snapshot(), shifted(snapshot())
        after["item_times"]["copied_at"] = before["item_times"]["copied_at"]
        session = mock.Mock()
        session.execute.side_effect = [[before], [], [after]]
        with self.assertRaisesRegex(ValueError, "完整平移"):
            history.apply_history(
                session,
                "control_test",
                "snapshot",
                self.directory,
                CONTRACT,
                history.history_plan(CONTRACT, snapshot())["plan_sha256"],
            )
        self.assertNotIn(mock.call("COMMIT"), session.execute.call_args_list)
        with self.assertRaisesRegex(ValueError, "时间以外"):
            sql.verify_shift(before, {**shifted(before), "fingerprint": "d" * 64})

    def test_natural_expiry_does_not_authorize_repeated_artificial_age(self):
        session = mock.Mock()
        session.execute.return_value = [{**snapshot(), "remaining_seconds": -1}]
        with self.assertRaisesRegex(ValueError, "本次刚完成"):
            history.apply_history(
                session,
                "control_test",
                "snapshot",
                self.directory,
                CONTRACT,
                history.history_plan(CONTRACT, snapshot())["plan_sha256"],
            )
        self.assertEqual(session.execute.call_count, 1)
        self.assertEqual(list(self.directory.iterdir()), [])

    def test_backup_failure_never_registers_or_commits(self):
        after = shifted(snapshot())
        history.write_evidence(
            self.directory / f"business-history-{MIGRATION}.json",
            {**CONTRACT, "artificial_history": True, "before": snapshot()},
        )
        session = mock.Mock()
        session.execute.side_effect = [
            [after],
            [{"count": 0}],
            [{"tenant_id": TENANT, "generation": "2"}],
            [],
        ]
        with mock.patch.object(
            backup, "export_target", side_effect=ValueError("导出失败")
        ):
            with self.assertRaisesRegex(ValueError, "导出失败"):
                history.backup_history(
                    session,
                    "control_test",
                    {"database": "target_test"},
                    "snapshot",
                    self.directory,
                    CONTRACT,
                )
        self.assertFalse(
            any(
                call.args[0].startswith(("INSERT", "COMMIT"))
                for call in session.execute.call_args_list
            )
        )

    def test_backup_registration_requires_exact_write_fence_and_retains_real_artifact(
        self,
    ):
        before, after = snapshot(), shifted(snapshot())
        history.write_evidence(
            self.directory / f"business-history-{MIGRATION}.json",
            {**CONTRACT, "artificial_history": True, "before": before},
        )
        session = mock.Mock()
        session.execute.side_effect = [[after], [{"count": 0}], []]
        with mock.patch.object(backup, "export_target") as export:
            with self.assertRaisesRegex(ValueError, "精确写入 fence"):
                history.backup_history(
                    session,
                    "control_test",
                    {"database": "target_test"},
                    "snapshot",
                    self.directory,
                    CONTRACT,
                )
            export.assert_not_called()
        session.reset_mock(side_effect=True)
        session.execute.side_effect = [
            [after],
            [{"count": 0}],
            [{"tenant_id": TENANT, "generation": "2"}],
            [],
            [after],
            [],
            [],
        ]
        artifact_path = self.directory / f"business-retention-{MIGRATION}.sql"

        def exported(*_args):
            self.assertIn("FOR UPDATE", session.execute.call_args_list[2].args[0])
            artifact_path.write_bytes(b"actual exported content")
            return {
                "path": str(artifact_path),
                "bytes": artifact_path.stat().st_size,
                "sha256": hashlib.sha256(artifact_path.read_bytes()).hexdigest(),
                "restored": False,
            }

        with mock.patch.object(backup, "export_target", side_effect=exported):
            result = history.backup_history(
                session,
                "control_test",
                {"database": "target_test"},
                "snapshot",
                self.directory,
                CONTRACT,
            )
        self.assertTrue(result["tenant_write_fence_held"])
        self.assertFalse(result["restored"])
        self.assertTrue((self.directory / f"business-backup-{MIGRATION}.json").is_file())
        self.assertEqual(session.execute.call_args.args, ("COMMIT",))

    def test_runtime_or_worker_mismatch_never_connects_to_database(self):
        with mock.patch.object(migration_mysql, "MysqlSession") as connect:
            with mock.patch.object(
                runtime, "verify_runtime", side_effect=ValueError("APP_ENV=test")
            ):
                with self.assertRaisesRegex(ValueError, "APP_ENV=test"):
                    history.run(
                        "historical-expired",
                        self.directory,
                        self.directory,
                        TENANT,
                        MIGRATION,
                        "a" * 64,
                    )
            with (
                mock.patch.object(
                    runtime, "verify_runtime", return_value={"scope_id": "test"}
                ),
                mock.patch.object(
                    worker, "worker_identity", return_value={"pid": 123}
                ),
            ):
                with self.assertRaisesRegex(ValueError, "停止"):
                    history.run(
                        "export-backup",
                        self.directory,
                        self.directory,
                        TENANT,
                        MIGRATION,
                    )
            connect.assert_not_called()

    def test_actual_dump_completion_and_digest_are_required_before_registration(self):
        content = (
            b"CREATE TABLE `biz_order` ();\n"
            + b"INSERT INTO `biz_order` VALUES (1);\n" * 3
        )

        def complete(command, **kwargs):
            self.assertEqual(command[-2:], ["--databases", "target_test"])
            self.assertIn("--single-transaction", command)
            self.assertIn("--skip-extended-insert", command)
            self.assertNotIn("secret", " ".join(command))
            kwargs["stdout"].write(content)
            return subprocess.CompletedProcess(command, 0)

        with (
            mock.patch.object(
                backup,
                "client_invocation",
                return_value=(["mysqldump"], {"MYSQL_PWD": "secret"}),
            ),
            mock.patch.object(backup.subprocess, "run", side_effect=complete),
        ):
            artifact = backup.export_target(
                {"database": "target_test"}, self.directory, MIGRATION
            )
        self.assertEqual(artifact["sha256"], hashlib.sha256(content).hexdigest())
        self.assertFalse(artifact["restored"])
        backup.verify_artifact(artifact, self.directory, MIGRATION)
        Path(artifact["path"]).write_bytes(content + b"damaged")
        with self.assertRaisesRegex(ValueError, "摘要已变化"):
            backup.verify_artifact(artifact, self.directory, MIGRATION)

    def test_failed_or_incomplete_dump_is_not_a_valid_backup(self):
        def failed(command, **kwargs):
            kwargs["stdout"].write(b"incomplete")
            return subprocess.CompletedProcess(command, 1)

        with (
            mock.patch.object(
                backup, "client_invocation", return_value=(["mysqldump"], {})
            ),
            mock.patch.object(backup.subprocess, "run", side_effect=failed),
        ):
            with self.assertRaisesRegex(ValueError, "不登记备份"):
                backup.export_target(
                    {"database": "target_test"}, self.directory, MIGRATION
                )
        self.assertTrue(
            (self.directory / f"business-retention-{MIGRATION}.sql").is_file()
        )

        def incomplete(command, **kwargs):
            kwargs["stdout"].write(b"CREATE TABLE `biz_order` ();\n")
            return subprocess.CompletedProcess(command, 0)

        with (
            mock.patch.object(
                backup, "client_invocation", return_value=(["mysqldump"], {})
            ),
            mock.patch.object(backup.subprocess, "run", side_effect=incomplete),
        ):
            with self.assertRaisesRegex(ValueError, "三条独立记录"):
                backup.export_target(
                    {"database": "target_test"}, self.directory, "123456790"
                )

    def test_backup_uses_tenant_generation_and_current_capture_time(self):
        statement = backup.register_sql(
            "control_test", snapshot(), "full-stack://history-test/id", "a" * 64
        )
        for required in (
            "'tenant',m.tenant_id,m.target_key,m.target_generation,m.target_schema_fingerprint",
            "@backup_captured_at",
            "INTERVAL 7 DAY",
            "WHERE m.id=123456789 AND m.tenant_id='tenant-0123abcd'",
            "未执行恢复验证",
        ):
            self.assertIn(required, statement)
        self.assertNotIn("'shard'", statement)

    def test_cleanup_preserves_current_target_and_requires_source_fence_removal(self):
        finalized = {
            **shifted(snapshot()),
            "state": "finalized",
            "intent_clear": 0,
            "cleanup_state": "cleaned",
            "cleanup_count": 3,
            "source_count": 0,
            "source_total": 0,
            "source_fence_matches": 0,
            "source_slot_matches": 0,
            "source_fences": 0,
            "source_slots": 0,
        }
        self.assertEqual(
            sql.validate_snapshot([finalized], TENANT, MIGRATION, finalized=True),
            finalized,
        )
        for key, value in (
            ("target_count", 0),
            ("source_fences", 1),
            ("source_slots", 1),
            ("cleanup_count", 2),
        ):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "ownership"):
                sql.validate_snapshot(
                    [{**finalized, key: value}], TENANT, MIGRATION, finalized=True
                )


if __name__ == "__main__":
    unittest.main()
