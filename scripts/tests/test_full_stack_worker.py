import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
import uuid
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_worker as worker
from ci_full_stack_resources import BINARIES
from full_stack_runtime import register_runtime, verify_runtime


class WorkerControlTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.root = local / f"worker-{uuid.uuid4().hex}"
        self.root.mkdir()
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "config").mkdir()
        (self.root / "config/app.toml").write_text("[app]\nport=8080\n", encoding="utf-8")
        self.directory = self.root / "runtime"
        self.directory.mkdir()
        (self.directory / "binaries.json").write_text(json.dumps(
            {name: str(Path(sys.executable).resolve()) for _, name in BINARIES}), encoding="utf-8")
        self.environment = mock.patch.dict(os.environ, {
            "APP_ENV": "test", "APP_SCOPE_ID": "worker-control-test", "APP_JOBS_MODE": "external",
            "APP_JOBS_HEALTH_HOST": "127.0.0.1", "APP_JOBS_HEALTH_PORT": "12345",
        }, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        register_runtime(self.root, self.directory)
        self.children = []
        self.popen = subprocess.Popen
        self.addCleanup(self.cleanup_children)
        listener = mock.patch.object(worker, "verify_listener")
        self.listener = listener.start()
        self.addCleanup(listener.stop)

    def cleanup_children(self):
        for child in self.children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)

    def spawn(self, arguments, **kwargs):
        self.assertEqual(arguments, [str(Path(sys.executable).resolve())])
        self.assertEqual(kwargs["env"]["SNOWFLAKE_WORKER_ID"], "2")
        process = self.popen([sys.executable, "-c", "import time; time.sleep(30)"], **kwargs)
        self.children.append(process)
        return process

    def control(self, operation, timeout=2):
        return worker.control(operation, self.root, self.directory, timeout)

    def test_restart_updates_receipt_and_cleanup_targets_the_new_process(self):
        with mock.patch.object(worker.subprocess, "Popen", side_effect=self.spawn), \
                mock.patch.object(worker, "ensure_port_free"), mock.patch.object(worker, "ready", return_value=True):
            first = self.control("start")
            self.assertEqual(self.control("status")["identity"], first["identity"])
            with self.assertRaisesRegex(ValueError, "重复启动"):
                self.control("start")
            self.assertEqual(self.control("crash")["state"], "stopped")
            self.children[0].wait(timeout=5)
            second = self.control("start")
            self.assertNotEqual(first["identity"], second["identity"])
            self.assertEqual(self.control("stop")["state"], "stopped")
            self.children[1].wait(timeout=5)
            self.assertEqual(self.control("stop")["state"], "stopped")
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 2)
        self.assertEqual(self.listener.call_count, 2)

    def test_start_timeout_reaps_child_and_keeps_log(self):
        with mock.patch.object(worker.subprocess, "Popen", side_effect=self.spawn), \
                mock.patch.object(worker, "ensure_port_free"), mock.patch.object(worker, "ready", return_value=False):
            with self.assertRaisesRegex(TimeoutError, "就绪超时"):
                self.control("start", timeout=0.01)
        self.assertIsNotNone(self.children[0].poll())
        self.assertEqual(self.control("status")["state"], "stopped")
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 1)

    def test_exit_before_readiness_preserves_receipt_and_allows_restart(self):
        def exit_before_ready(_url):
            self.children[-1].kill()
            self.children[-1].wait(timeout=5)
            return False

        with mock.patch.object(worker.subprocess, "Popen", side_effect=self.spawn), \
                mock.patch.object(worker, "ensure_port_free"):
            with mock.patch.object(worker, "ready", side_effect=exit_before_ready):
                with self.assertRaisesRegex(RuntimeError, "就绪前退出"):
                    self.control("start")
            failed = json.loads((self.directory / "worker.json").read_text(encoding="utf-8"))
            self.assertEqual(self.control("status")["state"], "stopped")
            self.assertFalse((self.directory / "worker-control.lock").exists())
            with mock.patch.object(worker, "ready", return_value=True):
                restarted = self.control("start")
            self.assertNotEqual(failed["identity"], restarted["identity"])
            self.control("stop")
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 2)

    def test_wrong_listener_reaps_started_child_and_keeps_failure_log(self):
        self.listener.side_effect = ValueError("监听进程身份不匹配")
        with mock.patch.object(worker.subprocess, "Popen", side_effect=self.spawn), \
                mock.patch.object(worker, "ensure_port_free"), mock.patch.object(worker, "ready", return_value=True):
            with self.assertRaisesRegex(ValueError, "监听进程身份不匹配"):
                self.control("start")
        self.assertIsNotNone(self.children[0].poll())
        self.assertEqual(self.control("status")["state"], "stopped")
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 1)

    def test_identity_change_is_not_terminated(self):
        with mock.patch.object(worker.subprocess, "Popen", side_effect=self.spawn), \
                mock.patch.object(worker, "ensure_port_free"), mock.patch.object(worker, "ready", return_value=True):
            self.control("start")
        path = self.directory / "worker.json"
        receipt = json.loads(path.read_text(encoding="utf-8"))
        receipt["identity"]["started"] = "0"
        path.write_text(json.dumps(receipt), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "身份已变化"):
            self.control("crash")
        self.assertIsNone(self.children[0].poll())

    def test_lock_and_configuration_changes_fail_closed(self):
        lock = self.directory / "worker-control.lock"
        lock.mkdir()
        with self.assertRaisesRegex(ValueError, "并发启动"):
            self.control("start")
        lock.rmdir()
        with mock.patch.dict(os.environ, {"APP_SCOPE_ID": "other-test"}):
            with self.assertRaisesRegex(ValueError, "不匹配"):
                self.control("status")
        (self.root / "config/app.toml").write_text("[app]\nport=9999\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "不匹配"):
            verify_runtime(self.root, self.directory)

    def test_binary_change_and_non_test_environment_are_rejected(self):
        manifest = self.directory / "binaries.json"
        value = json.loads(manifest.read_text(encoding="utf-8"))
        value["ryframe-worker"] = str(self.root / "config/app.toml")
        manifest.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "不匹配"):
            self.control("status")
        with mock.patch.dict(os.environ, {"APP_ENV": "production"}):
            with self.assertRaisesRegex(ValueError, "APP_ENV=test"):
                self.control("status")

    def test_unknown_operation_is_rejected_before_lock_or_runtime_access(self):
        with mock.patch.object(worker, "verify_runtime") as verify:
            with self.assertRaisesRegex(ValueError, "未知 Worker"):
                self.control("unknown")
        verify.assert_not_called()
        self.assertFalse((self.directory / "worker-control.lock").exists())

    def test_kernel_guard_wraps_runtime_verification_and_control_lock(self):
        events = []

        class Guard:
            def __enter__(self):
                events.append("guard-enter")

            def __exit__(self, *_):
                events.append("guard-exit")

        with mock.patch.object(worker, "process_guard", return_value=Guard()), \
                mock.patch.object(worker, "verify_runtime", wraps=worker.verify_runtime):
            self.control("status")
        self.assertEqual(events, ["guard-enter", "guard-exit"])
        self.assertFalse((self.directory / "worker-control.lock").exists())

    def test_stopped_receipt_fails_closed_when_the_port_is_owned_elsewhere(self):
        with mock.patch.object(worker, "ensure_port_free", side_effect=ValueError("occupied")):
            with self.assertRaisesRegex(ValueError, "occupied"):
                self.control("status")


if __name__ == "__main__":
    unittest.main()
