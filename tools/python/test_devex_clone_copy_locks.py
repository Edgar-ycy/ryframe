"""核对复制锁的登记、排他和释放，不连接业务服务。"""
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_copy_locks as locks
from devex_clone_ledger import CloneLedger
from devex_clone_target_state import generation_lock
from full_stack_process import write_receipt
from process_guard import process_guard
from restore_build import file_digest


class CopyLockTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.output, self.target = self.local / "copy", self.local / "target"
        self.output.mkdir()
        self.target.mkdir()
        self.source = self.bind(self.local / "export.json", {"source": "fixed"})
        self.initialized = self.bind(self.target / "initialized.json", {"target": "fixed"})
        self.bind(self.output / "session.json", {"source_export": self.source, "initialized": self.initialized,
                  "plan_sha256": "a" * 64, "generation_sha256": "b" * 64})

    def bind(self, path, value):
        write_receipt(path, value)
        return {"path": str(path), **file_digest(path)}

    def test_live_owner_and_same_guard_concurrency_are_rejected(self):
        owner = locks.register_copy_owner(self.backend, self.output)
        with process_guard(self.target, locks.GUARDS["target"]):
            with self.assertRaisesRegex(ValueError, "并发"):
                with generation_lock(self.target, copy_owner=owner):
                    self.fail("guard 必须禁止新控制器抢锁")

    def test_graceful_copy_lock_release_keeps_registered_owner_receipt(self):
        owner = locks.register_copy_owner(self.backend, self.output)
        with generation_lock(self.target, copy_owner=owner):
            with CloneLedger(self.backend, self.output / "ledger", "a" * 64, "b" * 64,
                             create=True, copy_owner=owner) as ledger:
                self.assertEqual(ledger.session_id, owner.session_id)
                self.assertTrue((self.output / "ledger/clone.lock/owner.json").exists())
        self.assertTrue(Path(owner.binding["path"]).exists())
        self.assertFalse((self.target / "initialize.lock").exists())
        self.assertFalse((self.output / "ledger/clone.lock").exists())

    def test_publication_failure_reacquires_registered_lock_and_does_not_leave_bare_lock(self):
        owner = locks.register_copy_owner(self.backend, self.output)
        ledger = CloneLedger(self.backend, self.output / "ledger", "a" * 64, "b" * 64,
                             create=True, copy_owner=owner)
        with self.assertRaises(ExceptionGroup), patch.object(ledger, "_publish", side_effect=OSError("fixture publish")):
            with ledger:
                ledger.finishing = True
        self.assertFalse((self.output / "ledger/clone.lock").exists())
        self.assertEqual(ledger.inspect()["status"], "needs_reconciliation")
        self.assertTrue(any(frame["event"] == {"type": "fault", "stage": "receipt_write"}
                            for frame in ledger.frames))

    def test_release_failure_preserves_owned_lock_and_fault_evidence(self):
        owner = locks.register_copy_owner(self.backend, self.output)
        ledger = CloneLedger(self.backend, self.output / "ledger", "a" * 64, "b" * 64,
                             create=True, copy_owner=owner)
        with self.assertRaises(ExceptionGroup), patch.object(ledger, "_release", side_effect=OSError("fixture release")):
            with ledger:
                pass
        self.assertTrue((self.output / "ledger/clone.lock/owner.json").exists())
        self.assertEqual(CloneLedger(self.backend, self.output / "ledger", "a" * 64, "b" * 64).inspect()["status"],
                         "needs_reconciliation")

    def test_owner_record_failure_keeps_unidentified_lock_for_manual_audit(self):
        owner = locks.register_copy_owner(self.backend, self.output)
        ledger = CloneLedger(self.backend, self.output / "ledger", "a" * 64, "b" * 64,
                             create=True, copy_owner=owner)
        with self.assertRaises(ExceptionGroup), patch("devex_clone_ledger.record_copy_lock", side_effect=OSError("fixture record")):
            with ledger:
                self.fail("记录失败不能执行数据步骤")
        self.assertTrue((self.output / "ledger/clone.lock").exists())
        self.assertFalse((self.output / "ledger/clone.lock/owner.json").exists())


if __name__ == "__main__":
    unittest.main()
