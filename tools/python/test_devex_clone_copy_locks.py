"""复制控制器异常退出的精确本地锁恢复；不连接任何实际数据库或对象服务。"""
import json
import os
from pathlib import Path
import subprocess
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


CHILD = '''
import json, os, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from devex_clone_copy_locks import register_copy_owner
from devex_clone_ledger import CloneLedger
from devex_clone_target_state import generation_lock
backend, output, target = map(Path, sys.argv[2:])
owner = register_copy_owner(backend, output)
with generation_lock(target, copy_owner=owner):
    with CloneLedger(backend, output / "ledger", "a" * 64, "b" * 64, create=True, copy_owner=owner) as ledger:
        before = {"kind": "database", "resource_sha256": "c" * 64, "schema_sha256": "d" * 64,
                  "tables_sha256": "e" * 64, "preserved_sha256": "f" * 64}
        ledger.intent("copy-pending", before, {**before, "tables_sha256": "0" * 64}, "1" * 64)
        (output / "registered.json").write_text(json.dumps(owner.binding), encoding="utf-8")
        os._exit(42)
'''


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

    def abandon(self):
        child = subprocess.run([sys.executable, "-c", CHILD, str(Path(__file__).resolve().parent),
                                str(self.backend), str(self.output), str(self.target)],
                               stdin=subprocess.DEVNULL, capture_output=True, timeout=10,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        self.assertEqual(child.returncode, 42, child.stderr.decode(errors="replace"))
        self.owner = json.loads((self.output / "registered.json").read_text(encoding="utf-8"))
        self.value = json.loads(Path(self.owner["path"]).read_text(encoding="utf-8"))
        return self.owner

    def recover(self, **updates):
        return locks.recover_copy_locks(self.backend, self.output, self.owner,
                                       **({"source_export": self.source, "initialized": self.initialized} | updates))

    def test_kernel_exit_releases_guards_but_recovery_preserves_ledger_and_requires_reconcile(self):
        self.abandon()
        journal = self.output / "ledger/events.ndjson"
        before = journal.read_bytes()
        result = self.recover()
        self.assertEqual(result["status"], "owned_locks_released")
        self.assertEqual(len(result["locks"]), 2)
        self.assertFalse(result["remote_writes_reconciled"])
        self.assertTrue(result["requires_reconcile"])
        self.assertEqual(journal.read_bytes(), before)
        self.assertFalse((self.target / "initialize.lock").exists())
        self.assertFalse((self.output / "ledger/clone.lock").exists())
        state = CloneLedger(self.backend, self.output / "ledger", "a" * 64, "b" * 64).inspect()
        self.assertEqual(state["status"], "needs_reconciliation")
        self.assertEqual(state["steps"]["copy-pending"]["phase"], "unknown")

    def test_old_timeout_does_not_hide_or_poison_current_session_command_status(self):
        self.abandon()
        self.bind(self.output / "command-session-old-unrelated.json", {"returncode": None})
        self.bind(self.output / f"command-{self.value['session_id']}-complete.json", {"returncode": 0})
        self.assertEqual(self.recover()["status"], "owned_locks_released")

    def test_current_unfinished_command_refuses_recovery_and_preserves_both_locks(self):
        self.abandon()
        command = self.output / f"command-{self.value['session_id']}-unknown.json"
        for raw in ("", "{", '{"returncode": null}'):
            with self.subTest(raw=raw):
                command.write_text(raw, encoding="utf-8")
                with self.assertRaises(ValueError):
                    self.recover()
                self.assertTrue((self.target / "initialize.lock/owner.json").exists())
                self.assertTrue((self.output / "ledger/clone.lock/owner.json").exists())

    def test_fixed_source_and_session_binding_rejects_replacement_before_cleanup(self):
        self.abandon()
        with self.assertRaisesRegex(ValueError, "固定"):
            self.recover(source_export=self.initialized)
        (self.output / "session.json").write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "已变化"):
            self.recover()
        self.assertTrue((self.target / "initialize.lock").exists())

    def test_bare_or_other_owner_lock_is_not_adopted_and_no_partial_cleanup_occurs(self):
        self.abandon()
        path = self.output / "ledger/clone.lock/owner.json"
        before = path.read_bytes()
        path.unlink()
        with self.assertRaisesRegex(ValueError, "裸锁"):
            self.recover()
        self.assertTrue((self.target / "initialize.lock/owner.json").exists())
        value = json.loads(before)
        value["inode"] += 1
        write_receipt(path, value)
        with self.assertRaisesRegex(ValueError, "登记持有人"):
            self.recover()

    def test_live_owner_and_same_guard_concurrency_are_rejected(self):
        owner = locks.register_copy_owner(self.backend, self.output)
        self.owner = owner.binding
        with self.assertRaisesRegex(ValueError, "仍在运行"):
            self.recover()
        with generation_lock(self.target, copy_owner=owner):
            with self.assertRaisesRegex(ValueError, "并发"):
                self.recover()
        with process_guard(self.target, locks.GUARDS["target"]):
            with self.assertRaisesRegex(ValueError, "并发"):
                with generation_lock(self.target, copy_owner=owner):
                    self.fail("guard 必须禁止新控制器抢锁")

    def test_reused_pid_does_not_receive_signals_during_recovery(self):
        self.abandon()
        reused = {**self.value["controller"], "started": "999"}
        with patch.object(locks, "process_identity", return_value=reused):
            result = self.recover()
        self.assertEqual(result["observed_controller"], reused)
        self.assertEqual(result["remote_writes"], 0)

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

    def test_rmdir_failure_restores_known_owner_before_returning_failure(self):
        self.abandon()
        lock = self.target / "initialize.lock"
        original = (lock / "owner.json").read_bytes()
        with patch.object(Path, "rmdir", side_effect=OSError("fixture directory busy")):
            with self.assertRaisesRegex(OSError, "directory busy"):
                self.recover()
        self.assertEqual((lock / "owner.json").read_bytes(), original)
        self.assertTrue((self.output / "ledger/clone.lock/owner.json").exists())

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
