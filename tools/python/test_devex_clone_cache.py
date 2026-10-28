"""缓存请求、Marker RESP 传输和原生会话异常边界的测试。"""
from contextlib import ExitStack
import copy
import json
import os
from io import BytesIO
from pathlib import Path
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import MagicMock, patch

import devex_clone_cache as cache
import devex_clone_cache_native as markers
import devex_clone_cache_request as model
from devex_clone_capture import write_json
from devex_clone_run_state import binding, initialize_state
from rate_limit_client import load_app_table
from restore_reference_plan import plan_hash
from devex_clone_target_resources import Resources


class CacheFixture(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="cr-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.directory, self.data = self.root / "run", self.root / "data"
        self.directory.mkdir()
        self.data.mkdir()
        wsl = self.file("wsl.exe", b"fake-wsl")
        config = self.file("data/redis.conf", b'requirepass "fixture-secret"\ndaemonize no\n')
        self.previous = {"port": 16390, "wsl": {key: wsl[key] for key in ("path", "sha256")},
            "distribution": "Ubuntu-24.04", "pid": 610000, "started": "100", "executable": "/usr/bin/redis-check-rdb",
            "sha256": "b" * 64, "configuration": config, "run_id": "a" * 40}
        self.private = {"APP_ENV": "test", "TEMP": str(self.root), "TMP": str(self.root)}
        self.config = self.root / "config"
        self.config.mkdir()
        self.application = self.config / "app.test.toml"
        self.application.write_text('[redis]\nhost="127.0.0.1"\nport=16390\ndatabase=0\ntls=false\n'
                                    'password="fixture-secret"\n', encoding="utf-8")
        environment = self.json("environment.json", {"environment": self.private})
        self.marker = {"namespace": "ryframe:{scope-seed}:", "ownership_key": "ryframe:{scope-seed}:.ryframe-owner",
            "ownership_value": "ryframe-owner:v1:scope-seed:redis", "sentinel_key": "ryframe:devex-fresh:scope-seed:sentinel",
            "sentinel_value": "devex-fresh:fixture"}
        self.initial = {"redis": markers.images(self.marker)[1], "generation": {"storage": {"redis": self.previous}}}
        self.original = {"id": "fixture", "storage": {"redis": self.previous}, "target": {"scope_id": "scope-seed"},
                         "reset": {key: self.marker[key] for key in ("sentinel_key", "sentinel_value")}}
        self.value = {"initialized": self.json("initialized.json", self.initial), "target_environment": environment}
        write_json(self.directory / "manifest.json", self.value)
        initialize_state(self.directory)
        identity = {key: self.previous[key] for key in ("pid", "started", "executable")}
        boot = "1" * 8 + "-" + "1" * 4 + "-" + "1" * 4 + "-" + "1" * 4 + "-" + "1" * 12
        receipt = self.json("process.json", {"format_version": 1, "role": "redis", "lifecycle": "running",
            "request_storage": self.previous, "scope_id": "cache-service", "linux_identity": {**identity, "boot_id": boot}})
        self.review = {"services": {"redis": {"directory": str(self.data), "scope_id": "cache-service"}},
                       "tools": {"redis_server": {"path": "/usr/bin/redis-server"}}}
        self.review_binding = self.json("review.json", self.review)
        self.original["review"] = {**self.review_binding, "canonical_sha256": plan_hash(self.review)}
        self.selected = {"redis": {**{key: self.marker[key] for key in ("namespace", "ownership_key", "ownership_value")},
                                   "url": "redis://127.0.0.1:16390/0"}}
        process_request = {key: self.previous[key] for key in ("wsl", "distribution", "executable", "sha256", "configuration", "port")}
        process_request.update(scope_id="cache-service", launcher="/usr/bin/redis-server", previous_identity=identity,
            previous_boot_id=boot, previous_run_id=self.previous["run_id"], password="fixture-secret", timeout_seconds=5,
            python={"path": "/usr/bin/python3", "executable": "/usr/bin/python3.12", "sha256": "c" * 64},
            directory={"path": str(self.data), "device": self.data.stat().st_dev, "inode": self.data.stat().st_ino})
        self.request = {"format_version": 1, "kind": "devex-clone-cache-restart", "side": "target",
            "manifest": binding(self.directory / "manifest.json"), **self.value, "previous": self.previous,
            "process_receipt": receipt, "process": process_request, "markers": self.marker}

    def file(self, name, contents):
        path = self.root / name
        path.write_bytes(contents)
        return binding(path)

    def json(self, name, value):
        path = self.root / name
        write_json(path, value)
        return binding(path)

    def patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(model, "load_app_table", side_effect=lambda backend, environment:
                            load_app_table(backend, environment, config_dir=self.config)))
        stack.enter_context(patch.object(model, "initialization_history", return_value=(self.initial, self.original)))
        stack.enter_context(patch.object(model, "request_binding", return_value=(self.review, self.selected)))
        return stack


