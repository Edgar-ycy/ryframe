"""原目录接续的离线回归；真实账本配合明确协议替身，不声明真实复制通过。"""
import copy
from dataclasses import replace
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_factory as factory
from devex_clone_capture import read_json
from devex_clone_ledger import CloneLedger, LedgerError
from devex_clone_resume import continue_copy
from restore_reference_io import ObjectCreateError
import test_devex_clone_factory as fixtures


class ResumeTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.FactoryTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.output, self.backend = self.case.output, self.case.backend

    def continue_copy(self, mode, **environments):
        values = self.case.environments | environments
        return continue_copy(self.backend, self.output, values["source"], values["target"],
                             mode=mode, run=self.case.case.runner)

    def interrupted(self, failure="commit-response-lost"):
        self.case.case.runner.failure = failure
        expected = ObjectCreateError if failure == "put-response-lost" else subprocess.TimeoutExpired
        with self.assertRaises(expected):
            self.case.execute()
        self.case.case.runner.failure = None
        return len(self.case.case.writes())

    def test_lost_commit_requires_reconciliation_and_does_not_repeat_confirmed_write(self):
        self.interrupted()
        with self.assertRaises(LedgerError):
            self.continue_copy("resume")
        self.assertEqual(len(self.case.case.writes()), 1)
        before = (self.output / "failure.json").read_bytes()
        reconciled = self.continue_copy("reconcile")
        self.assertEqual(set(reconciled["steps"].values()), {"confirmed"})
        self.assertEqual(len(self.case.case.writes()), 1)
        result = self.continue_copy("resume")
        self.assertEqual(result["status"], "data_steps_verified")
        self.assertEqual(len(self.case.case.writes()), 3)
        self.assertEqual((self.output / "failure.json").read_bytes(), before)
        self.assertFalse(result["target_ready"])
        self.assertFalse(result["restore_success"])

    def test_reconciled_before_image_gets_one_explicit_new_attempt(self):
        self.interrupted("before-commit")
        reconciled = self.continue_copy("reconcile")
        self.assertEqual(set(reconciled["steps"].values()), {"reconciled_before"})
        self.assertEqual(len(self.case.case.writes()), 1)
        self.continue_copy("resume")
        state = self.case.case.ledger.inspect()
        self.assertEqual(sorted(item["attempt"] for item in state["steps"].values()), [1, 1, 2])
        self.assertEqual(len(self.case.case.writes()), 4)

    def test_object_response_lost_is_verified_without_repeating_put_or_database_writes(self):
        self.assertEqual(self.interrupted("put-response-lost"), 3)
        with self.assertRaises(LedgerError):
            self.continue_copy("resume")
        result = self.continue_copy("reconcile")
        self.assertEqual(set(result["steps"].values()), {"confirmed"})
        self.continue_copy("resume")
        self.assertEqual(len(self.case.case.writes()), 3)

    def test_prewrite_interruption_reuses_plan_export_and_initialized_directory(self):
        with patch.object(factory, "TransferSteps", side_effect=RuntimeError("offline interrupted before intent")):
            with self.assertRaises(RuntimeError):
                self.case.execute()
        paths = [self.output / name for name in ("plan.json", "input.json", "session.json")]
        original = {path: path.read_bytes() for path in paths}
        with patch.object(factory.CloneSession, "prepare", side_effect=AssertionError("must not prepare again")), \
                patch.object(factory, "verify_target", side_effect=AssertionError("copied target is not fresh")):
            result = self.continue_copy("reconcile")
            self.assertEqual(result["steps"], {})
            self.assertEqual(len(result["pending_steps"]), 3)
            self.continue_copy("resume")
        self.assertEqual(original, {path: path.read_bytes() for path in paths})
        self.assertEqual(len(self.case.case.writes()), 3)

    def test_abandoned_open_session_requires_reconciliation_before_expensive_hydration(self):
        with patch.object(factory, "TransferSteps", side_effect=RuntimeError("prewrite")):
            with self.assertRaises(RuntimeError):
                self.case.execute()
        saved = read_json(self.output / "session.json")
        ledger = CloneLedger(self.backend, self.output / "ledger", saved["plan_sha256"], saved["generation_sha256"], mode="reconcile")
        ledger.__enter__()
        ledger._release()
        # 此 fixture 留下 open 账本，同时模拟控制器退出后由内核释放互斥。
        ledger.guard.__exit__(None, None, None)
        with patch("devex_clone_resume.hydrate", side_effect=AssertionError("known interruption must fail early")):
            with self.assertRaisesRegex(LedgerError, "reconcile"):
                self.continue_copy("resume")
        self.assertEqual(len(self.case.case.writes()), 0)
        self.continue_copy("reconcile")
        self.continue_copy("resume")
        self.assertEqual(len(self.case.case.writes()), 3)

    def test_changed_confirmed_postimage_blocks_resume_before_any_new_write(self):
        self.interrupted()
        self.continue_copy("reconcile")
        key = "shared-control"
        current = self.case.case.guard.current[key]
        self.case.case.guard.current[key] = replace(current, tables=copy.deepcopy(self.case.case.guard.basis[key].initialized.tables))
        with self.assertRaisesRegex(ValueError, "已确认步骤"):
            self.continue_copy("resume")
        self.assertEqual(len(self.case.case.writes()), 1)

    def test_unregistered_target_changes_are_not_adopted_by_reconciliation(self):
        self.interrupted()
        key = "dedicated-a"
        current = self.case.case.guard.current[key]
        self.case.case.guard.current[key] = replace(current, tables=copy.deepcopy(self.case.case.guard.basis[key].source.tables))
        with self.assertRaisesRegex(ValueError, "未登记写入"):
            self.continue_copy("reconcile")
        self.assertEqual(len(self.case.case.writes()), 1)

    def test_changed_environment_or_source_image_cannot_rebind_original_session(self):
        self.interrupted()
        with self.assertRaisesRegex(ValueError, "原复制代次"):
            self.continue_copy("reconcile", source=self.case.environments["source"] | {"NEW_INPUT": "changed"})
        path = self.output / "session.json"
        saved = read_json(path)
        saved["source_observation"]["shared-control"]["tables"]["sys_tenant"]["sha256"] = "f" * 64
        path.write_text(json.dumps(saved), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "来源当前完整行像"):
            self.continue_copy("reconcile")
        self.assertEqual(len(self.case.case.writes()), 1)

    def test_target_or_ledger_lock_is_not_removed_or_replaced(self):
        self.interrupted()
        for lock in (self.case.target / "initialize.lock", self.output / "ledger/clone.lock"):
            lock.mkdir()
            identity = lock.stat().st_ino
            with self.assertRaises((FileExistsError, LedgerError)):
                self.continue_copy("reconcile")
            self.assertEqual(lock.stat().st_ino, identity)
            lock.rmdir()
        self.assertEqual(len(self.case.case.writes()), 1)

    def test_completed_copy_can_be_reverified_without_overwriting_original_result(self):
        self.case.execute()
        before = (self.output / "result.json").read_bytes()
        self.continue_copy("reconcile")
        self.continue_copy("resume")
        self.assertEqual((self.output / "result.json").read_bytes(), before)
        self.assertEqual(len(self.case.case.writes()), 3)
        saved = read_json(self.output / "session.json")
        ledger = CloneLedger(self.backend, self.output / "ledger", saved["plan_sha256"], saved["generation_sha256"])
        self.assertEqual(ledger.inspect()["status"], "ledger_evidence_complete")


if __name__ == "__main__":
    unittest.main()
