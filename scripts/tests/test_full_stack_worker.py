import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import unittest
import uuid
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_worker as worker
from ci_full_stack_resources import BINARIES
from full_stack_process import (
    process_identity,
    read_process,
    record_process,
    terminate_owned_process,
)
from full_stack_runtime import register_runtime, verify_runtime


FAKE_WORKER = r"""
import os
if os.environ.get("SNOWFLAKE_WORKER_ID") == "2":
    import subprocess
    import socket
    import sys
    import time
    mode = os.environ.get("RYFRAME_FAKE_WORKER_MODE", "ready")
    if mode in {"tree", "tree-exit"}:
        environment = os.environ.copy()
        environment.pop("PYTHONPATH", None)
        descendant = subprocess.Popen(
            [
                sys.executable,
                "-S",
                "-c",
                "import os,signal,time; "
                "signal.signal(signal.SIGTERM, lambda *_: None); "
                "open(os.environ['RYFRAME_FAKE_DESCENDANT_PID'],'w').write(str(os.getpid())); "
                "time.sleep(60)",
            ],
            env=environment,
        )
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not os.path.isfile(
            environment["RYFRAME_FAKE_DESCENDANT_PID"]
        ):
            time.sleep(0.02)
        if mode == "tree-exit":
            os._exit(7)
    if mode == "idle":
        while True:
            time.sleep(1)
    if mode == "exit":
        os._exit(7)
    time.sleep(float(os.environ.get("RYFRAME_FAKE_WORKER_DELAY", "0")))
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", int(os.environ["APP_JOBS_HEALTH_PORT"])))
    server.listen()
    while True:
        connection, _ = server.accept()
        with connection:
            request = b""
            while b"\r\n\r\n" not in request:
                block = connection.recv(4096)
                if not block:
                    break
                request += block
            connection.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
"""


