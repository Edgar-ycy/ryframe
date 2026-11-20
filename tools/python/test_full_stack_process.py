import errno
import os
import subprocess
import sys
import json
import shutil
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, call, patch
from workspace_directory import WorkspaceDirectory
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import full_stack_process as process_module
from full_stack_process import _linux_identity, process_identity, read_process, record_process, terminate_owned_process
from full_stack_process import write_receipt
from artifact_digests import filesystem_path

ROOT = Path(__file__).resolve().parents[2]


class LinuxIdentityFailureTests(unittest.TestCase):
    pid = 123
    descriptor = 61
    stat = "123 (redis) " + " ".join(["0"] * 19 + ["456"])

    def test_deleted_live_image_is_not_reported_as_exited(self):
        for code in (errno.ENOENT, errno.ESRCH):
            with self.subTest(errno=code):
                missing = OSError(code, "fixture missing executable")
                with patch.object(Path, "read_text", return_value=self.stat) as read, \
                        patch.object(Path, "resolve", side_effect=missing), \
                        patch.object(process_module.select, "select", return_value=([], [], [])) as selected:
                    with self.assertRaisesRegex(ValueError, "进程仍存在") as raised:
                        _linux_identity(self.pid, self.descriptor)
                self.assertIs(raised.exception.__cause__, missing)
                read.assert_called_once_with()
                self.assertEqual(selected.call_args_list, [
                    call([self.descriptor], [], [], 0), call([self.descriptor], [], [], 0.02),
                ])

    def test_exit_during_image_lookup_is_reported_as_exited(self):
        for code, field in ((errno.ENOENT, "exe"), (errno.ESRCH, "stat")):
            with self.subTest(errno=code, field=field):
                missing = OSError(code, "fixture exit during observation")
                with patch.object(Path, "read_text", return_value=self.stat,
                                  side_effect=missing if field == "stat" else None), \
                        patch.object(Path, "resolve", side_effect=missing) as resolve, \
                        patch.object(process_module.select, "select",
                                     side_effect=[([], [], []), ([self.descriptor], [], [])]) as selected:
                    self.assertIsNone(_linux_identity(self.pid, self.descriptor))
                self.assertEqual(resolve.call_count, int(field == "exe"))
                self.assertEqual(selected.call_args_list, [
                    call([self.descriptor], [], [], 0), call([self.descriptor], [], [], 0.02),
                ])

    def test_access_denial_is_not_reported_as_exited(self):
        for code, field in ((errno.EPERM, "exe"), (errno.EACCES, "stat"), (errno.EIO, "exe")):
            with self.subTest(errno=code, field=field):
                denied = OSError(code, "fixture observation failed")
                with patch.object(Path, "read_text", return_value=self.stat,
                                  side_effect=denied if field == "stat" else None), \
                        patch.object(Path, "resolve", side_effect=denied), \
                        patch.object(process_module.select, "select", return_value=([], [], [])) as selected:
                    with self.assertRaises(OSError) as raised:
                        _linux_identity(self.pid, self.descriptor)
                self.assertIs(raised.exception, denied)
                selected.assert_called_once_with([self.descriptor], [], [], 0)

    def test_original_pidfd_readability_after_proc_reads_rejects_sampled_identity(self):
        with patch.object(Path, "read_text", return_value=self.stat) as read, \
                patch.object(Path, "resolve", return_value="/fixture/redis") as resolve, \
                patch.object(process_module.select, "select",
                             side_effect=[([], [], []), ([self.descriptor], [], [])]) as selected:
            self.assertIsNone(_linux_identity(self.pid, self.descriptor))
        read.assert_called_once_with()
        resolve.assert_called_once_with(strict=True)
        self.assertEqual(selected.call_args_list, [call([self.descriptor], [], [], 0)] * 2)

    def test_linux_entry_points_use_the_same_pidfd_for_identity_and_signal(self):
        kernel = SimpleNamespace(
            name="posix", pidfd_open=Mock(return_value=self.descriptor), close=Mock(),
        )
        signals = SimpleNamespace(pidfd_send_signal=Mock(), SIGTERM=15, SIGKILL=9)
        expected = {"pid": self.pid, "started": "456", "executable": "/fixture/redis"}
        with patch.object(process_module, "os", kernel), \
                patch.object(process_module, "signal", signals), \
                patch.object(Path, "read_text", return_value=self.stat), \
                patch.object(Path, "resolve", return_value=expected["executable"]), \
                patch.object(process_module.select, "select", side_effect=[
                    ([], [], []), ([], [], []), ([], [], []), ([], [], []), ([self.descriptor], [], []),
                ]) as selected:
            self.assertEqual(process_identity(self.pid), expected)
            self.assertTrue(terminate_owned_process(expected))
        self.assertEqual(kernel.pidfd_open.call_args_list, [call(self.pid)] * 2)
        self.assertEqual(kernel.close.call_args_list, [call(self.descriptor)] * 2)
        signals.pidfd_send_signal.assert_called_once_with(self.descriptor, signals.SIGTERM)
        self.assertEqual(selected.call_args_list,
                         [call([self.descriptor], [], [], 0)] * 4 + [call([self.descriptor], [], [], 5)])


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
    def test_live_child_with_disappearing_proc_executable_is_not_exited(self):
        with patch.object(Path, "resolve", side_effect=ProcessLookupError(errno.ESRCH, "fixture missing executable")):
            with self.assertRaisesRegex(ValueError, "进程仍存在"):
                process_identity(self.process.pid)
        self.assertIsNone(self.process.poll())

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
