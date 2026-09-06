import os
import subprocess
import sys
import unittest
from tests.workspace_directory import WorkspaceDirectory
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from full_stack_process import process_identity, read_process, record_process, terminate_owned_process

ROOT = Path(__file__).resolve().parents[2]


class OwnedProcessTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.directory = WorkspaceDirectory(dir=local)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.addCleanup(self.cleanup_process)

    def cleanup_process(self):
        if self.process.poll() is None:
            self.process.terminate()
        self.process.wait(timeout=5)

    def test_live_process_identity_prevents_pid_reuse_and_cleans_exact_child(self):
        identity = process_identity(self.process.pid)
        self.assertEqual(Path(identity["executable"]), Path(sys.executable).resolve())
        with self.assertRaisesRegex(ValueError, "身份已变化"):
            terminate_owned_process({**identity, "started": "0"})
        self.assertIsNone(self.process.poll(), "不匹配的 PID 必须保持运行")
        self.assertTrue(terminate_owned_process(identity))
        self.process.wait(timeout=5)
        self.assertIsNone(process_identity(self.process.pid))
        self.assertFalse(terminate_owned_process(identity))

    def test_process_receipt_binds_role_scope_and_actual_binary(self):
        receipt = record_process(self.root, "worker", self.process.pid, sys.executable, "owned-test")
        self.assertEqual(read_process(self.root, "worker", "owned-test"), receipt["identity"])
        with self.assertRaisesRegex(ValueError, "scope 不匹配"):
            read_process(self.root, "worker", "other-test")
        with self.assertRaisesRegex(ValueError, "可执行文件不一致"):
            record_process(self.root, "worker", self.process.pid, str(self.root / "unknown.exe"), "owned-test")

    def test_system_pid_values_are_rejected(self):
        for pid in (-1, 0, 1, True, "42"):
            with self.subTest(pid=pid), self.assertRaises(ValueError):
                terminate_owned_process({"pid": pid})


if __name__ == "__main__":
    unittest.main()
