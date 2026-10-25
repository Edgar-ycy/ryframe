"""Redis 控制设施的离线边界与显式 WSL Python 子进程测试；不运行 Redis。"""
import copy
import ast
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import Mock, patch

import devex_clone_cache_process as process
import devex_clone_cache_native as native


BOOT = "5a81dc04-9e9a-416b-8eab-1b059bddf489"


class CacheNativeTransportTests(unittest.TestCase):
    def binding(self):
        from contextlib import nullcontext
        return patch.object(native, "verified_tool", return_value=nullcontext("bound-tool.exe"))

    def test_credentials_only_travel_in_stdin_and_ambient_secrets_are_removed(self):
        response = subprocess.CompletedProcess([], 0, b'{"ok":true,"result":null}', b'')
        with (self.binding(), patch.object(native, "run_native", return_value=response) as run,
              patch.dict(os.environ, {"APP_REDIS_PASSWORD": "ambient-secret", "AWS_SECRET_ACCESS_KEY": "ambient-aws",
                                      "RYFRAME_SECRET": "ambient-private"})):
            self.assertIsNone(native.invoke("password", value="bound-private-value"))
        args, options = run.call_args
        self.assertNotIn("bound-private-value", repr(args))
        self.assertIn(b"bound-private-value", options["input"])
        self.assertFalse(any(name.upper().startswith(("APP_", "AWS_", "RYFRAME_")) for name in options["env"]))
        run.assert_called_once()

    def test_failure_retains_exit_code_and_never_retries(self):
        response = subprocess.CompletedProcess([], 1, json.dumps({"ok": False, "error": "CacheControlFailed",
            "message": "核对原始代次；禁止重放", "exit_code": 17}).encode(), b'private-secret')
        with self.binding(), patch.object(native, "run_native", return_value=response) as run:
            with self.assertRaisesRegex(RuntimeError, "17") as failure:
                native.invoke("call", request={})
        self.assertNotIn("private-secret", str(failure.exception))
        run.assert_called_once()

    def test_stage_timeout_keeps_cleanup_diagnostic_and_never_replays_ports(self):
        message = "LinuxObserverTimeout；launcher退出未确认；Linux/Git后代状态未知；禁止自动重放"
        child = Mock()
        child.stdin = io.BytesIO()
        child.stdout = Mock()
        child.stdout.readline.side_effect = [json.dumps({"ok": False, "error": "CacheObservationTimeout",
            "message": message, "exit_code": None}).encode() + b"\n", b""]
        child.wait.return_value = 1
        child.poll.return_value = 1
        with (self.binding(), patch.object(native.subprocess, "Popen", return_value=child) as spawn,
              self.assertRaisesRegex(TimeoutError, "LinuxObserverTimeout") as failure):
            native.stage_session("status", Path("D:/owned-run"), {"allow_close_attempt": False},
                lambda *_: self.fail("失败响应不能执行端口"))
        self.assertEqual(str(failure.exception), message)
        spawn.assert_called_once()
        child.kill.assert_not_called()

    def test_connection_refusal_stays_distinct_from_unknown_results(self):
        for category, expected in (("CacheConnectionRefused", ConnectionRefusedError),
                                   ("CacheObservationTimeout", TimeoutError), ("CacheProcessRejected", ValueError)):
            response = subprocess.CompletedProcess([], 1, json.dumps({"ok": False, "error": category,
                "message": "仅观察失败", "exit_code": None}).encode(), b'')
            with self.subTest(category=category), self.binding(), patch.object(native, "run_native", return_value=response):
                with self.assertRaises(expected):
                    native.invoke("facts", request={})

    def test_missing_binding_bad_response_and_invalid_utf8_never_fall_back(self):
        from contextlib import nullcontext
        with patch.object(native, "verified_tool", return_value=nullcontext(None)), patch.object(native, "run_native") as run:
            with self.assertRaises(ValueError):
                native.invoke("password", value="secret")
            run.assert_not_called()
        for raw in (b'{}', b'{"ok":true,"ok":false,"result":null}', b'\xff'):
            response = subprocess.CompletedProcess([], 0, raw, b'')
            with self.subTest(raw=raw[:1]), self.binding(), patch.object(native, "run_native", return_value=response) as run:
                with self.assertRaises((ValueError, RuntimeError)):
                    native.invoke("password", value="secret")
                run.assert_called_once()
        with patch.object(native, "run_native") as run, self.assertRaises(ValueError):
            native.invoke("password", value="\ud800")
        run.assert_not_called()

    def test_malformed_exit_status_never_becomes_a_retryable_observation_failure(self):
        categories = ("CacheConnectionRefused", "CacheObservationTimeout", "CacheProcessRejected", "CacheControlFailed")
        for category in categories:
            codes = [True, "private-secret", 0, 2 ** 31, -(2 ** 31) - 1]
            if category != "CacheControlFailed":
                codes.append(17)
            for code in codes:
                response = subprocess.CompletedProcess([], 1, json.dumps({"ok": False, "error": category,
                    "message": "仅观察失败", "exit_code": code}).encode(), b'private-secret')
                with self.subTest(category=category, code=code), self.binding(), \
                        patch.object(native, "run_native", return_value=response) as run:
                    with self.assertRaisesRegex(RuntimeError, "退出状态无效") as failure:
                        native.invoke("facts", request={})
                    self.assertNotIn("private-secret", str(failure.exception))
                    run.assert_called_once()


