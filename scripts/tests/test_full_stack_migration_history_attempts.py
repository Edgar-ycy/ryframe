import copy
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_migration_mysql as migration_mysql
from test_full_stack_migration_history import (
    CONTRACT,
    MIGRATION,
    TENANT,
    history,
    shifted,
    snapshot,
    sql,
)


def retried_snapshot():
    row = snapshot()
    succeeded = row["attempt_records"][0]
    failed, expired = copy.deepcopy(succeeded), copy.deepcopy(succeeded)
    failed.update(sequence="1", outcome="failed")
    expired.update(sequence="2", outcome="lease_expired")
    expired["times"]["finished_at"] = None
    succeeded["sequence"] = "3"
    row.update(job_claim_sequence="3", attempt_records=[failed, expired, succeeded])
    return row


class AttemptHistoryTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def test_plan_reads_once_without_file_or_database_writes(self):
        before = retried_snapshot()
        session = mock.Mock()
        session.execute.return_value = [copy.deepcopy(before)]
        planned = history.plan_history(session, "SELECT exact_snapshot", CONTRACT)
        session.execute.assert_called_once_with("SELECT exact_snapshot")
        self.assertEqual(list(self.directory.iterdir()), [])
        self.assertEqual(planned["before"], before)
        self.assertEqual(planned["state"], "history-planned")
        later = copy.deepcopy(before)
        later["remaining_seconds"] -= 10
        later["attempt_records"].reverse()
        self.assertEqual(
            history.history_plan(CONTRACT, later)["plan_sha256"], planned["plan_sha256"]
        )

    def test_plan_binds_scope_tenant_job_and_all_attempt_timestamps(self):
        before = retried_snapshot()
        planned = history.history_plan(CONTRACT, before)
        for field, value in (
            ("scope_id", "other-scope"),
            ("server_uuid", "other-server"),
        ):
            with self.subTest(field=field):
                self.assertNotEqual(
                    history.history_plan({**CONTRACT, field: value}, before)[
                        "plan_sha256"
                    ],
                    planned["plan_sha256"],
                )
        changed = copy.deepcopy(before)
        changed["attempt_records"][0]["times"]["available_at"] = (
            "2026-09-03T00:00:00.000000Z"
        )
        session = mock.Mock()
        session.execute.return_value = [changed]
        with self.assertRaisesRegex(ValueError, "plan"):
            history.apply_history(
                session,
                "control_test",
                "snapshot",
                self.directory,
                CONTRACT,
                planned["plan_sha256"],
            )
        session.execute.assert_called_once_with("snapshot")
        self.assertEqual(list(self.directory.iterdir()), [])
        with self.assertRaisesRegex(ValueError, "ownership"):
            history.history_plan({**CONTRACT, "tenant_id": "other-tenant"}, before)

    def test_all_owned_attempts_shift_and_unknown_completion_stays_unknown(self):
        before = retried_snapshot()
        after = shifted(before)
        session = mock.Mock()
        session.execute.side_effect = [[before], [], [after], []]
        plan = history.history_plan(CONTRACT, before)
        result = history.apply_history(
            session,
            "control_test",
            "snapshot",
            self.directory,
            CONTRACT,
            plan["plan_sha256"],
        )
        self.assertIsNone(result["after"]["attempt_records"][1]["times"]["finished_at"])
        self.assertIsNotNone(
            result["after"]["attempt_records"][0]["times"]["finished_at"]
        )
        statement = session.execute.call_args_list[1].args[0]
        for expected in (
            "sys_background_job_attempt a",
            "j.id=a.job_id",
            "m.background_job_id=j.id",
            f"m.id={MIGRATION}",
            f"m.tenant_id='{TENANT}'",
            f"j.tenant_id='{TENANT}'",
            f"j.id={before['job_id']}",
            "a.sequence IN (1,2,3)",
            "j.job_type='tenant_data_migration'",
        ):
            self.assertIn(expected, statement)
        for field in sql.ATTEMPT_TIMES:
            self.assertIn(
                f"a.{field}=DATE_SUB(a.{field}, INTERVAL 169 HOUR)", statement
            )
        self.assertNotIn("COALESCE", statement)
        self.assertNotIn("outcome=", statement)
        self.assertEqual(session.execute.call_args.args, ("COMMIT",))

    def test_partial_attempt_shift_never_commits(self):
        before = retried_snapshot()
        after = shifted(before)
        after["attempt_records"][0]["times"] = before["attempt_records"][0]["times"]
        session = mock.Mock()
        session.execute.side_effect = [[before], [], [after]]
        with self.assertRaisesRegex(ValueError, "完整平移"):
            history.apply_history(
                session,
                "control_test",
                "snapshot",
                self.directory,
                CONTRACT,
                history.history_plan(CONTRACT, before)["plan_sha256"],
            )
        self.assertNotIn(mock.call("COMMIT"), session.execute.call_args_list)
        self.assertTrue((self.directory / f"device-history-{MIGRATION}.json").exists())

    def test_foreign_missing_duplicate_or_running_attempts_are_rejected(self):
        mutations = [
            lambda row: row["attempt_records"].pop(),
            lambda row: row["attempt_records"][0].update(job_id="999"),
            lambda row: row["attempt_records"][0].update(sequence="2"),
            lambda row: row["attempt_records"][0].update(outcome="running"),
            lambda row: row.update(job_claim_sequence="4"),
            lambda row: row["attempt_records"][0]["times"].update(finished_at=None),
            lambda row: row["attempt_records"][1]["times"].update(
                finished_at=row["job_times"]["completed_at"]
            ),
        ]
        for index, change in enumerate(mutations):
            row = retried_snapshot()
            change(row)
            with (
                self.subTest(index=index),
                self.assertRaisesRegex(ValueError, "ownership"),
            ):
                sql.validate_snapshot([row], TENANT, MIGRATION)

    def test_attempt_locks_are_bound_to_migration_and_tenant(self):
        session = mock.Mock()
        session.execute.side_effect = [[], [{"id": MIGRATION}], [], [], []]
        history.lock_records(session, "control_test", TENANT, MIGRATION)
        statement = session.execute.call_args.args[0]
        for expected in (
            "sys_background_job_attempt a",
            f"m.id={MIGRATION}",
            f"m.tenant_id='{TENANT}'",
            f"j.tenant_id='{TENANT}'",
            "ORDER BY a.sequence FOR UPDATE",
        ):
            self.assertIn(expected, statement)

    def test_write_without_read_only_plan_never_connects(self):
        with mock.patch.object(migration_mysql, "MysqlSession") as connect:
            with self.assertRaisesRegex(ValueError, "只读 plan"):
                history.run(
                    "historical-expired",
                    self.directory,
                    self.directory,
                    TENANT,
                    MIGRATION,
                )
            connect.assert_not_called()


if __name__ == "__main__":
    unittest.main()