class TransportTests(unittest.TestCase):
    def test_marker_transport_encodes_utf8_and_parses_successful_resp(self):
        backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        cases = [
            (["SCAN", "0", "MATCH", "ryframe:{scope}:*", "COUNT", "1000"],
             "*2\r\n$1\r\n0\r\n*1\r\n$12\r\nowner:测试\r\n".encode(), ["0", ["owner:测试"]]),
            (["GET", "owner:测试"], "$9\r\n所有者\r\n".encode(), "所有者"),
            (["GET", "missing"], b"$-1\r\n", None),
            (["MSETNX", "owner:测试", "值", "sentinel", "ready"], b":1\r\n", 1),
        ]
        call = cache.transport(backend, {"previous": {"port": 16390}},
                               {"APP_ENV": "test"}, lambda: None, password="密钥")
        for command, response, expected in cases:
            connection = MagicMock()
            connection.__enter__.return_value = connection
            connection.makefile.return_value.__enter__.return_value = BytesIO(b"+OK\r\n+OK\r\n" + response)
            with self.subTest(command=command[0], result=expected), \
                    patch("devex_clone_target_resources.socket.create_connection", return_value=connection) as connect:
                self.assertEqual(call(command), expected)
            connect.assert_called_once_with(("127.0.0.1", 16390), timeout=5)
            wire = [item.args[0] for item in connection.sendall.call_args_list]
            self.assertEqual([Resources._response(BytesIO(item)) for item in wire],
                             [["AUTH", "密钥"], ["SELECT", "0"], command])
            self.assertEqual(wire[0], "*2\r\n$4\r\nAUTH\r\n$6\r\n密钥\r\n".encode())
            if command[0] == "MSETNX":
                self.assertEqual(wire[2], ("*5\r\n$6\r\nMSETNX\r\n$12\r\nowner:测试\r\n"
                                          "$3\r\n值\r\n$8\r\nsentinel\r\n$5\r\nready\r\n").encode())

    def test_exchange_preserves_callback_failure_and_rejects_later_port_without_replay(self):
        backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        payload = {"format_version": 1, "kind": "ryframe-cache-process", "operation": "stage",
                   "arguments": {"action": "execution-preflight", "directory": str(backend / ".local-tests/exchange"),
                                 "caller": os.getpid(), "parameters": {"value": {}, "mode": "restart",
                                                                        "number": 1, "request_file": None}}}
        first = {"port": "producers-stopped", "arguments": None, "sequence": 1}
        final = {"ok": False, "error": "CacheProcessRejected", "message": "端口失败；禁止重放", "exit_code": None}
        later = {"port": "producers-stopped", "arguments": None, "sequence": 2}
        # 保留真实 exchange、工具绑定核验和消息线程，仅替换进程的标准输入输出。
        for kind in (TimeoutError, KeyboardInterrupt):
            for extra_port in (False, True):
                primary = kind("原始端口失败")
                child = MagicMock()
                child.stdin = MagicMock(spec=BytesIO)
                child.stdin.closed = False
                child.stdin.close.side_effect = lambda: setattr(child.stdin, "closed", True)
                child.stdout.readline.side_effect = [
                    json.dumps(first).encode() + b"\n",
                    json.dumps(later if extra_port else final).encode() + b"\n", b"",
                ]
                child.wait.return_value = 1
                child.poll.return_value = 1
                callback = MagicMock(side_effect=primary)
                with self.subTest(error=kind.__name__, extra_port=extra_port), \
                        patch.object(markers.subprocess, "Popen", return_value=child) as spawn:
                    if extra_port:
                        with self.assertRaisesRegex(RuntimeError, "失败后仍请求执行"):
                            markers.exchange(payload, callback)
                    else:
                        with self.assertRaises(kind) as failure:
                            markers.exchange(payload, callback)
                        self.assertIs(failure.exception, primary)
                callback.assert_called_once_with("producers-stopped", None)
                spawn.assert_called_once()
                self.assertEqual(spawn.call_args.args[0][1:], ["__cache-process"])
                sent = [json.loads(item.args[0]) for item in child.stdin.write.call_args_list]
                self.assertEqual(sent, [payload, {"sequence": 1, "ok": False, "result": None}])
                child.kill.assert_not_called()
                child.stdin.close.assert_called_once()
                child.stdout.close.assert_called_once()

    def test_checkpoint_failure_blocks_connection_or_next_protocol_send(self):
        for rejected in range(1, 5):
            connection = MagicMock()
            connection.__enter__.return_value = connection
            connection.makefile.return_value.__enter__.return_value = BytesIO(b"+OK\r\n+OK\r\n:1\r\n")
            checkpoint = MagicMock(side_effect=[None] * (rejected - 1) + [ValueError("guard changed")])
            backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
            call = cache.transport(backend, {"previous": {"port": 16390}},
                                     {"APP_ENV": "test"}, checkpoint, password="fixture-secret")
            with self.subTest(rejected=rejected), \
                    patch("devex_clone_target_resources.socket.create_connection", return_value=connection) as connect, \
                    self.assertRaisesRegex(ValueError, "guard changed"):
                call(["MSETNX", "owner", "owned", "sentinel", "expected"])
            self.assertEqual(connect.call_count, 0 if rejected == 1 else 1)
            sent = [Resources._response(BytesIO(item.args[0])) for item in connection.sendall.call_args_list]
            self.assertEqual(sent, [["AUTH", "fixture-secret"], ["SELECT", "0"]][:max(0, rejected - 2)])


