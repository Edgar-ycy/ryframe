import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from tests.workspace_directory import WorkspaceDirectory
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_runtime as runtime
from ci_full_stack_resources import BINARIES
from full_stack_process import process_identity, write_receipt
from full_stack_runtime import register_runtime


class RuntimeControlTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "config").mkdir()
        (self.root / "config/app.toml").write_text('[app]\nhost="127.0.0.1"\nport=18210\n', encoding="utf-8")
        self.directory = self.root / "runtime"
        self.directory.mkdir()
        write_receipt(self.directory / "binaries.json", {
            name: str(Path(sys.executable).resolve()) for _, name in BINARIES})
        self.environment = mock.patch.dict(os.environ, {
            "APP_ENV": "test", "APP_SCOPE_ID": "runtime-control-test", "APP_JOBS_MODE": "external",
            "APP_JOBS_HEALTH_HOST": "127.0.0.1", "APP_JOBS_HEALTH_PORT": "19210",
        }, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        register_runtime(self.root, self.directory)
        self.children = []
        self.popen = subprocess.Popen
        self.addCleanup(self.cleanup_children)
        for field, value in (("require_closed_port", None), ("verify_listener", None), ("_ready", True)):
            patch = mock.patch.object(runtime, field, return_value=value)
            setattr(self, field, patch.start())
            self.addCleanup(patch.stop)
        patch = mock.patch.object(runtime.subprocess, "Popen", side_effect=self.spawn)
        self.launch = patch.start()
        self.addCleanup(patch.stop)

    def cleanup_children(self):
        for child in self.children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)

    def spawn(self, arguments, **kwargs):
        self.assertEqual(arguments, [str(Path(sys.executable).resolve())])
        self.assertIn(kwargs["env"]["SNOWFLAKE_WORKER_ID"], ("1", "2"))
        history = self.history()
        self.assertEqual(history["events"][-1]["event"], "start-intent")
        child = self.popen([sys.executable, "-c", "import time; time.sleep(30)"], **kwargs)
        self.children.append(child)
        return child

    def control(self, operation, roles=("api", "worker"), timeout=2):
        return runtime.control(self.root, self.directory, operation, roles, "http://127.0.0.1:18210", timeout)

    def history(self):
        return json.loads((self.directory / runtime.HISTORY).read_text(encoding="utf-8"))

    def test_restart_uses_latest_receipts_and_keeps_producer_history(self):
        first = self.control("start")
        self.assertEqual(set(first["processes"]), {"api", "worker"})
        self.assertEqual(self.control("status")["processes"]["api"]["identity"],
                         first["processes"]["api"]["identity"])
        with self.assertRaisesRegex(ValueError, "重复启动"):
            self.control("start")
        self.control("stop")
        for child in self.children:
            child.wait(timeout=5)
        second = self.control("start")
        self.assertNotEqual(first["processes"]["worker"]["identity"], second["processes"]["worker"]["identity"])
        self.control("stop")
        self.assertTrue(all(value["state"] == "stopped" for value in self.control("status")["processes"].values()))
        self.assertEqual(sum(event["event"] == "start-intent" for event in self.history()["events"]), 4)
        self.assertEqual(len(list(self.directory.glob("api-*.log"))), 2)
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 2)

    def test_unrelated_tool_change_does_not_prevent_exact_stop(self):
        self.control("start", ("worker",))
        (self.root / "tools.py").write_text("# unrelated tool revision\n", encoding="utf-8")
        result = self.control("stop", ("worker",))
        self.assertEqual(result["processes"]["worker"]["state"], "stopped")
        self.children[0].wait(timeout=5)

    def test_timeout_preserves_log_and_start_intent_and_reaps_process(self):
        self._ready.return_value = False
        with self.assertRaisesRegex(TimeoutError, "就绪超时"):
            self.control("start", ("worker",), timeout=0.01)
        self.assertIsNotNone(self.children[0].poll())
        self.assertEqual(self.history()["events"][-1]["event"], "start-failed")
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 1)
        self.assertFalse((self.directory / runtime.LOCK).exists())

    def test_unexpected_exit_is_observed_and_restart_is_available(self):
        self.control("start", ("worker",))
        self.children[0].kill()
        self.children[0].wait(timeout=5)
        self.assertEqual(self.control("status", ("worker",))["processes"]["worker"]["state"], "stopped")
        self.control("start", ("worker",))
        self.control("stop", ("worker",))
        self.children[1].wait(timeout=5)

    def test_second_role_failure_rolls_back_only_newly_started_roles(self):
        self.verify_listener.side_effect = [None, ValueError("端口身份不匹配")]
        with self.assertRaisesRegex(ValueError, "端口身份不匹配"):
            self.control("start")
        for child in self.children:
            self.assertIsNotNone(child.wait(timeout=5))
        self.assertEqual(self.history()["events"][-1]["event"], "startup-rollback")

    def test_pid_reuse_fails_before_stopping_either_role(self):
        self.control("start")
        path = self.directory / "worker.json"
        receipt = json.loads(path.read_text(encoding="utf-8"))
        receipt["identity"]["started"] = "0"
        write_receipt(path, receipt)
        with self.assertRaisesRegex(ValueError, "身份已变化"):
            self.control("stop")
        self.assertTrue(all(child.poll() is None for child in self.children))

    def test_unknown_listener_and_configuration_or_scope_changes_fail_closed(self):
        self.require_closed_port.side_effect = ValueError("明确端口已有监听")
        with self.assertRaisesRegex(ValueError, "已有监听"):
            self.control("status")
        self.require_closed_port.side_effect = None
        with mock.patch.dict(os.environ, {"APP_SCOPE_ID": "wrong-test"}):
            with self.assertRaisesRegex(ValueError, "不匹配"):
                self.control("stop")
        (self.root / "config/app.toml").write_text('[app]\nport=1\n', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "不匹配"):
            self.control("start")
        self.launch.assert_not_called()

    def test_invalid_roles_and_wrong_api_address_do_not_launch(self):
        for roles in ((), ("unknown",), ("api", "api")):
            with self.subTest(roles=roles), self.assertRaises(ValueError):
                self.control("start", roles)
        with self.assertRaisesRegex(ValueError, "精确端口"):
            runtime.control(self.root, self.directory, "start", ("api",), "http://127.0.0.1:1234")
        self.launch.assert_not_called()

    def test_controller_lock_blocks_concurrent_control_and_live_reconciliation(self):
        with runtime.controller_lock(self.directory, "start"):
            with self.assertRaisesRegex(ValueError, "并发"):
                self.control("start")
            with self.assertRaisesRegex(ValueError, "并发"):
                runtime.reconcile_lock(self.directory)
        self.assertFalse((self.directory / runtime.LOCK).exists())

    def stale_lock(self, identity):
        path = self.directory / runtime.LOCK
        path.mkdir()
        write_receipt(path / "owner.json", {
            "format_version": 1, "kind": "devex-clone-runtime-lock", "identity": identity,
            "runtime_directory": str(self.directory), "operation": "start", "token": "test",
        })

    def test_dead_owner_lock_reconciliation_does_not_claim_remote_writes(self):
        child = self.popen([sys.executable, "-c", "import time; time.sleep(30)"],
                           creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        self.children.append(child)
        identity = process_identity(child.pid)
        child.kill()
        child.wait(timeout=5)
        self.stale_lock(identity)
        result = runtime.reconcile_lock(self.directory)
        self.assertFalse(result["remote_writes_reconciled"])
        self.assertIsNone(result["observed_identity"])
        self.assertFalse((self.directory / runtime.LOCK).exists())
        self.assertEqual(len(list(self.directory.glob("controller-reconcile-*.json"))), 1)

    def test_reused_pid_can_release_old_lock_without_signalling_new_owner(self):
        current = process_identity(os.getpid())
        self.stale_lock({**current, "started": "0"})
        with mock.patch.object(runtime, "terminate_owned_process") as terminate:
            result = runtime.reconcile_lock(self.directory)
        self.assertEqual(result["observed_identity"], current)
        terminate.assert_not_called()

    def test_unidentified_or_modified_lock_is_retained(self):
        path = self.directory / runtime.LOCK
        path.mkdir()
        with self.assertRaises(FileNotFoundError):
            runtime.reconcile_lock(self.directory)
        path.rmdir()
        with self.assertRaisesRegex(ValueError, "锁已变化"):
            with runtime.controller_lock(self.directory, "start"):
                (path / "unexpected").write_text("evidence", encoding="utf-8")
        self.assertTrue((path / "owner.json").exists())


if __name__ == "__main__":
    unittest.main()
