"""失败收据早于监督器退出时，恢复不能提前释放控制锁。"""

from contextlib import ExitStack
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import full_stack_worker as worker


class CompletedReconciliationTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / worker.LOCK
        self.path.mkdir()
        self.operation = {"path": self.path, "owner": {"operation_id": "a" * 32}}
        self.identity = {"pid": 4321, "creation_time": "original", "executable": "worker"}
        self.result = {"operation": "start", "outcome": "failed", "error": "就绪失败"}
        stack = self.enterContext(ExitStack())
        self.validated = stack.enter_context(
            mock.patch.object(worker, "_validate_result", return_value=self.result)
        )
        self.child = stack.enter_context(
            mock.patch.object(worker, "_validate_child", return_value={"identity": self.identity})
        )
        self.identity_query = stack.enter_context(
            mock.patch.object(worker, "process_identity", return_value=self.identity)
        )
        self.archive = stack.enter_context(
            mock.patch.object(worker, "_archive", return_value=self.result)
        )
        self.cleanup = stack.enter_context(
            mock.patch("full_stack_process_monitor.wait_startup_cleanup")
        )
        (self.path / "supervisor.json").write_text("{}", encoding="utf-8")

    def reconcile(self):
        return worker._archive_completed(self.operation, {"scope_id": "test-scope"})

    def test_failed_result_waits_for_live_supervisor_without_archiving(self):
        with self.assertRaisesRegex(worker._PendingStart, "等待监督进程退出"):
            self.reconcile()
        self.archive.assert_not_called()
        self.cleanup.assert_not_called()
        self.assertTrue(self.path.is_dir())

    def test_failed_result_requires_complete_member_cleanup_before_archive(self):
        self.identity_query.return_value = None
        order = []
        self.cleanup.side_effect = lambda *args: order.append("cleanup")
        self.archive.side_effect = lambda *args: order.append("archive") or self.result
        self.assertEqual(self.reconcile(), self.result)
        self.assertEqual(order, ["cleanup", "archive"])
        self.cleanup.assert_called_once_with(
            self.path.parent, "worker", "test-scope", "a" * 32, self.identity
        )

    def test_cleanup_failure_retains_operation_and_original_failure(self):
        self.identity_query.return_value = None
        self.cleanup.side_effect = ValueError("完整成员关闭证明缺失")
        with self.assertRaisesRegex(ValueError, "完整成员关闭证明缺失"):
            self.reconcile()
        self.archive.assert_not_called()
        self.assertEqual(self.result["error"], "就绪失败")
        self.assertTrue(self.path.exists())

    def test_reused_supervisor_pid_is_rejected(self):
        self.identity_query.return_value = {**self.identity, "creation_time": "reused"}
        with self.assertRaisesRegex(ValueError, "复用的 PID"):
            self.reconcile()
        self.archive.assert_not_called()
        self.cleanup.assert_not_called()

    def test_successful_start_keeps_live_supervisor(self):
        self.result["outcome"] = "succeeded"
        self.assertEqual(self.reconcile(), self.result)
        self.identity_query.assert_not_called()
        self.cleanup.assert_not_called()
        self.archive.assert_called_once_with(self.operation)

    def test_failed_candidate_waits_without_claiming_authorized_members(self):
        (self.path / "supervisor.json").rename(self.path / "candidate.json")
        with self.assertRaises(worker._PendingStart):
            self.reconcile()
        self.identity_query.return_value = None
        self.assertEqual(self.reconcile(), self.result)
        self.cleanup.assert_not_called()
        self.child.assert_called_with(
            self.operation, "candidate.json", "full-stack-worker-control-candidate"
        )

    def test_invalid_result_and_child_receipts_never_release_lock(self):
        self.validated.side_effect = ValueError("控制结果与当前请求不匹配")
        with self.assertRaisesRegex(ValueError, "当前请求不匹配"):
            self.reconcile()
        self.validated.side_effect = None
        self.child.side_effect = ValueError("监督授权不属于当前操作")
        with self.assertRaisesRegex(ValueError, "不属于当前操作"):
            self.reconcile()
        self.archive.assert_not_called()

    def test_other_completed_operations_do_not_wait_for_startup(self):
        self.result["operation"] = "status"
        self.assertEqual(self.reconcile(), self.result)
        self.child.assert_not_called()
        self.cleanup.assert_not_called()


if __name__ == "__main__":
    unittest.main()
