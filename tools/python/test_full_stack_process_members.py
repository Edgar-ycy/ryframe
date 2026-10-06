"""完整成员退出确认与失败传播边界，不启动外部服务。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
from full_stack_process import write_receipt
from full_stack_process_members import WindowsMembers, _same_process_creation, finish_members
from full_stack_process_monitor import receipt_path, wait_members

ROOT = Path(__file__).resolve().parents[2]


class ProcessMembersTests(unittest.TestCase):
    def test_pidfd_creation_check_allows_exec_but_rejects_pid_reuse(self):
        before = {"pid": 7, "started": "11", "executable": "/usr/bin/python"}
        after_exec = {**before, "executable": "/workspace/ryframe-worker"}
        reused = {**after_exec, "started": "12"}

        self.assertTrue(_same_process_creation(after_exec, before["started"]))
        self.assertFalse(_same_process_creation(reused, before["started"]))
        self.assertFalse(_same_process_creation(None, before["started"]))

    def test_atomic_receipt_publish_supports_deep_runtime_directory(self):
        directory = WorkspaceDirectory(dir=ROOT / ".local-tests")
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        while len(str(root)) < 180:
            root /= "深层 路径 xxxxxxxxxx"
        root.mkdir(parents=True)
        path = receipt_path(root, "rustfs", "a" * 32, "ready")

        write_receipt(path, {"status": "ready"})

        self.assertEqual(json.loads(path.read_text(encoding="utf-8")), {"status": "ready"})
        self.assertEqual(list(root.glob(".*.tmp")), [])

    def test_empty_job_list_does_not_skip_delayed_member_handle(self):
        members = Mock()
        members.capture.side_effect = [2, 0, 0]
        members.pending.side_effect = [True, False]
        members.identities.return_value = [{"pid": 3, "started": "1"}]
        with patch("full_stack_process_members.time.sleep") as sleep:
            result = finish_members(members)
        self.assertEqual(result, members.identities.return_value)
        self.assertEqual(sleep.call_count, 1)
        members.terminate.assert_called_once()
        self.assertEqual(members.capture.call_count, 3)

    def test_member_query_failure_does_not_report_success(self):
        members = Mock()
        members.capture.side_effect = OSError("query failed")
        with self.assertRaisesRegex(OSError, "query failed"):
            finish_members(members)
        members.identities.assert_not_called()

    def test_unsignaled_member_timeout_is_a_failure(self):
        members = Mock()
        members.capture.return_value = 0
        members.pending.return_value = True
        with self.assertRaisesRegex(TimeoutError, "完整进程树"):
            finish_members(members, timeout=0)
        members.identities.assert_not_called()

    def fixture(self):
        directory = WorkspaceDirectory(dir=ROOT / ".local-tests")
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        identity = {"pid": 2147482000, "started": "1", "executable": str(Path(sys.executable).resolve())}
        receipt = {"runtime_directory": str(root), "role": "api", "scope_id": "member-test",
                   "operation_id": "a" * 32, "monitor": identity, "supervisor": {**identity, "pid": 2147482001}}
        proof = {"format_version": 1, "directory": str(root),
                 **{key: receipt[key] for key in ("role", "scope_id", "operation_id", "monitor", "supervisor")},
                 "status": "stopped", "error_type": None, "members": [receipt["supervisor"]]}
        return receipt, proof

    def test_pid_reuse_never_accepts_an_existing_success_receipt(self):
        receipt, proof = self.fixture()
        write_receipt(receipt_path(Path(receipt["runtime_directory"]), "api", "a" * 32, "stopped"), proof)
        changed = {**receipt["monitor"], "started": "2"}
        with patch("full_stack_process_monitor.process_identity", return_value=changed), \
                self.assertRaisesRegex(ValueError, "PID 已复用"):
            wait_members(receipt, timeout=0)

    def test_monitor_failure_and_missing_receipt_propagate(self):
        receipt, proof = self.fixture()
        with patch("full_stack_process_monitor.process_identity", return_value=None), \
                self.assertRaisesRegex(ValueError, "没有完整成员"):
            wait_members(receipt, timeout=0)
        proof.update(status="failed", error_type="PermissionError")
        write_receipt(receipt_path(Path(receipt["runtime_directory"]), "api", "a" * 32, "stopped"), proof)
        with patch("full_stack_process_monitor.process_identity", return_value=None), \
                self.assertRaisesRegex(ValueError, "PermissionError"):
            wait_members(receipt, timeout=0)

    def test_old_or_other_operation_proof_cannot_establish_completion(self):
        receipt, proof = self.fixture()
        path = receipt_path(Path(receipt["runtime_directory"]), "api", "a" * 32, "stopped")
        for changed in ({**proof, "operation_id": "b" * 32}, {**proof, "members": []},
                        {**proof, "monitor": {**proof["monitor"], "started": "2"}}):
            write_receipt(path, changed)
            with patch("full_stack_process_monitor.process_identity", return_value=None), self.assertRaises(ValueError):
                wait_members(receipt, timeout=0)

    @unittest.skipUnless(os.name == "nt", "真实 Windows Job 名称占用")
    def test_job_name_collision_does_not_take_over_an_existing_job(self):
        import uuid
        from full_stack_process import process_identity
        operation = uuid.uuid4().hex
        original = WindowsMembers(operation, process_identity(os.getpid()))
        try:
            with self.assertRaisesRegex(ValueError, "名称已经被占用"):
                WindowsMembers(operation, process_identity(os.getpid()))
            self.assertEqual(original.capture(), 0)
        finally:
            original.close()


if __name__ == "__main__":
    unittest.main()