class FrozenHelperSetTests(unittest.TestCase):
    """WSL 冻结辅助集合必须自包含；仓库工具目录不进入 Linux 侧导入路径。"""

    def helpers(self) -> dict:
        tools = Path(__file__).resolve().parent
        return {"cache": tools / "devex_clone_cache_process.py", "process": tools / "full_stack_process.py"}

    def test_unconditional_imports_stay_inside_the_frozen_set(self):
        allowed = set(sys.stdlib_module_names) | set(self.helpers())
        for key, path in self.helpers().items():
            names = set()
            tree = ast.parse(path.read_text(encoding="utf-8"))
            for node in tree.body:  # 只统计无条件模块级导入；平台守卫由 Linux 子进程用例覆盖
                if isinstance(node, ast.Import):
                    names.update(alias.name.split(".")[0] for alias in node.names)
                elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                    names.add(node.module.split(".")[0])
            with self.subTest(helper=key):
                self.assertEqual(sorted(names - allowed), [])

    @unittest.skipIf(os.name == "nt", "WSL 辅助进程只在 Linux/WSL 侧导入")
    def test_frozen_helpers_import_without_the_repository_tools(self):
        temporary = WorkspaceDirectory(dir=Path(__file__).resolve().parents[2] / ".local-tests/tmp", prefix="fh-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "cache.py").write_bytes(self.helpers()["cache"].read_bytes())
        (root / "full_stack_process.py").write_bytes(self.helpers()["process"].read_bytes())
        result = subprocess.run([sys.executable, "-B", "-c", "import cache, full_stack_process"], cwd=root,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
        self.assertEqual(result.returncode, 0, result.stderr)


@unittest.skipUnless(os.name == "nt", "WSL Redis 进程测试仅在 Windows runner 执行")
class CacheProcessTests(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="cp-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        data = self.root / "data"
        data.mkdir()
        config = data / "redis.conf"
        config.write_text('save ""\nappendonly no\n', encoding="utf-8")
        wsl = self.root / "wsl.exe"
        wsl.write_bytes(b"isolated stub")
        self.request = {"scope_id": "cache-test", "wsl": {"path": str(wsl), "sha256": process.bound(wsl)["sha256"]},
                        "distribution": "Ubuntu-24.04", "launcher": "/usr/bin/redis-server", "executable": "/usr/bin/redis-check-rdb",
                        "sha256": "a" * 64, "configuration": process.bound(config),
                        "directory": {"path": str(data), "device": data.stat().st_dev, "inode": data.stat().st_ino},
                        "previous_identity": {"pid": 200, "started": "50", "executable": "/usr/bin/redis-check-rdb"},
                        "previous_boot_id": BOOT, "previous_run_id": "a" * 40, "port": 16390,
                        "password": "bound-private-value", "timeout_seconds": 1,
                        "python": {"path": "/usr/bin/python3", "executable": "/usr/bin/python3.12", "sha256": "b" * 64}}
        self.output = self.root / "process"
        self.output.mkdir()
        self.identity = {"pid": 300, "started": "60", "executable": self.request["executable"], "boot_id": BOOT}

    def intent(self):
        process.prepare(self.request, self.output)
        process.write(self.output / "intent.json", {"request_sha256": process.canonical(self.request),
                      "arguments": process.command(self.request, self.output, "serve"), "launch": process.bound(self.output / "launch-request.json")})

    def recorded(self):
        self.intent()
        launcher = {"pid": 400, "started": "70", "executable": self.request["wsl"]["path"]}
        process.write(self.output / "launcher.json", {"identity": launcher, "intent": process.bound(self.output / "intent.json")})
        process.write(self.output / "linux-intent.json", {"request_sha256": process.canonical(self.request),
                      "arguments": [self.request["launcher"], process.linux_path(self.request["configuration"]["path"])], "supervisor": {}})
        process.write(self.output / "linux-process.json", {"request_sha256": process.canonical(self.request), "identity": self.identity,
                      "intent": {**process.bound(self.output / "linux-intent.json"), "path": process.linux_path(str(self.output / "linux-intent.json"))}})
        return process.inspect_start(self.request, self.output)

    def facts(self):
        return {"process_id": "300", "run_id": "b" * 40, "config_file": process.linux_path(self.request["configuration"]["path"]),
                "configuration_sha256": process.canonical(process.configuration(self.request))}

    def test_original_files_directory_and_exact_fields(self):
        process.validate(self.request)
        for update in ({"port": True}, {"previous_boot_id": "unknown"}, {"extra": True}):
            with self.subTest(update=tuple(update)), self.assertRaises(ValueError):
                process.validate({**self.request, **update})
        bad = copy.deepcopy(self.request)
        bad["directory"]["inode"] += 1
        with self.assertRaises(ValueError):
            process.validate(bad)
        Path(self.request["configuration"]["path"]).write_bytes(b"changed")
        with self.assertRaises(ValueError):
            process.validate(self.request)

    def test_first_generation_allows_only_a_complete_absence_of_predecessor(self):
        initial = {**self.request, "previous_identity": None, "previous_boot_id": None, "previous_run_id": None}
        process.validate(initial)
        for key, value in (("previous_boot_id", BOOT), ("previous_run_id", "a" * 40)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                process.validate({**initial, key: value})

    def test_password_requires_direct_utf8_text_and_rejects_legacy_fields(self):
        for password in (None, False, 123, "", " \t", "\0hidden-secret", "\ud800"):
            with self.subTest(kind=type(password).__name__), self.assertRaises(ValueError) as error:
                process.validate({**self.request, "password": password})
            self.assertNotIn("hidden-secret", str(error.exception))
        for legacy in ({**self.request, "password_env": "APP_REDIS_PASSWORD"},
                       {key: value for key, value in self.request.items() if key != "password"}):
            with self.assertRaises(ValueError):
                process.validate(legacy)

    def test_invalid_password_fails_before_network_or_process_creation(self):
        invalid = {**self.request, "password": None}
        with patch.object(process.socket, "create_connection") as connect, \
                patch.object(process, "launch_process") as popen:
            with self.assertRaises(ValueError):
                process.redis_facts(invalid, {"APP_REDIS_PASSWORD": "ambient-secret"})
            with self.assertRaises(ValueError):
                process.start(invalid, {"APP_ENV": "test"}, self.output, lambda: None)
        connect.assert_not_called()
        popen.assert_not_called()

    def test_first_generation_does_not_observe_a_predecessor_before_launch(self):
        request = {**self.request, "previous_identity": None, "previous_boot_id": None, "previous_run_id": None}
        payload = {"request": request, "request_sha256": process.canonical(request), "windows_output": str(self.output)}
        authorization = json.dumps({"request_sha256": payload["request_sha256"], "output": str(self.output)}) + "\n"
        kernel = Mock()
        with patch.object(process, "linux_inputs", return_value=(self.output, kernel)), \
                patch.object(process.sys, "stdin", io.StringIO(authorization)), \
                patch.object(process, "linux_identity", side_effect=lambda _kernel, pid: (
                    self.assertNotEqual(pid, self.request["previous_identity"]["pid"]),
                    {"pid": pid, "started": "60", "executable": self.request["executable"], "boot_id": BOOT},
                )[1]) as identity, \
                patch.object(process.socket, "socket"), \
                patch.object(process, "write", side_effect=RuntimeError("已越过前代检查")), \
                patch.object(process, "launch_process") as popen, self.assertRaisesRegex(RuntimeError, "已越过前代检查"):
            process.linux_serve(payload)
        self.assertEqual([call.args[1] for call in identity.call_args_list], [os.getpid()])
        popen.assert_not_called()

    def test_atomic_publication_never_exposes_partial_json_and_never_overwrites(self):
        target = self.output / "receipt.json"
        original = json.dump
        entered, finish = threading.Event(), threading.Event()
        errors = []
        def slow(value, stream, **kwargs):
            stream.write(" ")
            stream.flush()
            entered.set()
            if not finish.wait(5):
                raise TimeoutError()
            original(value, stream, **kwargs)
        def publish():
            try:
                process.write(target, {"identity": self.identity})
            except BaseException as error:
                errors.append(error)
        with patch.object(process.json, "dump", side_effect=slow):
            worker = threading.Thread(target=publish)
            worker.start()
            self.assertTrue(entered.wait(5))
            self.assertFalse(target.exists())
            finish.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertFalse(errors)
        self.assertEqual(process.read(target), {"identity": self.identity})
        before = target.read_bytes()
        with self.assertRaises(FileExistsError):
            process.write(target, {"replacement": True})
        self.assertEqual(target.read_bytes(), before)

    def test_no_intent_is_not_started_but_popen_unknown_is_not_inferred(self):
        self.assertIsNone(process.inspect_start(self.request, self.output))
        self.intent()
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_inspection_is_local_and_frozen_helper_changes_rejected(self):
        runtime = self.recorded()
        with patch.object(process.subprocess, "run", side_effect=AssertionError("not local")):
            self.assertEqual(process.inspect_start(self.request, self.output), runtime)
        (self.output / "helpers/cache.py").write_bytes(b"changed")
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_public_identity_and_observation_receipts_exclude_password(self):
        runtime = self.recorded()
        alive = {"identity": self.identity, "alive": True, "boot_id": BOOT, "terminated": False}
        with patch.object(process, "windows_identity", return_value=True), \
                patch.object(process, "linux_call", return_value=alive), \
                patch.object(process, "redis_facts", return_value=self.facts()):
            observed = process.observe(self.request, {"APP_ENV": "test"}, runtime, self.output)
        self.assertNotIn(self.request["password"], json.dumps(observed))
        # launch-request 是包含直接凭据的私有控制输入；公开收据仅保存其绑定摘要。
        for path in self.output.glob("*.json"):
            if path.name != "launch-request.json":
                with self.subTest(receipt=path.name):
                    self.assertNotIn(self.request["password"], path.read_text(encoding="utf-8"))

    def test_historical_inspection_survives_tool_upgrade_but_regular_calls_reject_it(self):
        runtime = self.recorded()
        Path(self.request["wsl"]["path"]).write_bytes(b"reviewed successor WSL")
        before = {path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        with patch.object(process.subprocess, "run", side_effect=AssertionError("历史检查不能调用工具")):
            self.assertEqual(process.inspect_start(self.request, self.output), runtime)
            for operation in (lambda: process.linux_call(self.request, runtime, "observe"),
                              lambda: process.stop(self.request, {}, runtime, self.output),
                              lambda: process.observe(self.request, {}, runtime)):
                with self.subTest(operation=operation), self.assertRaisesRegex(ValueError, "WSL"):
                    operation()
        self.assertEqual(before, {path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()})

    def test_other_request_or_launcher_identity_is_rejected(self):
        self.recorded()
        with self.assertRaises(ValueError):
            process.inspect_start({**self.request, "scope_id": "another-run"}, self.output)
        path = self.output / "launcher.json"
        value = process.read(path)
        value["identity"]["started"] = False
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_info_identity_config_and_run_are_all_required(self):
        process.check_facts(self.request, self.identity, self.facts())
        for key, value in (("process_id", "999"), ("run_id", self.request["previous_run_id"]),
                           ("config_file", "/another/redis.conf"), ("configuration_sha256", "0" * 64)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                process.check_facts(self.request, self.identity, {**self.facts(), key: value})
        with self.assertRaises(ValueError):
            process.check_facts(self.request, self.identity, self.facts(), run_id="c" * 40)

    def test_boot_change_never_signals_reused_pid(self):
        kernel = Mock()
        with patch.object(process, "boot_id", return_value="another-boot"):
            self.assertFalse(process.linux_stop(kernel, self.identity))
        kernel.terminate_owned_process.assert_not_called()
        with patch.object(process, "boot_id", return_value=BOOT):
            process.linux_stop(kernel, self.identity)
        kernel.terminate_owned_process.assert_called_once_with({key: self.identity[key] for key in ("pid", "started", "executable")})

    def test_readonly_check_action_reaches_frozen_linux_observer_without_stop(self):
        path = self.output / "control.json"
        payload = {"request_sha256": process.canonical(self.request)}
        process.write(path, payload)
        result = {"identity": self.identity, "alive": False, "boot_id": BOOT, "terminated": False}
        with (patch.object(process.sys, "argv", ["cache.py", "check", "--request", str(path),
                                                "--sha256", process.bound(path)["sha256"]]),
              patch.object(process.sys, "stdin", io.StringIO(json.dumps(self.identity))),
              patch.object(process.sys, "stdout", io.StringIO()) as output,
              patch.object(process, "linux_check", return_value=result) as observe):
            process.main()
        observe.assert_called_once_with(payload, self.identity, stop=False)
        self.assertEqual(json.loads(output.getvalue()), result)

    def test_linux_control_refuses_unrecorded_identity_before_signal(self):
        self.recorded()
        payload = process.read(self.output / "launch-request.json")
        kernel = Mock()
        with patch.object(process, "linux_inputs", return_value=(self.output, kernel)), self.assertRaises(ValueError):
            process.linux_check(payload, {**self.identity, "pid": 999}, stop=True)
        kernel.terminate_owned_process.assert_not_called()

    def test_stop_falls_back_to_read_only_check_when_precise_stop_call_fails(self):
        runtime = self.recorded()
        stopped = {"identity": self.identity, "alive": False, "boot_id": BOOT, "terminated": False}
        actions = []

        def linux_call(_request, _runtime, action):
            actions.append(action)
            if action == "stop":
                raise RuntimeError("WSL Redis 精确身份操作失败")
            return stopped

        with patch.object(process, "windows_identity", return_value=False), \
                patch.object(process, "linux_call", side_effect=linux_call), \
                patch("full_stack_process.process_identity", return_value=None), \
                patch("devex_clone_source_proof.require_closed_port"):
            result = process.stop(self.request, {}, runtime, self.output)
        self.assertEqual(actions, ["stop", "check"])
        self.assertEqual(result["status"], "redis_process_stopped")
        self.assertTrue(result["observed_after_failed_stop"])
        self.assertFalse(result["alive"])
        saved = json.loads((self.output / "stopped.json").read_text(encoding="utf-8"))
        self.assertEqual(saved, result)

    def test_stop_keeps_original_error_when_read_only_check_still_alive(self):
        runtime = self.recorded()
        alive = {"identity": self.identity, "alive": True, "boot_id": BOOT, "terminated": False}

        def linux_call(_request, _runtime, action):
            if action == "stop":
                raise RuntimeError("WSL Redis 精确身份操作失败")
            return alive

        with patch.object(process, "windows_identity", return_value=False), \
                patch.object(process, "linux_call", side_effect=linux_call), \
                self.assertRaisesRegex(RuntimeError, "精确身份操作失败"):
            process.stop(self.request, {}, runtime, self.output)
        self.assertFalse((self.output / "stopped.json").exists())

    def test_launch_constructor_exception_remains_unknown(self):
        with patch("devex_clone_source_proof.require_closed_port"), patch.object(process, "launch_process", side_effect=OSError("injected")):
            with self.assertRaises(OSError):
                process.start(self.request, {"APP_ENV": "test"}, self.output, lambda: None)
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)
        self.assertFalse((self.output / "partial.json").exists())

    def test_child_environment_contains_only_selector_and_host_runtime(self):
        private = {"APP_ENV": "test", "TEMP": str(self.root), "TMP": str(self.root)}
        with patch.dict(os.environ, {"APP_REDIS_PASSWORD": "ambient-secret", "APP_ENV": "prod",
                                     "AWS_SECRET_ACCESS_KEY": "ambient-aws", "RYFRAME_SECRET": "ambient-private"}), \
                patch("devex_clone_source_proof.require_closed_port"), \
                patch.object(process, "launch_process", side_effect=OSError("constructor unknown")) as popen:
            with self.assertRaises(OSError):
                process.start(self.request, private, self.output, lambda: None)
        child_environment = popen.call_args.kwargs["env"]
        self.assertEqual({key: value for key, value in child_environment.items() if key.upper().startswith("APP_")},
                         {"APP_ENV": "test"})
        self.assertEqual({key: child_environment[key] for key in private}, private)
        self.assertNotIn("AWS_SECRET_ACCESS_KEY", child_environment)
        self.assertNotIn("RYFRAME_SECRET", child_environment)
        public = ["intent.json", "failure.json"]
        for name in public:
            text = (self.output / name).read_text(encoding="utf-8")
            self.assertNotIn(self.request["password"], text)
            self.assertNotIn("ambient-secret", text)
        self.assertNotIn(self.request["password"], json.dumps(popen.call_args.args))

    def test_legacy_child_environment_rejected_before_any_launch_intent(self):
        for environment in ({"APP_REDIS_PASSWORD": "ambient-secret"},
                            {"APP_ENV": "test", "AWS_SECRET_ACCESS_KEY": "ambient-secret"},
                            {"APP_ENV": "test", "rediscli_auth": "ambient-secret"},
                            {"APP_ENV": "test", "PATH": "unbound-host-path"},
                            {"APP_ENV": "prod"}, {"APP_ENV": "test", "TEMP": 123},
                            {"APP_ENV": "test", "TEMP": "one", "temp": "two"}):
            with self.subTest(keys=list(environment)), patch.object(process, "launch_process") as popen, self.assertRaises(ValueError):
                process.start(self.request, environment, self.output, lambda: None)
            popen.assert_not_called()
            self.assertEqual(list(self.output.iterdir()), [])

    def test_guard_before_popen_has_no_intent(self):
        with patch.object(process, "launch_process") as popen, self.assertRaises(RuntimeError):
            process.start(self.request, {"APP_ENV": "test"}, self.output,
                          lambda: (_ for _ in ()).throw(RuntimeError("guard")))
        popen.assert_not_called()
        self.assertIsNone(process.inspect_start(self.request, self.output))

    def test_authorization_eof_and_wrong_run_never_launch_redis(self):
        kernel = Mock()
        payload = {"request": self.request, "request_sha256": process.canonical(self.request), "windows_output": str(self.output)}
        for data in ("", '{}\n'):
            with self.subTest(data=bool(data)), patch.object(process, "linux_inputs", return_value=(self.output, kernel)), \
                    patch.object(process.sys, "stdin", io.StringIO(data)), patch.object(process, "launch_process") as popen, self.assertRaises(ValueError):
                process.linux_serve(payload)
            popen.assert_not_called()

    def test_launcher_exits_before_authorization_has_explicit_partial_proof(self):
        child = Mock(pid=777, stdin=io.BytesIO(), returncode=1)
        child.wait.return_value = 1
        with patch("devex_clone_source_proof.require_closed_port"), patch.object(process, "launch_process", return_value=child), \
                patch("full_stack_process.process_identity", return_value=None), self.assertRaises(ValueError):
            process.start(self.request, {"APP_ENV": "test"}, self.output, lambda: None)
        result = process.inspect_start(self.request, self.output)
        self.assertEqual(result["state"], "not_started")
        self.assertFalse(process.read(self.output / "partial.json")["authorization_sent"])

    @unittest.skipUnless(os.environ.get("RYFRAME_WSL_STUB_TEST") == "1", "显式 WSL Python stub 验证，不启动 Redis")
    def test_wsl_supervisor_crash_keeps_child_identity_for_pidfd_recovery(self):
        script = self.root / "orphan.py"
        child = self.root / "child.py"
        child.write_text("import time; time.sleep(30)\n", encoding="utf-8")
        helper = process.linux_path(str(Path(process.__file__).resolve()))
        kernel = process.linux_path(str(self.backend / "tools/python/full_stack_process.py"))
        output = process.linux_path(str(self.output))
        script.write_text("\n".join([
            "import importlib.util,json,os,sys,subprocess,time,socket", "from pathlib import Path", "sys.dont_write_bytecode=True",
            "def load(name,path):", " s=importlib.util.spec_from_file_location(name,path); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); return m",
            f"p=load('cache',{helper!r}); k=load('kernel',{kernel!r}); out=Path({output!r})",
            f"request=json.loads({json.dumps(self.request)!r})",
            f"request.update(launcher=sys.executable,executable=str(Path(sys.executable).resolve()),configuration={{'path':{str(child)!r}}})",
            "request['previous_identity']['pid']=2147483646",
            "with socket.socket() as sock: sock.bind(('127.0.0.1',0)); request['port']=sock.getsockname()[1]",
            f"payload={{'request':request,'request_sha256':p.canonical(request),'windows_output':{str(self.output)!r}}}",
            "if len(sys.argv)>1:", " payload=p.read(out/'payload.json'); p.linux_inputs=lambda *_args,**_kwargs:(out,k); p.linux_serve(payload); sys.exit()",
            "p.write(out/'payload.json',payload)",
            "parent=subprocess.Popen([sys.executable,__file__,'serve'],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)",
            "expected=None",
            "try:", " parent.stdin.write((json.dumps({'request_sha256':payload['request_sha256'],'output':payload['windows_output']})+'\\n').encode()); parent.stdin.close()",
            " deadline=time.monotonic()+8",
            " while not (out/'linux-process.json').exists():",
            "  assert parent.poll() is None, parent.stderr.read().decode()",
            "  assert time.monotonic()<deadline; time.sleep(.01)",
            " expected=p.read(out/'linux-process.json')['identity']",
            " p.write(out/'acknowledged.json',{'request_sha256':payload['request_sha256'],'identity':expected})",
            " while not (out/'accepted.json').exists(): assert time.monotonic()<deadline; time.sleep(.01)",
            " parent.kill(); parent.wait(timeout=5)", " assert p.linux_identity(k,expected['pid'])==expected",
            " p.linux_inputs=lambda *_args,**_kwargs:(out,k)",
            " result=p.linux_check(payload,expected,stop=True)", " assert result['terminated'] and not result['alive']",
            " print(json.dumps({'orphan_registered':True,'pidfd_recovered':True}))",
            "finally:", " if parent.poll() is None: parent.kill(); parent.wait(timeout=5)",
            " if expected is not None: p.linux_stop(k,expected)",
        ]), encoding="utf-8")
        result = subprocess.run(["C:/Windows/System32/wsl.exe", "--distribution", "Ubuntu-24.04", "--exec", "/usr/bin/python3",
                                 process.linux_path(str(script))], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))
        self.assertEqual(json.loads(result.stdout), {"orphan_registered": True, "pidfd_recovered": True})

    @unittest.skipUnless(os.environ.get("RYFRAME_WSL_STUB_TEST") == "1", "显式 WSL Python stub 验证，不启动 Redis")
    def test_wsl_atomic_publish_and_pidfd_exact_cleanup(self):
        script = self.root / "native.py"
        helper = process.linux_path(str(Path(process.__file__).resolve()))
        kernel = process.linux_path(str(self.backend / "tools/python/full_stack_process.py"))
        output = process.linux_path(str(self.output))
        script.write_text("\n".join([
            "import importlib.util,json,os,sys,subprocess,time", "from pathlib import Path",
            "sys.dont_write_bytecode=True", "def load(name,path):",
            " s=importlib.util.spec_from_file_location(name,path); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); return m",
            f"p=load('cache',{helper!r}); k=load('kernel',{kernel!r}); out=Path({output!r})",
            "p.write(out/'wsl-published.json',{'complete':True})",
            "try: p.write(out/'wsl-published.json',{'replaced':True}); raise AssertionError('overwritten')",
            "except FileExistsError: pass", "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'])",
            "try:", " identity=p.linux_identity(k,child.pid)",
            " try: p.linux_stop(k,{**identity,'started':str(int(identity['started'])+1)}); raise AssertionError('mismatch accepted')",
            " except ValueError: pass", " assert child.poll() is None", " assert p.linux_stop(k,identity)", " child.wait(timeout=10)",
            " assert p.linux_identity(k,child.pid) is None", " print(json.dumps({'atomic_publish':True,'pidfd_cleanup':True}))",
            "finally:", " if child.poll() is None: child.kill(); child.wait(timeout=10)",
        ]), encoding="utf-8")
        result = subprocess.run(["C:/Windows/System32/wsl.exe", "--distribution", "Ubuntu-24.04", "--exec", "/usr/bin/python3",
                                 process.linux_path(str(script))], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))
        self.assertEqual(json.loads(result.stdout), {"atomic_publish": True, "pidfd_cleanup": True})
        self.assertEqual(process.read(self.output / "wsl-published.json"), {"complete": True})


if __name__ == "__main__":
    unittest.main()