class RequestTests(CacheFixture):
    def test_request_accepts_original_markers_and_distinct_service_scope(self):
        with self.patches(), patch.object(model, "process_binding", wraps=model.process_binding) as process_bound:
            self.assertEqual(model.validate_request(self.backend, self.directory, self.value, self.request), self.private)
        self.assertEqual(process_bound.call_args.kwargs["review_binding"], self.review_binding)

    def test_request_rejects_changed_directory_credential_or_tool(self):
        with self.patches():
            for field, value in (("directory", {**self.request["process"]["directory"], "inode": 1}),
                                 ("executable", "/usr/bin/other"), ("port", 16391), ("scope_id", "other-cache")):
                request = copy.deepcopy(self.request)
                request["process"][field] = value
                with self.subTest(field=field), self.assertRaises(ValueError):
                    model.validate_request(self.backend, self.directory, self.value, request)
            self.application.write_text(self.application.read_text(encoding="utf-8").replace(
                'password="fixture-secret"', 'password="different"'), encoding="utf-8")
            with self.assertRaises(ValueError):
                model.validate_request(self.backend, self.directory, self.value, self.request)

    def test_request_reads_bound_file_password_without_ambient_overrides(self):
        with self.patches(), patch.dict(os.environ, {"APP_REDIS_PASSWORD": "ambient-ignored", "APP_ENV": "prod"}):
            self.assertEqual(model.validate_request(self.backend, self.directory, self.value, self.request), self.private)

    def test_process_password_must_match_application_and_redis_configuration(self):
        with self.patches():
            wrong = copy.deepcopy(self.request)
            wrong["process"]["password"] = "another-secret"
            with self.assertRaises(ValueError) as error:
                model.validate_request(self.backend, self.directory, self.value, wrong)
            self.assertNotIn("another-secret", str(error.exception))
            self.application.write_text(self.application.read_text(encoding="utf-8").replace(
                'password="fixture-secret"', 'password="another-secret"'), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "绑定的 password"):
                model.validate_request(self.backend, self.directory, self.value, wrong)


if __name__ == "__main__":
    unittest.main()
