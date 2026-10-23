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
from contextlib import contextmanager

sys.path.insert(0, str(Path(__file__).resolve().parent))
import full_stack_worker as worker
from ci_full_stack_resources import BINARIES
from full_stack_process import (
    process_identity,
    read_process,
    record_process,
    terminate_owned_process,
)
from full_stack_runtime import register_runtime, verify_runtime
from rate_limit_tool import tool_binding


FAKE_WORKER = r"""
import os
import sys
import subprocess
original_popen = subprocess.Popen
def product_popen(arguments, *args, **kwargs):
    # 合成产品使用 Python；让产品参数位于 -c 之后，避免解释器拒绝未知选项。
    if len(arguments) > 1 and arguments[0] == sys.executable and arguments[1] == "--config-dir":
        arguments = [arguments[0], "-c", "pass", *arguments[1:]]
    if arguments and arguments[0] == sys.executable and ("--config-dir" in arguments or "__supervise" in arguments):
        from pathlib import Path
        kwargs["env"] = {**kwargs.get("env", {}), "PYTHONPATH": str(Path(__file__).parent)}
    return original_popen(arguments, *args, **kwargs)
subprocess.Popen = product_popen
if "--config-dir" in sys.argv:
    import json
    import tomllib
    from pathlib import Path
    directory = Path(sys.argv[sys.argv.index("--config-dir") + 1])
    settings = tomllib.loads((directory / "app.test.toml").read_text(encoding="utf-8"))
    assert settings["snowflake_worker_id"] == 2
    assert os.environ.get("APP_ENV") == "test"
    assert not any(key.startswith(("APP_", "RYFRAME_", "VITE_")) and key != "APP_ENV" for key in os.environ)
    assert "SNOWFLAKE_WORKER_ID" not in os.environ
    controls = json.loads((directory.parent / "fake-controls.json").read_text(encoding="utf-8"))
    (directory.parent / f"fake-started-{os.getpid()}.json").write_text(json.dumps(controls), encoding="utf-8")
    import subprocess
    import socket
    import sys
    import time
    mode = controls.get("RYFRAME_FAKE_WORKER_MODE", "ready")
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
                "import sys; open(sys.argv[1],'w').write(str(os.getpid())); "
                "time.sleep(60)",
                controls["RYFRAME_FAKE_DESCENDANT_PID"],
            ],
            env=environment,
        )
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not os.path.isfile(
            controls["RYFRAME_FAKE_DESCENDANT_PID"]
        ):
            time.sleep(0.02)
        if mode == "tree-exit":
            os._exit(7)
    if mode == "idle":
        while True:
            time.sleep(1)
    if mode == "exit":
        os._exit(7)
    time.sleep(float(controls.get("RYFRAME_FAKE_WORKER_DELAY", "0")))
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", settings["jobs"]["health_port"]))
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
        binding = tool_binding()
        prelude = ""
        if binding is not None:
            # 合成进程显式从测试文件读取已绑定工具，不借环境变量传协议。
            descriptor = self.fake / "tool-binding.json"
            descriptor.write_text(json.dumps(binding), encoding="utf-8")
            prelude = (f"import sys, json\nsys.path.insert(0, {str(Path(__file__).resolve().parent)!r})\n"
                       "from rate_limit_tool import install_binding\n"
                       f"if '--tool-binding-json' not in sys.argv: install_binding(open({str(descriptor)!r}, encoding='utf-8').read())\n")
        (self.fake / "sitecustomize.py").write_text(prelude + FAKE_WORKER, encoding="utf-8")
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
                "PYTHONPATH": str(self.fake),
            }
        )
        self.environment = mock.patch.dict(os.environ, environment, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        product_environment = worker.process_environment
        fake_environment = mock.patch.object(worker, "process_environment", side_effect=lambda: {**product_environment(), "PYTHONPATH": str(self.fake)})
        fake_environment.start()
        self.addCleanup(fake_environment.stop)
        self.port = port
        for name, worker_id in (("config", 3), ("worker-config", 2)):
            config_dir = self.directory / name
            config_dir.mkdir()
            (config_dir / "app.toml").write_text("", encoding="utf-8")
            (config_dir / "app.test.toml").write_text(
                f'scope_id="worker-control-test"\nsnowflake_worker_id={worker_id}\n[app]\nport=8080\n[jobs]\nmode="external"\nhealth_host="127.0.0.1"\nhealth_port={port}\n', encoding="utf-8")
        (self.directory / "fake-controls.json").write_text("{}", encoding="utf-8")
        register_runtime(self.root, self.directory)
        self.controllers = []
        self.fake_workers = []

    @staticmethod
    def free_port():
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            return listener.getsockname()[1]

    def cleanup(self):
        result = getattr(self._outcome, "result", None)
        failures = [] if result is None else result.errors + result.failures
        if any(getattr(test, "test_case", test) is self for test, _ in failures):
            evidence = self.root.parent.parent / "python-failures" / self.root.name
            evidence.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(self.root, evidence)
            print(f"Worker 失败收据与日志已保留：{evidence}", file=sys.stderr)
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

    @contextmanager
    def fake_controls(self, _environment=None, values=None, clear=False):
        path = self.directory / "fake-controls.json"
        original = path.read_text(encoding="utf-8")
        path.write_text(json.dumps(values or {}), encoding="utf-8")
        try:
            yield
        finally:
            path.write_text(original, encoding="utf-8")

    def spawn_fake_worker(self, mode: str, *, register: bool) -> dict:
        environment = {**worker.process_environment(), "PYTHONPATH": str(self.fake)}
        (self.directory / "fake-controls.json").write_text(json.dumps({"RYFRAME_FAKE_WORKER_MODE": mode}), encoding="utf-8")
        process = subprocess.Popen(
            [sys.executable, "-c", "pass", "--config-dir", str(self.directory / "worker-config")],
            cwd=self.root,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.wait_for(lambda: process_identity(process.pid) is not None)
        self.wait_for(lambda: (self.directory / f"fake-started-{process.pid}.json").is_file())
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
            [str(Path(__file__).resolve().parent), str(self.fake)]
        )
        process = subprocess.Popen(
            [
                sys.executable,
                "-c",
                source,
                str(Path(__file__).resolve().parent),
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
                with self.fake_controls(
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

    def test_parent_death_preserves_failed_start_and_waits_for_cleanup(self):
        with self.fake_controls(os.environ, {"RYFRAME_FAKE_WORKER_MODE": "exit"}):
            self.launch_interrupted_controller("authorized")
            lock = self.directory / worker.LOCK
            self.wait_for(lambda: (lock / "result.json").is_file())
            failure = json.loads((lock / "result.json").read_text(encoding="utf-8"))
            self.assertEqual(failure["outcome"], "failed")
            self.assertIn("退出", failure["error"])
            self.assertEqual(self.control("status")["state"], "stopped")
            archive = self.directory / worker.ARCHIVE / failure["operation_id"]
            self.assertEqual(
                json.loads((archive / "result.json").read_text(encoding="utf-8")),
                failure,
            )
            self.assertTrue((self.directory / failure["log"]).is_file())
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
        with self.fake_controls(
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
        with self.fake_controls(
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
        with self.fake_controls(
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
        with self.fake_controls(
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
        with self.fake_controls(
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

    def test_status_reconciles_worker_tree_closed_with_an_outer_job(self):
        started = self.control("start")
        tree = json.loads(
            (self.directory / "worker-tree.json").read_text(encoding="utf-8")
        )
        self.assertEqual(tree["process"], started["identity"])
        # 模拟浏览器阶段收口：成员监督器随外层 Job 被终止，整树级联退出且没有关闭证明。
        terminate_owned_process(tree["monitor"], crash=True)
        self.wait_for(lambda: self.gone(tree["monitor"]), timeout=15)
        self.wait_for(lambda: self.gone(tree["supervisor"]), timeout=15)
        self.wait_for(lambda: self.gone(tree["process"]), timeout=15)
        proof = self.directory / f"worker-members-{tree['operation_id']}-stopped.json"
        self.assertFalse(proof.exists())

        result = self.control("status")
        self.assertEqual(result["state"], "stopped")
        self.assertTrue(result["reconciled"])
        record = json.loads(
            (
                self.directory
                / f"worker-external-termination-{tree['operation_id']}.json"
            ).read_text(encoding="utf-8")
        )
        self.assertEqual(record["status"], "external-termination")
        self.assertEqual(
            record["identities"],
            {"supervisor": "missing", "monitor": "missing", "process": "missing"},
        )
        self.assertIsNone(record["normal_shutdown_proof"])
        self.assertEqual(record["checked"]["monitor"], tree["monitor"])
        again = self.control("status")
        self.assertEqual(again["state"], "stopped")
        self.assertTrue(again["reconciled"])

        # 外部终止登记后允许在套件树之外启动新代次（浏览器证据阶段的恢复路径）。
        restarted = self.control("start")
        self.assertEqual(restarted["state"], "running")
        self.assertNotEqual(restarted["identity"], started["identity"])
        self.assertEqual(self.control("stop")["state"], "stopped")

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
        with mock.patch.dict(os.environ, {"APP_SCOPE_ID": "other-test", "SNOWFLAKE_WORKER_ID": "2"}):
            self.assertEqual(self.control("status")["state"], "stopped")
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
        (self.directory / "config/app.toml").write_text(
            "[app]\nport=9999\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(ValueError, "不匹配"):
            verify_runtime(self.root, self.directory)

    def test_stopped_state_fails_closed_when_the_registered_port_is_occupied(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", self.port))
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
