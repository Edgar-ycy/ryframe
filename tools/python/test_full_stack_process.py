import os
import subprocess
import sys
import json
import shutil
import unittest
from unittest.mock import Mock, patch
from workspace_directory import WorkspaceDirectory
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from full_stack_process import _linux_identity, process_identity, read_process, record_process, terminate_owned_process
from full_stack_process import write_receipt
from artifact_digests import filesystem_path

ROOT = Path(__file__).resolve().parents[2]


class LinuxIdentityFailureTests(unittest.TestCase):
    def test_deleted_live_image_is_not_reported_as_exited(self):
        stat = "123 (redis) " + " ".join(["0"] * 20)
        with patch.object(Path, "read_text", return_value=stat), \
                patch.object(Path, "resolve", side_effect=FileNotFoundError):
            with self.assertRaisesRegex(ValueError, "进程仍存在"):
                _linux_identity(123)

    def test_exit_during_image_lookup_is_reported_as_exited(self):
        stat = "123 (redis) " + " ".join(["0"] * 20)
        with patch.object(Path, "read_text", side_effect=[stat, FileNotFoundError()]), \
                patch.object(Path, "resolve", side_effect=FileNotFoundError):
            self.assertIsNone(_linux_identity(123))

    def test_access_denial_is_not_reported_as_exited(self):
        stat = "123 (redis) " + " ".join(["0"] * 20)
        with patch.object(Path, "read_text", return_value=stat), \
                patch.object(Path, "resolve", side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                _linux_identity(123)
        with patch.object(Path, "read_text", side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                _linux_identity(123)


@unittest.skipUnless(os.name == "nt", "Windows 内核 API 场景仅在 Windows 执行")
class WindowsIdentityDenialTests(unittest.TestCase):
    def kernel(self, waits, *, times=1, image=0):
        kernel = Mock()
        kernel.WaitForSingleObject.side_effect = waits
        kernel.GetProcessTimes.return_value = times
        kernel.QueryFullProcessImageNameW.return_value = image
        return kernel

    def test_exit_during_denied_image_lookup_is_reported_as_exited(self):
        import full_stack_process as module

        kernel = self.kernel([258, 258, 0])
        with patch.object(module.ctypes, "get_last_error", return_value=5), \
                patch.object(module.time, "sleep"):
            self.assertIsNone(module._windows_identity(kernel, 1234, 4321))

    def test_gen_failure_during_image_lookup_waits_for_exit(self):
        import full_stack_process as module

        kernel = self.kernel([258, 258, 258, 0])
        with patch.object(module.ctypes, "get_last_error", return_value=31), \
                patch.object(module.time, "sleep"):
            self.assertIsNone(module._windows_identity(kernel, 1234, 4321))

    def test_other_kernel_errors_still_fail_closed_while_alive(self):
        import full_stack_process as module

        kernel = self.kernel([258, 258])
        with patch.object(module.ctypes, "get_last_error", return_value=87):
            with self.assertRaises(OSError):
                module._windows_identity(kernel, 1234, 4321)

    def test_persistent_denial_without_exit_fails_closed(self):
        import full_stack_process as module

        kernel = self.kernel([258, 258, 258, 258])
        with patch.object(module.ctypes, "get_last_error", return_value=5), \
                patch.object(module.time, "monotonic", side_effect=[0, 0, 6]), \
                patch.object(module.time, "sleep"):
            with self.assertRaises(OSError):
                module._windows_identity(kernel, 1234, 4321)


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

    @unittest.skipUnless(os.name == "nt", "需要 Windows 扩展路径语义")
    def test_atomic_receipt_supports_long_paths_without_temporary_files(self):
        directory = self.root / ("x" * 100) / ("y" * 100)
        os.makedirs(filesystem_path(directory))
        self.addCleanup(shutil.rmtree, filesystem_path(self.root / ("x" * 100)))
        path = directory / "frontend-members-ready.json"
        self.assertGreater(len(str(path)), 260)
        write_receipt(path, {"status": "ready"})
        write_receipt(path, {"status": "complete"})
        with open(filesystem_path(path), encoding="utf-8") as stream:
            self.assertEqual(json.load(stream), {"status": "complete"})
        self.assertEqual(os.listdir(filesystem_path(directory)), [path.name])

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

    @unittest.skipIf(os.name == "nt", "Linux /proc 退出竞态")
    def test_process_identity_treats_disappearing_proc_executable_as_exited(self):
        with patch.object(Path, "resolve", side_effect=ProcessLookupError):
            self.assertIsNone(process_identity(self.process.pid))

    def test_process_receipt_binds_role_scope_and_actual_binary(self):
        receipt = record_process(self.root, "frontend", self.process.pid, sys.executable, "owned-test")
        self.assertEqual(read_process(self.root, "frontend", "owned-test"), receipt["identity"])
        with self.assertRaisesRegex(ValueError, "scope 不匹配"):
            read_process(self.root, "frontend", "other-test")
        with self.assertRaisesRegex(ValueError, "可执行文件不一致"):
            record_process(self.root, "frontend", self.process.pid, str(self.root / "unknown.exe"), "owned-test")

    def test_system_pid_values_are_rejected(self):
        for pid in (-1, 0, 1, True, "42"):
            with self.subTest(pid=pid), self.assertRaises(ValueError):
                terminate_owned_process({"pid": pid})


if __name__ == "__main__":
    unittest.main()
