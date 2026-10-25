"""缓存恢复的请求、原子标记及外层发布；全部使用文件和协议替身。"""
from contextlib import ExitStack
import copy
import os
from io import BytesIO
from pathlib import Path
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import MagicMock, patch

import devex_clone_cache as cache
import devex_clone_cache_native as markers
import devex_clone_cache_process as process
import devex_clone_cache_request as model
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding, begin, bind_controller_attempt, finish, initialize_state, run_lock
from rate_limit_client import load_app_table
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
        self.private = {"APP_ENV": "test"}
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
        self.request_file = Path(self.json("request.json", self.request)["path"])
        self.store, self.commands = {}, []
        self.alive, self.counter, self.failure = False, 0, None

    def file(self, name, contents):
        path = self.root / name
        path.write_bytes(contents)
        return binding(path)

    def json(self, name, value):
        path = self.root / name
        write_json(path, value)
        return binding(path)

    def call(self, command):
        self.commands.append(command)
        if command[0] == "SCAN":
            self.assertEqual(command, ["SCAN", "0", "MATCH", self.marker["namespace"] + "*", "COUNT", "1000"])
            return ["0", [key for key in self.store if key.startswith(self.marker["namespace"])]]
        if command[0] == "GET":
            return self.store.get(command[1])
        self.assertEqual(command, ["MSETNX", self.marker["ownership_key"], self.marker["ownership_value"],
                                  self.marker["sentinel_key"], self.marker["sentinel_value"]])
        self.assertTrue(list(self.directory.glob("cache-target/a*/owner/intent.json")))
        if self.failure == "before":
            raise TimeoutError("fixture before write")
        if any(command[index] in self.store for index in (1, 3)):
            return 0
        self.store.update({command[1]: command[2], command[3]: command[4]})
        if self.failure == "after":
            raise TimeoutError("fixture after write")
        return 1

    def start(self, request, environment, output, guard):
        self.assertTrue(output.is_dir())
        guard()
        self.counter += 1
        self.alive = True
        self.store = {}
        receipt = self.json(str((output / "fake-process.json").relative_to(self.root)), {"generation": self.counter})
        base = {"process_receipt": receipt, "output": str(output), "generation": self.counter}
        write_json(output / "base.json", base)
        return self.observe(request, environment, base)

    def inspect(self, request, output):
        path = output / "base.json"
        return read_json(path) if path.exists() else None

    def observe(self, request, environment, base, output=None):
        if not self.alive:
            raise ValueError("fixture stopped")
        return {**base, "redis": {**self.previous, "pid": 620000 + self.counter,
                    "started": str(200 + self.counter), "run_id": str(self.counter) * 40}, "facts": {}}

    def stop(self, request, environment, runtime, output):
        self.alive = False
        return {"state": "stopped", "runtime": runtime}

    def patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(model, "load_app_table", side_effect=lambda backend, environment:
                            load_app_table(backend, environment, config_dir=self.config)))
        stack.enter_context(patch.object(model, "initialization_history", return_value=(self.initial, self.original)))
        stack.enter_context(patch.object(model, "request_binding", return_value=(self.review, self.selected)))
        stack.enter_context(patch.object(cache, "producers_stopped"))
        stack.enter_context(patch.object(cache, "transport", return_value=self.call))
        for name in ("start", "inspect", "observe", "stop"):
            stack.enter_context(patch.object(process, "inspect_start" if name == "inspect" else name, side_effect=getattr(self, name)))
        return stack

    def execute(self, mode, *, outer_failure=False):
        with run_lock(self.directory) as owner:
            number = begin(self.directory, cache.STAGE, mode, {})
            bind_controller_attempt(self.directory, number, owner)
            try:
                result = cache.execute_cache(self.backend, self.directory, self.value, mode, number,
                    request_file=self.request_file if mode == "restart" else None)
            except BaseException as error:
                finish(self.directory, number, error=error, verify_results=False)
                raise
            finish(self.directory, number, result=result, error=ValueError("outer") if outer_failure else None)
            return result


class TransportTests(unittest.TestCase):
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
        with self.patches():
            self.assertEqual(model.validate_request(self.backend, self.directory, self.value, self.request), self.private)

    def test_registered_lookup_passes_only_explicit_initialization_lock(self):
        with self.patches():
            self.execute("restart")
            with patch.object(model, "initialization_history", return_value=(self.initial, self.original)) as history:
                cache.registered_cache_binding(self.backend, self.directory, owned_lock_identity=123)
                self.assertEqual(history.call_args.args[-1], 123)


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




class CacheAdapterTests(CacheFixture):
    def test_restart_uses_real_redis_transport_for_marker_protocol(self):
        bulk = lambda value: f"${len(value.encode())}\r\n{value}\r\n".encode()
        replies = [b"*2\r\n$1\r\n0\r\n*0\r\n", b"$-1\r\n", b"$-1\r\n", b":1\r\n",
                   b"*2\r\n$1\r\n0\r\n*1\r\n" + bulk(self.marker["ownership_key"]),
                   bulk(self.marker["ownership_value"]), bulk(self.marker["sentinel_value"])]
        connections = []
        for reply in replies:
            connection = MagicMock()
            connection.__enter__.return_value = connection
            connection.makefile.return_value.__enter__.return_value = BytesIO(b"+OK\r\n+OK\r\n" + reply)
            connections.append(connection)
        real_transport = cache.transport
        with self.patches(), patch.object(cache, "transport", wraps=real_transport), \
                patch("devex_clone_target_resources.socket.create_connection", side_effect=connections) as connect:
            self.assertEqual(self.execute("restart")["status"], "cache_ready")
        commands = []
        for connection in connections:
            sent = [Resources._response(BytesIO(call.args[0])) for call in connection.sendall.call_args_list]
            self.assertEqual(sent[:2], [["AUTH", self.request["process"]["password"]], ["SELECT", "0"]])
            self.assertEqual(len(sent), 3)
            commands.append(sent[2])
        self.assertEqual([command[0] for command in commands], ["SCAN", "GET", "GET", "MSETNX", "SCAN", "GET", "GET"])
        self.assertEqual(commands[3], ["MSETNX", self.marker["ownership_key"], self.marker["ownership_value"],
                                       self.marker["sentinel_key"], self.marker["sentinel_value"]])
        self.assertEqual(connect.call_count, 7)
        connect.assert_called_with(("127.0.0.1", self.previous["port"]), timeout=5)


    def test_stage_port_preserves_original_cancellation_and_never_retries_start(self):
        with self.patches(), patch.object(process, "start", side_effect=KeyboardInterrupt) as start:
            with self.assertRaises(KeyboardInterrupt):
                self.execute("restart")
        start.assert_called_once()
        self.assertEqual(self.commands, [])
        self.assertFalse((self.directory / "cache-target/a0001/ready.json").exists())

    def test_stage_port_keeps_unknown_transport_result_as_original_timeout(self):
        with self.patches():
            self.failure = "after"
            with self.assertRaises(TimeoutError):
                self.execute("restart")
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 1)
        self.assertTrue((self.directory / "cache-target/a0001/owner/intent.json").is_file())
        self.assertFalse((self.directory / "cache-target/a0001/owner/confirmed.json").exists())


if __name__ == "__main__":
    unittest.main()
