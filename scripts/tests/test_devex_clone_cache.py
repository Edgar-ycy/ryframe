"""缓存恢复的请求、原子标记及外层发布；全部使用文件和协议替身。"""
from contextlib import ExitStack
import copy
from pathlib import Path
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

import devex_clone_cache as cache
import devex_clone_cache_owner as markers
import devex_clone_cache_process as process
import devex_clone_cache_request as model
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding, begin, bind_controller_attempt, finish, initialize_state, run_lock
from full_stack_process import write_receipt


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
        self.private = {"APP_REDIS_HOST": "127.0.0.1", "APP_REDIS_PORT": "16390", "APP_REDIS_DATABASE": "0",
                        "APP_REDIS_TLS": "false", "APP_REDIS_PASSWORD": "fixture-secret"}
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
            previous_boot_id=boot, previous_run_id=self.previous["run_id"], password_env="APP_REDIS_PASSWORD", timeout_seconds=5,
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
        stack.enter_context(patch.object(model, "initialization_history", return_value=(self.initial, self.original)))
        stack.enter_context(patch.object(model, "request_binding", return_value=(self.review, self.selected)))
        stack.enter_context(patch.object(cache, "producers_stopped"))
        stack.enter_context(patch.object(cache, "transport", return_value=self.call))
        for name in ("start", "inspect", "observe", "stop"):
            stack.enter_context(patch.object(process, "inspect_start" if name == "inspect" else name, side_effect=getattr(self, name)))
        stack.enter_context(patch.object(process, "status", side_effect=lambda *_: {"state": "running" if self.alive else "stopped"}))
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


