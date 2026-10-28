"""缓存原生传输、当前校验与历史证据读取的离线边界测试。"""
import copy
import io
import json
import os
import queue
from pathlib import Path
import subprocess
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

    def test_exchange_finishes_without_eof_and_error_cleanup_never_closes_a_foreign_read(self):
        class ShortQueue(queue.Queue):
            def get(self, block=True, timeout=None):
                return super().get(block, timeout=0.05)
        final = b'{"ok":true,"result":"published"}\n'
        port = b'{"port":"producers-stopped","arguments":null,"sequence":1}\n'
        primary = KeyboardInterrupt("original callback failure")
        cases = (("final", final, None),
                 ("json", b'{private-invalid-json}\n', ValueError),
                 ("sequence", port.replace(b'"sequence":1', b'"sequence":2'), RuntimeError),
                 ("callback", port + final, KeyboardInterrupt),
                 ("timeout", b"", TimeoutError))
        for name, frames, expected in cases:
            with self.subTest(case=name):
                read_fd, writer = os.pipe()
                child = Mock()
                child.stdin = io.BytesIO()
                child.stdout = os.fdopen(read_fd, "rb")
                child.poll.return_value = 0
                child.wait.return_value = 0
                if frames:
                    os.write(writer, frames)
                callback = Mock(side_effect=primary) if name == "callback" else Mock()
                result, failures = [], []
                def exchange():
                    try:
                        result.append(native.exchange({"operation": "stage"}, callback))
                    except BaseException as error:
                        failures.append(error)
                # 真匿名管道的 writer 始终持有；旧实现会卡在下一次 read 与 close 的流锁。
                with self.binding(), patch.object(native.subprocess, "Popen", return_value=child) as spawn, \
                        patch.object(queue, "Queue", ShortQueue):
                    worker = threading.Thread(target=exchange, daemon=True)
                    worker.start()
                    try:
                        worker.join(3)
                        bounded = not worker.is_alive()
                        if name != "timeout" and bounded:
                            self.assertTrue(child.stdout.closed)
                    finally:
                        # 即使被测实现回归，也先解除测试自己的阻塞，再断言，不能挂住门禁。
                        os.close(writer)
                        worker.join(3)
                    self.assertTrue(bounded, "writer 未关闭时 exchange 必须有界返回")
                    self.assertFalse(worker.is_alive())
                    spawn.assert_called_once()
                    child.kill.assert_not_called()
                if expected is None:
                    self.assertEqual(result, ["published"])
                    self.assertFalse(failures)
                else:
                    self.assertEqual(len(failures), 1)
                    self.assertIsInstance(failures[0], expected)
                    if name == "callback":
                        self.assertIs(failures[0], primary)
                    else:
                        callback.assert_not_called()

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
        # 历史收据读取只需要绑定字节；这些数据不作为可执行 helper 使用。
        helpers = self.output / "helpers"
        helpers.mkdir()
        frozen = {}
        for key, name in (("cache", "cache.py"), ("process", "full_stack_process.py")):
            path = helpers / name
            path.write_bytes(b"historical receipt fixture\n")
            frozen[key] = {**process.bound(path), "path": process.linux_path(str(path))}
        process.write(self.output / "launch-request.json", {
            "format_version": 1, "request": self.request, "request_sha256": process.canonical(self.request),
            "windows_output": str(self.output), "linux_output": process.linux_path(str(self.output)), "helpers": frozen})
        arguments = [self.request["wsl"]["path"], "--distribution", self.request["distribution"], "--exec",
                     self.request["python"]["path"], frozen["cache"]["path"], "serve", "--request",
                     process.linux_path(str(self.output / "launch-request.json")), "--sha256",
                     process.bound(self.output / "launch-request.json")["sha256"]]
        process.write(self.output / "intent.json", {"request_sha256": process.canonical(self.request),
                      "arguments": arguments, "launch": process.bound(self.output / "launch-request.json")})

    def recorded(self):
        self.intent()
        launcher = {"pid": 400, "started": "70", "executable": self.request["wsl"]["path"]}
        process.write(self.output / "launcher.json", {"identity": launcher, "intent": process.bound(self.output / "intent.json")})
        process.write(self.output / "linux-intent.json", {"request_sha256": process.canonical(self.request),
                      "arguments": [self.request["launcher"], process.linux_path(self.request["configuration"]["path"])], "supervisor": {}})
        process.write(self.output / "linux-process.json", {"request_sha256": process.canonical(self.request), "identity": self.identity,
                      "intent": {**process.bound(self.output / "linux-intent.json"), "path": process.linux_path(str(self.output / "linux-intent.json"))}})
        return process.inspect_start(self.request, self.output)

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

    def test_historical_inspection_rejects_frozen_helper_byte_changes(self):
        runtime = self.recorded()
        self.assertEqual(process.inspect_start(self.request, self.output), runtime)
        (self.output / "helpers/cache.py").write_bytes(b"changed")
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_historical_inspection_survives_tool_upgrade_but_regular_calls_reject_it(self):
        runtime = self.recorded()
        Path(self.request["wsl"]["path"]).write_bytes(b"reviewed successor WSL")
        before = {path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        self.assertEqual(process.inspect_start(self.request, self.output), runtime)
        for operation in (lambda: process.validate(self.request),
                          lambda: process.stop(self.backend, self.request, {}, runtime, self.output),
                          lambda: process.observe(self.backend, self.request, {}, runtime)):
            with self.subTest(operation=operation), self.assertRaises(ValueError):
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

if __name__ == "__main__":
    unittest.main()
