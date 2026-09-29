"""调度状态后像只读核对；不将模型结果记为真实 API 或恢复成功。"""
import copy
from decimal import Decimal
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from devex_clone_schedule import schedule_row_sha256
from devex_clone_schedule_check import inspect_disabled

REPO = Path(__file__).resolve().parents[2]


class ScheduleCheckTests(unittest.TestCase):
    def setUp(self):
        self.before = {"id": Decimal(3), "tenant_id": "system", "name": "数据保留清理",
                       "handler_key": "system.data_retention_cleanup", "cron_expression": "0 30 3 * * * *",
                       "timezone": "UTC", "enabled": Decimal(1), "misfire_policy": "fire_once",
                       "concurrency_policy": "forbid", "max_runtime_seconds": Decimal(900),
                       "next_run_at": "2026-09-05 03:30:00", "last_run_at": "2026-09-04 03:30:00",
                       "version": Decimal(7), "del_flag": "0", "created_at": "2026-09-01 00:00:00",
                       "updated_at": "2026-09-04 03:30:00"}
        self.after = {**self.before, "enabled": Decimal(0), "next_run_at": None, "version": Decimal(8),
                      "updated_at": "2026-09-04 07:00:01.123456"}
        self.action = {"action": "disable_schedule_via_api", "tenant_id": "system", "schedule_id": "3",
                       "handler_key": self.before["handler_key"], "expected_version": 7,
                       "source_row_sha256": schedule_row_sha256(self.before)}

    def inspect(self, row=None, started="2026-09-04 07:00:00", finished="2026-09-04 07:00:02"):
        return inspect_disabled(REPO, self.action, self.before, self.after if row is None else row, started, finished)

    def test_exact_current_transition_preserves_business_and_history_fields(self):
        originals = copy.deepcopy((self.before, self.after, self.action))
        result = self.inspect()
        self.assertEqual(result["status"], "disabled_row_verified")
        self.assertFalse(result["automatic_retry_allowed"])
        self.assertFalse(result["api_execution_proven"])
        self.assertFalse(result["target_ready"])
        self.assertEqual((self.before, self.after, self.action), originals)

    def test_unchanged_full_before_never_grants_automatic_retry(self):
        result = self.inspect(self.before)
        self.assertEqual(result["status"], "unchanged_before")
        self.assertFalse(result["automatic_retry_allowed"])
        self.assertTrue(result["worker_must_remain_stopped"])

    def test_unrelated_business_or_history_change_requires_reconciliation(self):
        for field, value in (("name", "其他名称"), ("handler_key", "system.other"),
                             ("cron_expression", "0 0 * * * * *"), ("timezone", "Asia/Shanghai"),
                             ("last_run_at", None), ("created_at", "2026-09-02 00:00:00"),
                             ("del_flag", "2"), ("id", Decimal(4)), ("tenant_id", "other")):
            with self.subTest(field=field):
                self.assertEqual(self.inspect({**self.after, field: value})["status"], "needs_reconciliation")

    def test_only_one_version_increment_null_next_run_and_database_window_are_valid(self):
        for field, value in (("version", Decimal(7)), ("version", Decimal(9)), ("enabled", Decimal(1)),
                             ("next_run_at", self.before["next_run_at"]), ("updated_at", None),
                             ("updated_at", "2026-09-04 06:59:59"), ("updated_at", "2026-09-04 07:00:03")):
            with self.subTest(field=field, value=value):
                self.assertEqual(self.inspect({**self.after, field: value})["status"], "needs_reconciliation")

    def test_full_original_source_hash_and_action_binding_required(self):
        for field, value in (("source_row_sha256", "a" * 64), ("schedule_id", "4"),
                             ("tenant_id", "other"), ("handler_key", "system.other"), ("expected_version", 6)):
            with self.subTest(field=field), self.assertRaises(ValueError):
                inspect_disabled(REPO, {**self.action, field: value}, self.before, self.after,
                                 "2026-09-04 07:00:00", "2026-09-04 07:00:02")

    def test_missing_or_extra_columns_ambiguous_integers_and_time_are_rejected(self):
        missing = dict(self.after)
        missing.pop("created_at")
        for row in (missing, {**self.after, "unknown": 1}, {**self.after, "version": True},
                    {**self.after, "id": Decimal("3.5")}, {**self.after, "updated_at": "2026-09-04T07:00:01+00:00"}):
            with self.subTest(row=row), self.assertRaises(ValueError):
                self.inspect(row)
        with self.assertRaises(ValueError):
            self.inspect(started="2026-09-04 07:00:03")


if __name__ == "__main__":
    unittest.main()