class CacheTests(CacheFixture):
    def test_restart_preserves_original_inputs_and_requires_outer_publication(self):
        originals = {path: binding(path) for path in self.root.iterdir() if path.is_file()}
        with self.patches():
            self.execute("restart", outer_failure=True)
            with self.assertRaises(ValueError):
                cache.registered_cache_binding(self.backend, self.directory)
            self.execute("reconcile")
            with patch.object(process, "observe", side_effect=AssertionError("live call")), patch.object(cache, "producers_stopped", side_effect=AssertionError("business guard")):
                proof = cache.registered_cache_binding(self.backend, self.directory)
        self.assertEqual(proof["attempt"], 2)
        self.assertEqual(originals, {path: binding(path) for path in originals})
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 1)

    def test_unknown_after_response_only_reconciles_without_replay(self):
        with self.patches():
            self.failure = "after"
            with self.assertRaises(TimeoutError):
                self.execute("restart")
            with self.assertRaises(ValueError):
                self.execute("resume")
            self.failure = None
            self.assertEqual(self.execute("reconcile")["status"], "cache_ready")
            self.assertEqual(cache.registered_cache_binding(self.backend, self.directory)["attempt"], 3)
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 1)

    def test_unknown_before_requires_reconcile_then_explicit_resume(self):
        with self.patches():
            self.failure = "before"
            with self.assertRaises(TimeoutError):
                self.execute("restart")
            self.failure = None
            with self.assertRaises(ValueError):
                self.execute("resume")
            self.assertEqual(self.execute("reconcile")["status"], "cache_reconciled_before")
            with self.assertRaises((ValueError, FileNotFoundError)):
                cache.registered_cache_binding(self.backend, self.directory)
            self.execute("resume")
            self.assertEqual(cache.registered_cache_binding(self.backend, self.directory)["attempt"], 4)
            self.execute("stop")
            self.execute("restart")
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 3)

    def test_partial_or_unknown_namespace_reconcile_blocks_all_writes(self):
        with self.patches():
            self.failure = "before"
            with self.assertRaises(TimeoutError):
                self.execute("restart")
            self.store[self.marker["ownership_key"]] = self.marker["ownership_value"]
            with self.assertRaises(ValueError):
                self.execute("reconcile")
            with self.assertRaises(ValueError):
                self.execute("resume")
        self.assertEqual(sum(command[0] == "MSETNX" for command in self.commands), 1)

    def test_duplicate_restart_and_stop_never_fall_back_to_old_success(self):
        with self.patches():
            self.execute("restart")
            with self.assertRaises(ValueError):
                self.execute("restart")
            with self.assertRaises(ValueError):
                cache.registered_cache_binding(self.backend, self.directory)
            self.execute("stop")
            with self.assertRaises(ValueError):
                cache.registered_cache_binding(self.backend, self.directory)
            self.execute("restart")
            self.assertEqual(cache.registered_cache_binding(self.backend, self.directory)["attempt"], 4)

    def test_status_is_readonly_and_cleanup_ignores_current_product_source(self):
        with self.patches():
            self.execute("restart")
            before = {path: binding(path) for path in self.directory.rglob("*") if path.is_file()}
            self.assertEqual(cache.cache_status(self.backend, self.directory)["processes"][0]["process"]["state"], "running")
            self.assertEqual(before, {path: binding(path) for path in self.directory.rglob("*") if path.is_file()})
            Path(self.value["target_environment"]["path"]).unlink()
            with patch.object(cache, "validate_request", side_effect=AssertionError("current product")):
                self.execute("recover")
            self.assertFalse(self.alive)

    def test_guard_failure_has_no_marker_write_and_no_publication(self):
        with self.patches(), patch.object(cache, "producers_stopped", side_effect=ValueError("producer alive")):
            with self.assertRaises(ValueError):
                self.execute("restart")
        self.assertEqual(self.commands, [])
        self.assertFalse(self.alive)

    def test_cross_attempt_owner_observation_is_rejected(self):
        with self.patches():
            self.failure = "before"
            with self.assertRaises(TimeoutError):
                self.execute("restart")
            self.execute("reconcile")
            owner = self.directory / cache.STAGE / "a0001/owner"
            proof = read_json(owner / "reconcile-0002.json")
            proof["observed"] = self.json("foreign-before.json", markers.images(self.marker)[0])
            write_receipt(owner / "reconcile-0002.json", proof)
            with self.assertRaises(ValueError):
                self.execute("resume")

    def test_invalid_confirmation_cannot_hide_earlier_unknown_generation(self):
        with self.patches():
            self.execute("restart")
            write_receipt(self.directory / cache.STAGE / "a0001/owner/confirmed.json", {})
            self.execute("stop")
            with self.assertRaises(ValueError):
                self.execute("restart")
        self.assertEqual(self.counter, 1)

    def test_cleanup_proven_partial_start_allows_only_new_restart(self):
        def partial(request, environment, output, guard):
            write_json(output / "base.json", {"state": "not_started", "output": str(output)})
            raise TimeoutError("fixture precise partial cleanup")
        with self.patches():
            with patch.object(process, "start", side_effect=partial), self.assertRaises(TimeoutError):
                self.execute("restart")
            with self.assertRaises(ValueError):
                self.execute("resume")
            self.execute("restart")
            self.assertEqual(cache.registered_cache_binding(self.backend, self.directory)["attempt"], 3)


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

    def test_request_rejects_changed_original_and_markers(self):
        changes = [("side", "source"), ("previous", {**self.previous, "port": 16391}),
                   ("markers", {**self.marker, "sentinel_value": "other"}), ("initialized", self.value["target_environment"])]
        with self.patches():
            for key, value in changes:
                with self.subTest(key=key), self.assertRaises(ValueError):
                    model.validate_request(self.backend, self.directory, self.value, {**self.request, key: value})

    def test_request_rejects_changed_directory_credential_or_tool(self):
        with self.patches():
            for field, value in (("directory", {**self.request["process"]["directory"], "inode": 1}),
                                 ("executable", "/usr/bin/other"), ("port", 16391), ("scope_id", "other-cache")):
                request = copy.deepcopy(self.request)
                request["process"][field] = value
                with self.subTest(field=field), self.assertRaises(ValueError):
                    model.validate_request(self.backend, self.directory, self.value, request)
            self.private["APP_REDIS_PASSWORD"] = "different"
            write_receipt(Path(self.value["target_environment"]["path"]), {"environment": self.private})
            with self.assertRaises(ValueError):
                model.validate_request(self.backend, self.directory, self.value, self.request)

    def test_namespace_capture_rejects_outside_keys_and_cursor_loops(self):
        for response in (["0", ["foreign"]], ["3", []], ["0", "not-array"]):
            with self.subTest(response=response), self.assertRaises(ValueError):
                markers.capture(lambda _: response, self.marker)


if __name__ == "__main__":
    unittest.main()
