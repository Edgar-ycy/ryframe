"""合法全局清理任务与调度历史的关联回归；仅检查离线完整 SQL。"""
import copy
from pathlib import Path
import re
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import test_devex_clone as base
from devex_clone_jobs_fixture import CLEANUPS, JSON_TIME, SQL_TIME, add_cleanup, bind_actions
from devex_clone_model import create_plan
from devex_clone_job_relations import PAYLOAD_FIELDS, SYSTEM_CLEANUP_JOBS, TRIGGERS, positive_id


class CleanupRelationTests(unittest.TestCase):
    def setUp(self):
        self.case = base.CloneTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)

    def validate(self):
        return create_plan(bind_actions(self.case), self.case.backend)

    def test_three_real_system_cleanup_shapes_keep_null_parent_tenant(self):
        for index in range(len(CLEANUPS)):
            add_cleanup(self.case, index)
        result = self.validate()
        self.assertEqual(result["status"], "offline_verified_pending_target_actions")
        self.assertEqual(len(result["pending_target_actions"]), 3)
        self.assertFalse(result["execution_authorized"])

    def test_scheduled_manual_and_misfire_terminal_cleanup_are_supported(self):
        original = copy.deepcopy(self.case.data)
        for trigger in ("scheduled", "manual", "misfire"):
            for terminal in ("succeeded", "dead"):
                with self.subTest(trigger=trigger, terminal=terminal):
                    self.case.data = copy.deepcopy(original)
                    add_cleanup(self.case, trigger=trigger, terminal=terminal)
                    self.validate()

    def test_wrong_scope_type_schedule_and_execution_identity_are_rejected(self):
        schedule, job, execution = add_cleanup(self.case)
        changes = (
            (schedule, "tenant_id", "clone-source-01"), (execution, "tenant_id", "clone-source-01"),
            (job, "tenant_id", "system"), (job, "tenant_id", "clone-source-01"),
            (schedule, "handler_key", "system.unknown_cleanup"), (job, "job_type", "system.message.retention"),
            (job, "job_type", "unknown"), (job, "schedule_id", None), (job, "schedule_id", 999),
            (execution, "schedule_id", None), (execution, "schedule_id", 999),
            (execution, "background_job_id", 999), (schedule, "id", 999),
        )
        for row, field, bad in changes:
            with self.subTest(field=field, bad=bad):
                old = row[field]; row[field] = bad
                with self.assertRaises(ValueError):
                    self.validate()
                row[field] = old

    def test_wrong_time_precision_timezone_and_payload_are_rejected(self):
        _, job, execution = add_cleanup(self.case)
        changes = (
            (job, "scheduled_for", None), (job, "scheduled_for", "2026-09-01 00:00:00.123457"),
            (execution, "scheduled_for", "2026-09-01T00:00:00.123456Z"),
            (execution, "trigger_kind", "unknown"), (job, "payload", None), (job, "payload", []),
            (job, "payload", {"schedule_id": "101", "trigger_kind": "scheduled"}),
            (job, "payload", {"schedule_id": 101, "trigger_kind": "scheduled", "scheduled_for": JSON_TIME}),
            (job, "payload", {"schedule_id": "102", "trigger_kind": "scheduled", "scheduled_for": JSON_TIME}),
            (job, "payload", {"schedule_id": "101", "trigger_kind": "manual", "scheduled_for": JSON_TIME}),
        )
        for row, field, bad in changes:
            with self.subTest(field=field, bad=bad):
                old = row[field]; row[field] = bad
                with self.assertRaises(ValueError):
                    self.validate()
                row[field] = old
        for bad in (SQL_TIME, "2026-09-01T00:00:00.123457Z", "2026-09-01T00:00:00.1234561Z",
                    "2026-09-01T08:00:00.123456+08:00", "2026-02-31T00:00:00Z"):
            with self.subTest(time=bad):
                job["payload"]["scheduled_for"] = bad
                with self.assertRaises(ValueError):
                    self.validate()
        job["payload"]["scheduled_for"] = JSON_TIME
        job["payload"]["unreviewed"] = "extra"
        with self.assertRaises(ValueError):
            self.validate()

    def test_nonterminal_leased_unclosed_or_missing_parent_is_rejected(self):
        _, job, execution = add_cleanup(self.case)
        for field, bad in (("status", "pending"), ("status", "running"), ("status", "failed"),
                           ("completed_at", None), ("lease_owner", "worker"), ("lease_until", SQL_TIME)):
            with self.subTest(field=field):
                old = job[field]; job[field] = bad
                with self.assertRaises(ValueError):
                    self.validate()
                job[field] = old
        self.case.data["shared-control"] = [(t, r) for t, r in self.case.data["shared-control"] if t != "sys_background_job"]
        with self.assertRaisesRegex(ValueError, "缺少父任务"):
            self.validate()
        execution["background_job_id"] = None
        with self.assertRaises(ValueError):
            self.validate()

    def test_unreviewed_null_parent_cannot_use_same_null_execution_tenant(self):
        schedule, job, execution = add_cleanup(self.case)
        schedule.update(enabled=0, handler_key="custom.cleanup")
        job["job_type"] = "custom.cleanup"
        execution["tenant_id"] = None
        with self.assertRaises(ValueError):
            self.validate()

    def test_handler_job_types_payload_and_triggers_match_current_rust_contract(self):
        root = Path(__file__).resolve().parents[2] / "crates/ryframe-application/src"
        targets = (root / "jobs/schedule_targets.rs").read_text(encoding="utf-8")
        specs = (
            ("ExportCleanupTarget", "EXPORT_CLEANUP_JOB_TYPE", "system/export/types.rs"),
            ("MessageRetentionTarget", "MESSAGE_RETENTION_JOB_TYPE", "system/message.rs"),
            ("DataRetentionTarget", "DATA_RETENTION_JOB_TYPE", "system/data_retention/mod.rs"),
        )
        declared = {}
        for target, constant, filename in specs:
            section = targets.split("impl ScheduledJobTarget for " + target + " {", 1)[1].split("\n}\n", 1)[0]
            handler = re.search(r'fn handler_key\(&self\).*?"([a-z_.]+)"', section, re.S)[1]
            value = re.search(r"pub const " + constant + r': &str = "([a-z_.]+)";', (root / filename).read_text(encoding="utf-8"))[1]
            self.assertIn("ScheduledJobTargetScope::System", section)
            self.assertRegex(section, r"fn job_type\(&self\)[^{]+\{\s*" + constant)
            self.assertIn("build_system_cleanup_job(self, context)", section)
            declared[handler] = value
        self.assertEqual(SYSTEM_CLEANUP_JOBS, declared)
        builder = targets.split("fn build_system_cleanup_job(", 1)[1]
        self.assertIn("tenant_id: None", builder)
        payload = builder.split("payload: serde_json::json!({", 1)[1].split("}),", 1)[0]
        self.assertEqual(set(re.findall(r'"([a-z_]+)":', payload)), PAYLOAD_FIELDS)
        for value in ("context.schedule_id.to_string()", "context.trigger_kind", "context.scheduled_for"):
            self.assertIn(value, payload)
        module = (root / "jobs/schedule/mod.rs").read_text(encoding="utf-8")
        self.assertEqual(set(re.findall(r'const TRIGGER_[A-Z]+: &str = "([a-z]+)";', module)), TRIGGERS)

    def test_invalid_numeric_ids_and_completed_before_schedule_are_rejected(self):
        from decimal import Decimal
        for value in (None, "101", True, 0, -1, 2**63, Decimal("NaN"), Decimal("Infinity"), Decimal("1.5")):
            with self.subTest(value=str(value)), self.assertRaises(ValueError):
                positive_id(value)
        _, job, _ = add_cleanup(self.case)
        job["completed_at"] = "2026-09-01 00:00:00.123455"
        with self.assertRaises(ValueError):
            self.validate()


if __name__ == "__main__":
    unittest.main()
