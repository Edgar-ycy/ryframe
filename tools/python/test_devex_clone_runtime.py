import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from workspace_directory import WorkspaceDirectory
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_runtime as runtime
from ci_full_stack_resources import BINARIES
from full_stack_process import write_receipt
from full_stack_runtime import register_runtime


class RuntimeControlTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "config").mkdir()
        (self.root / "config/app.toml").write_text('scope_id="runtime-control-test"\nsnowflake_worker_id=3\n[app]\nhost="127.0.0.1"\nport=18210\n[jobs]\nmode="external"\nhealth_host="127.0.0.1"\nhealth_port=19210\n', encoding="utf-8")
        (self.root / "config/app.test.toml").write_text("", encoding="utf-8")
        self.directory = self.root / "runtime"
        self.directory.mkdir()
        write_receipt(self.directory / "binaries.json", {
            name: str(Path(sys.executable).resolve()) for _, name in BINARIES})
        self.environment = mock.patch.dict(os.environ, {"APP_ENV": "test"}, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        register_runtime(self.root, self.directory, source_config_dir=self.root / "config")
        self.children = []
        self.popen = subprocess.Popen
        self.addCleanup(self.cleanup_children)
        for field, value in (("require_closed_port", None), ("verify_listener", None), ("_ready", True)):
            patch = mock.patch.object(runtime, field, return_value=value)
            setattr(self, field, patch.start())
            self.addCleanup(patch.stop)
        self.launch = mock.Mock(side_effect=self.spawn)
        process_port = SimpleNamespace(
            Popen=self.launch, DEVNULL=subprocess.DEVNULL, STDOUT=subprocess.STDOUT,
            CREATE_NO_WINDOW=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        patch = mock.patch.object(runtime, "subprocess", process_port)
        patch.start()
        self.addCleanup(patch.stop)

    def cleanup_children(self):
        for child in self.children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)

    def spawn(self, arguments, **kwargs):
        self.assertEqual(arguments[0], str(Path(sys.executable).resolve()))
        self.assertEqual(arguments[1], "--config-dir")
        self.assertIn(Path(arguments[2]).name, ("config", "worker-config"))
        self.assertNotIn("SNOWFLAKE_WORKER_ID", kwargs["env"])
        self.assertEqual({key for key in kwargs["env"] if key.startswith("APP_")}, {"APP_ENV"})
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
        self.cleanup_children()
        second = self.control("start")
        self.assertNotEqual(first["processes"]["worker"]["identity"], second["processes"]["worker"]["identity"])
        self.cleanup_children()
        self.assertTrue(all(value["state"] == "stopped" for value in self.control("status")["processes"].values()))
        self.assertEqual(sum(event["event"] == "start-intent" for event in self.history()["events"]), 4)
        self.assertEqual(len(list(self.directory.glob("api-*.log"))), 2)
        self.assertEqual(len(list(self.directory.glob("worker-*.log"))), 2)

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
        restarted = self.control("start", ("worker",))
        self.assertEqual(restarted["processes"]["worker"]["state"], "running")

    def test_unlocked_observation_creates_no_lock_history_or_receipt(self):
        source = self.root / "config/app.toml"
        source.write_text(source.read_text(encoding="utf-8").replace("port=18210\n", "port=18211\n"), encoding="utf-8")
        before = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        result = runtime.observe(
            self.root, self.directory, ("api", "worker"), "http://127.0.0.1:18210"
        )
        after = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        self.assertEqual(before, after)
        self.assertEqual(result["kind"], "devex-clone-runtime-observation")
        self.assertTrue(all(item["state"] == "stopped" for item in result["processes"].values()))
        self.assertFalse((self.directory / runtime.LOCK).exists())
        self.assertFalse((self.directory / runtime.HISTORY).exists())

    def test_second_role_failure_rolls_back_only_newly_started_roles(self):
        self.verify_listener.side_effect = [None, ValueError("端口身份不匹配")]
        with self.assertRaisesRegex(ValueError, "端口身份不匹配"):
            self.control("start")
        for child in self.children:
            self.assertIsNotNone(child.wait(timeout=5))
        self.assertEqual(self.history()["events"][-1]["event"], "startup-rollback")

    def test_unknown_listener_and_configuration_changes_fail_closed(self):
        self.require_closed_port.side_effect = ValueError("明确端口已有监听")
        with self.assertRaisesRegex(ValueError, "已有监听"):
            self.control("status")
        self.require_closed_port.side_effect = None
        receipt = json.loads((self.directory / "runtime.json").read_text(encoding="utf-8"))
        (Path(receipt["api_config_dir"]) / "app.toml").write_text('[app]\nport=1\n', encoding="utf-8")
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

    def test_controller_lock_blocks_concurrent_start(self):
        with runtime.controller_lock(self.directory, "start"):
            with self.assertRaisesRegex(ValueError, "并发"):
                self.control("start")
        self.launch.assert_not_called()
        self.assertFalse((self.directory / runtime.LOCK).exists())

    def test_modified_lock_is_retained_and_blocks_a_new_start(self):
        path = self.directory / runtime.LOCK
        owner = path / "owner.json"
        unknown = path / "unexpected"
        with self.assertRaisesRegex(ValueError, "锁已变化"):
            with runtime.controller_lock(self.directory, "start"):
                original = owner.read_bytes()
                unknown.write_bytes(b"evidence")
        self.assertEqual(owner.read_bytes(), original)
        self.assertEqual(unknown.read_bytes(), b"evidence")
        with self.assertRaisesRegex(ValueError, "异常退出"):
            self.control("start")
        self.launch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