class WorkerControlTests(unittest.TestCase):
    def setUp(self):
        local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.root = local / f"worker-control-{uuid.uuid4().hex}"
        self.root.mkdir()
        self.addCleanup(self.cleanup)
        (self.root / "config").mkdir()
        (self.root / "config/app.toml").write_text(
            "[app]\nport=8080\n", encoding="utf-8"
        )
        self.directory = self.root / "runtime"
        self.directory.mkdir()
        self.fake = self.root / "fake-python"
        self.fake.mkdir()
        (self.fake / "sitecustomize.py").write_text(FAKE_WORKER, encoding="utf-8")
        port = self.free_port()
        (self.directory / "binaries.json").write_text(
            json.dumps(
                {name: str(Path(sys.executable).resolve()) for _, name in BINARIES}
            ),
            encoding="utf-8",
        )
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("APP_") and not key.startswith("RYFRAME_E2E_")
        }
        environment.update(
            {
                "APP_ENV": "test",
                "APP_SCOPE_ID": "worker-control-test",
                "APP_JOBS_MODE": "external",
                "APP_JOBS_HEALTH_HOST": "127.0.0.1",
                "APP_JOBS_HEALTH_PORT": str(port),
                "PYTHONPATH": str(self.fake),
            }
        )
        self.environment = mock.patch.dict(os.environ, environment, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        register_runtime(self.root, self.directory)
        self.controllers = []
        self.fake_workers = []

    @staticmethod
    def free_port():
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            return listener.getsockname()[1]

    def cleanup(self):
        for process in self.controllers:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=5)
        for process, identity in self.fake_workers:
            if process_identity(identity["pid"]) == identity:
                terminate_owned_process(identity, crash=True)
            process.wait(timeout=5)
        for path in (self.directory / worker.LOCK,):
            for name in ("supervisor.json", "candidate.json"):
                receipt = path / name
                if receipt.is_file():
                    try:
                        identity = json.loads(receipt.read_text(encoding="utf-8"))[
                            "identity"
                        ]
                        if process_identity(identity["pid"]) == identity:
                            terminate_owned_process(identity, crash=True)
                    except (KeyError, OSError, ValueError):
                        pass
        process = self.directory / "worker.json"
        if process.is_file():
            try:
                identity = read_process(self.directory, "worker", "worker-control-test")
                if process_identity(identity["pid"]) == identity:
                    terminate_owned_process(identity, crash=True)
            except (OSError, ValueError):
                pass
        shutil.rmtree(self.root)

    def control(self, operation, timeout=4):
        return worker.control(operation, self.root, self.directory, timeout)

    def wait_for(self, condition, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if condition():
                return
            time.sleep(0.02)
        self.fail("等待进程级测试检查点超时")

    @staticmethod
    def gone(identity):
        try:
            return process_identity(identity["pid"]) is None
        except PermissionError:
            return False

    def spawn_fake_worker(self, mode: str, *, register: bool) -> dict:
        environment = {
            **os.environ,
            "RYFRAME_FAKE_WORKER_MODE": mode,
            "SNOWFLAKE_WORKER_ID": "2",
        }
        process = subprocess.Popen(
            [sys.executable],
            cwd=self.root,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.wait_for(lambda: process_identity(process.pid) is not None)
        identity = process_identity(process.pid)
        self.assertIsNotNone(identity)
        self.fake_workers.append((process, identity))
        if register:
            recorded = record_process(
                self.directory,
                "worker",
                process.pid,
                sys.executable,
                "worker-control-test",
            )["identity"]
            self.assertEqual(recorded, identity)
        return identity

    def test_start_persists_atomic_receipts_and_worker_survives_cli(self):
        started = self.control("start")
        self.assertEqual(started["state"], "running")
        identity = started["identity"]
        self.assertEqual(process_identity(identity["pid"]), identity)
        self.assertFalse((self.directory / worker.LOCK).exists())

        archive = self.directory / worker.ARCHIVE / started["operation_id"]
        self.assertEqual(
            {path.name for path in archive.iterdir()},
            {
                "owner.json",
                "request.json",
                "candidate.json",
                "supervisor.json",
                "progress.json",
                "result.json",
            },
        )
        self.assertEqual(list(self.directory.glob(".wc-*.tmp")), [])
        receipts = {
            name: json.loads((archive / name).read_text(encoding="utf-8"))
            for name in ("owner.json", "request.json", "result.json")
        }
        for value in receipts.values():
            self.assertEqual(value["operation_id"], started["operation_id"])
            self.assertEqual(value["runtime_directory"], str(self.directory))
            self.assertEqual(value["source_sha256"], started["source_sha256"])
        self.assertEqual(
            receipts["owner.json"]["identity"], started["controller_identity"]
        )
        self.assertGreater((archive / "owner.json").stat().st_size, 0)
        self.assertGreater((archive / "request.json").stat().st_size, 0)
        self.assertGreater((archive / "result.json").stat().st_size, 0)

        with self.assertRaisesRegex(ValueError, "重复启动"):
            self.control("start")
        self.assertEqual(process_identity(identity["pid"]), identity)
        self.assertEqual(self.control("status")["state"], "running")
        self.assertEqual(self.control("reconcile")["state"], "running")
        self.assertEqual(self.control("stop")["state"], "stopped")
        self.assertEqual(self.control("stop")["state"], "stopped")
        self.assertEqual(self.control("status")["state"], "stopped")
        self.assertIsNone(process_identity(identity["pid"]))

    def launch_interrupted_controller(self, checkpoint, operation="start"):
        marker = self.root / f"{checkpoint}.marker"
        source = """
import sys
import time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import full_stack_worker as worker
target, operation, marker = sys.argv[2], sys.argv[3], Path(sys.argv[4])
def pause(phase):
    if phase == target:
        marker.write_text(phase, encoding="utf-8")
        time.sleep(30)
worker._control_checkpoint = pause
worker.control(operation, Path(sys.argv[5]), Path(sys.argv[6]), 4)
"""
        environment = os.environ.copy()
        environment["PYTHONPATH"] = os.pathsep.join(
            [str(Path(__file__).resolve().parents[1]), str(self.fake)]
        )
        process = subprocess.Popen(
            [
                sys.executable,
                "-c",
                source,
                str(Path(__file__).resolve().parents[1]),
                checkpoint,
                operation,
                str(marker),
                str(self.root),
                str(self.directory),
            ],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.controllers.append(process)
        self.wait_for(marker.is_file)
        identity = process_identity(process.pid)
        self.assertIsNotNone(identity)
        terminate_owned_process(identity, crash=True)
        process.wait(timeout=5)
        return process

    def test_parent_death_is_recoverable_during_prepare_launch_and_readiness(self):
        for checkpoint in ("prepared", "authorized", "waiting-ready"):
            with self.subTest(checkpoint=checkpoint):
                with mock.patch.dict(
                    os.environ, {"RYFRAME_FAKE_WORKER_DELAY": "0.5"}, clear=False
                ):
                    self.launch_interrupted_controller(checkpoint)
                    lock = self.directory / worker.LOCK
                    self.assertTrue((lock / "owner.json").is_file())
                    self.assertTrue((lock / "request.json").is_file())
                    self.assertGreater((lock / "owner.json").stat().st_size, 0)
                    self.assertGreater((lock / "request.json").stat().st_size, 0)
                    if checkpoint == "prepared":
                        result = self.control("reconcile")
                        self.assertEqual(result["state"], "stopped")
                    else:
                        self.wait_for(
                            lambda: (lock / "result.json").is_file(), timeout=10
                        )
                        result = self.control("status")
                        self.assertEqual(result["state"], "running")
                        self.control("stop")
                self.assertFalse(lock.exists())

    def test_parent_death_reconciles_simple_operations_to_their_exact_goal(self):
        started = self.control("start")
        self.launch_interrupted_controller("stop-prepared", "stop")
        stopped = self.control("status")
        self.assertEqual(stopped["state"], "stopped")
        self.assertIsNone(process_identity(started["identity"]["pid"]))
        archived = [
            json.loads(path.read_text(encoding="utf-8"))
            for path in (self.directory / worker.ARCHIVE).glob("*/result.json")
        ]
        self.assertTrue(
            any(
                value["operation"] == "stop"
                and value["outcome"] == "succeeded"
                and value["state"] == "stopped"
                and value["reconciled"] is True
                for value in archived
            )
        )

        started = self.control("start")
        for operation in ("status", "reconcile"):
            self.launch_interrupted_controller(f"{operation}-prepared", operation)
            current = self.control("reconcile" if operation == "status" else "status")
            self.assertEqual(current["state"], "running")
            self.assertEqual(current["identity"], started["identity"])
        self.control("stop")

    def test_start_timeout_is_archived_and_reaps_exact_worker(self):
        with mock.patch.dict(
            os.environ, {"RYFRAME_FAKE_WORKER_MODE": "idle"}, clear=False
        ):
            with self.assertRaisesRegex(RuntimeError, "就绪超时"):
                self.control("start", timeout=0.2)
        identity = read_process(self.directory, "worker", "worker-control-test")
        self.assertIsNone(process_identity(identity["pid"]))
        self.assertEqual(self.control("status")["state"], "stopped")
        results = [
            json.loads(path.read_text(encoding="utf-8"))
            for path in (self.directory / worker.ARCHIVE).glob("*/result.json")
        ]
        self.assertTrue(
            any(
                value["operation"] == "start" and value["outcome"] == "failed"
                for value in results
            )
        )

    def test_worker_exit_before_readiness_is_reaped_and_restartable(self):
        with mock.patch.dict(
            os.environ, {"RYFRAME_FAKE_WORKER_MODE": "exit"}, clear=False
        ):
            with self.assertRaisesRegex(RuntimeError, "Worker.*退出码 7"):
                self.control("start")
        failed = read_process(self.directory, "worker", "worker-control-test")
        self.assertIsNone(process_identity(failed["pid"]))
        self.assertEqual(self.control("status")["state"], "stopped")
        restarted = self.control("start")
        self.assertEqual(restarted["state"], "running")
        self.control("stop")

    def test_stop_reaps_worker_descendant_and_parent_exit_cannot_orphan_it(self):
        descendant_pid = self.root / "worker-descendant.pid"
        with mock.patch.dict(
            os.environ,
            {
                "RYFRAME_FAKE_WORKER_MODE": "tree",
                "RYFRAME_FAKE_DESCENDANT_PID": str(descendant_pid),
            },
            clear=False,
        ):
            started = self.control("start")
            self.wait_for(descendant_pid.is_file)
            descendant = process_identity(int(descendant_pid.read_text(encoding="utf-8")))
            self.assertIsNotNone(descendant)
            tree = json.loads(
                (self.directory / "worker-tree.json").read_text(encoding="utf-8")
            )
            self.assertEqual(tree["process"], started["identity"])
            self.assertEqual(
                process_identity(tree["supervisor"]["pid"]), tree["supervisor"]
            )
            self.assertEqual(self.control("stop")["state"], "stopped")
            self.assertTrue(self.gone(descendant))
            normal_control = json.loads(
                (
                    self.directory
                    / f"worker-tree-{tree['operation_id']}-control.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(normal_control["mode"], "normal")

        descendant_pid.unlink()
        with mock.patch.dict(
            os.environ,
            {
                "RYFRAME_FAKE_WORKER_MODE": "tree-exit",
                "RYFRAME_FAKE_DESCENDANT_PID": str(descendant_pid),
            },
            clear=False,
        ):
            with self.assertRaisesRegex(RuntimeError, "Worker.*退出"):
                self.control("start")
            self.wait_for(descendant_pid.is_file)
            exited_descendant = process_identity(
                int(descendant_pid.read_text(encoding="utf-8"))
            )
            self.assertIsNone(exited_descendant)

    def test_crash_control_records_force_mode_and_reaps_worker_descendant(self):
        descendant_pid = self.root / "crash-descendant.pid"
        with mock.patch.dict(
            os.environ,
            {
                "RYFRAME_FAKE_WORKER_MODE": "tree",
                "RYFRAME_FAKE_DESCENDANT_PID": str(descendant_pid),
            },
            clear=False,
        ):
            self.control("start")
            self.wait_for(descendant_pid.is_file)
            descendant = process_identity(int(descendant_pid.read_text(encoding="utf-8")))
            self.assertIsNotNone(descendant)
            tree = json.loads(
                (self.directory / "worker-tree.json").read_text(encoding="utf-8")
            )
            self.assertEqual(self.control("crash")["state"], "stopped")
            self.assertTrue(self.gone(descendant))
            crash_control = json.loads(
                (
                    self.directory
                    / f"worker-tree-{tree['operation_id']}-control.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(crash_control["mode"], "crash")

    def test_wrong_worker_creation_identity_fails_closed_without_signalling_reused_pid(
        self,
    ):
        started = self.control("start")
        path = self.directory / "worker.json"
        original = json.loads(path.read_text(encoding="utf-8"))
        changed = {**original, "identity": {**original["identity"], "started": "0"}}
        path.write_text(json.dumps(changed), encoding="utf-8")
        for operation in ("status", "stop", "reconcile"):
            with (
                self.subTest(operation=operation),
                self.assertRaisesRegex(ValueError, "身份已变化"),
            ):
                self.control(operation)
            self.assertEqual(
                process_identity(started["identity"]["pid"]), started["identity"]
            )
        path.write_text(json.dumps(original), encoding="utf-8")
        self.control("stop")

    def test_duplicate_or_unknown_control_receipt_content_fails_closed(self):
        self.launch_interrupted_controller("prepared")
        lock = self.directory / worker.LOCK
        request = lock / "request.json"
        original = request.read_text(encoding="utf-8")
        request.write_text(
            original.replace(
                '"operation": "start",',
                '"operation": "start",\n  "operation": "stop",',
                1,
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(ValueError, "重复字段"):
            self.control("reconcile")
        self.assertTrue(lock.is_dir())
        self.assertFalse((self.directory / "worker.json").exists())

    def test_empty_legacy_lock_requires_explicit_safe_reconciliation(self):
        lock = self.directory / worker.LOCK
        lock.mkdir()
        with self.assertRaisesRegex(ValueError, "显式使用 reconcile"):
            self.control("status")
        first = self.control("reconcile")
        self.assertEqual(first["state"], "stopped")
        self.assertFalse(lock.exists())
        self.assertEqual(self.control("reconcile")["state"], "stopped")
        self.assertEqual(
            len(list((self.directory / worker.ARCHIVE).glob("legacy-*.json"))), 1
        )

        started = self.control("start")
        lock.mkdir()
        adopted = self.control("reconcile")
        self.assertEqual(adopted["state"], "running")
        self.assertEqual(adopted["identity"], started["identity"])
        self.control("stop")

    def test_configuration_and_binary_changes_fail_before_process_control(self):
        with mock.patch.dict(os.environ, {"APP_SCOPE_ID": "other-test"}):
            with self.assertRaisesRegex(ValueError, "不匹配"):
                self.control("status")
        with mock.patch.dict(os.environ, {"APP_ENV": "production"}):
            with self.assertRaisesRegex(ValueError, "APP_ENV=test"):
                self.control("status")
        manifest = self.directory / "binaries.json"
        original = manifest.read_text(encoding="utf-8")
        binaries = json.loads(original)
        binaries["ryframe-worker"] = str(self.root / "config/app.toml")
        manifest.write_text(json.dumps(binaries), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "不匹配"):
            self.control("status")
        manifest.write_text(original, encoding="utf-8")
        (self.root / "config/app.toml").write_text(
            "[app]\nport=9999\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(ValueError, "不匹配"):
            verify_runtime(self.root, self.directory)

    def test_stopped_state_fails_closed_when_the_registered_port_is_occupied(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", int(os.environ["APP_JOBS_HEALTH_PORT"])))
            listener.listen()
            with self.assertRaisesRegex(ValueError, "端口已占用"):
                self.control("status")

    def test_simple_status_and_reconcile_reject_a_live_unready_worker(self):
        identity = self.spawn_fake_worker("idle", register=True)
        for operation in ("status", "reconcile"):
            with (
                self.subTest(operation=operation),
                self.assertRaisesRegex(ValueError, "未通过就绪探针"),
            ):
                self.control(operation)
            self.assertEqual(process_identity(identity["pid"]), identity)
            self.assertFalse((self.directory / worker.LOCK).exists())

    def test_active_status_reconciliation_rejects_a_foreign_ready_listener(self):
        identity = self.spawn_fake_worker("idle", register=True)
        foreign = self.spawn_fake_worker("ready", register=False)
        self.wait_for(
            lambda: worker.ready(
                verify_runtime(self.root, self.directory)["worker_ready_url"]
            )
        )
        self.launch_interrupted_controller("status-prepared", "status")

        with self.assertRaisesRegex(ValueError, "端口不属于"):
            self.control("reconcile")
        self.assertEqual(process_identity(identity["pid"]), identity)
        self.assertEqual(process_identity(foreign["pid"]), foreign)
        self.assertTrue((self.directory / worker.LOCK).is_dir())

    def test_kernel_guard_wraps_runtime_verification_and_receipt_transition(self):
        events = []

        class Guard:
            def __enter__(self):
                events.append("guard-enter")

            def __exit__(self, *_):
                events.append("guard-exit")

        with mock.patch.object(worker, "process_guard", return_value=Guard()):
            self.assertEqual(self.control("status")["state"], "stopped")
        self.assertEqual(events, ["guard-enter", "guard-exit"])
        self.assertFalse((self.directory / worker.LOCK).exists())

    def test_unknown_operation_and_boolean_timeout_are_rejected_without_lock(self):
        with mock.patch.object(worker, "verify_runtime") as verify:
            for operation in ("unknown", []):
                with (
                    self.subTest(operation=operation),
                    self.assertRaisesRegex(ValueError, "未知 Worker"),
                ):
                    self.control(operation)
            for timeout in (True, "1"):
                with (
                    self.subTest(timeout=timeout),
                    self.assertRaisesRegex(ValueError, "就绪超时"),
                ):
                    self.control("status", timeout=timeout)
        verify.assert_not_called()
        self.assertFalse((self.directory / worker.LOCK).exists())


if __name__ == "__main__":
    unittest.main()
