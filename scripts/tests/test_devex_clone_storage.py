"""存储代次的固定输入、外层发布与失败关闭，不连接真实服务。"""
from contextlib import ExitStack, nullcontext
import copy
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import Mock, patch

import devex_clone_storage as storage
import devex_clone_storage_process as process
import devex_clone_storage_request as model
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import (binding, begin, bind_controller_attempt, finish, initialize_state, run_lock)
from full_stack_process import process_identity, write_receipt


class StorageTests(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="sr-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.directory = self.root / "run"
        self.directory.mkdir()
        self.data = self.root / "data"
        self.data.mkdir()
        exe = self.file("rustfs.exe", b"fixture-rustfs")
        self.old = {"pid": 610001, "started": "100", "executable": exe["path"]}
        self.new = {**self.old, "pid": 610002, "started": "200"}
        self.alive = False
        self.controller = process_identity(os.getpid())
        self.credentials = {key: self.file(key + ".txt", (key + "-private").encode()) for key in ("access_key", "secret_key")}
        self.private = {"KEY": "access_key-private", "SECRET": "secret_key-private", "RUSTFS_UNDECLARED": "forbidden"}
        environment = self.json("private.json", {"environment": self.private})
        source = self.json("source.json", {"source": {"s3": {"endpoint": "http://127.0.0.1:18290", "access_key_env": "KEY", "secret_key_env": "SECRET"}}})
        self.value = {"id": "same-run", "source_request": source, "initialized": self.json("initialized.json", {"fixture": True}),
                      "source_environment": environment, "target_environment": environment}
        write_json(self.directory / "manifest.json", self.value)
        initialize_state(self.directory)
        launch = self.json("old-launch.json", {"scope_id": "storage-fixture", "executable": exe, "data_dir": str(self.data),
            "api_url": "http://127.0.0.1:18290", "console_url": "http://127.0.0.1:18291",
            "access_key_file": self.credentials["access_key"]["path"], "secret_key_file": self.credentials["secret_key"]["path"]})
        self.request = {"format_version": 1, "kind": "devex-clone-storage-restart", "side": "source",
            "manifest": binding(self.directory / "manifest.json"), "original": source, "environment": environment,
            "scope_id": "storage-fixture", "executable": exe,
            "data_directory": {"path": str(self.data), "device": self.data.stat().st_dev, "inode": self.data.stat().st_ino},
            "api_url": "http://127.0.0.1:18290", "console_url": "http://127.0.0.1:18291",
            "credential_files": self.credentials, "previous": {}, "timeout_seconds": 5}
        old_process = self.json("old-process.json", {"scope_id": "storage-fixture", "identity": self.old, "data_dir": str(self.data),
            "launch_plan": launch, "command": model.arguments(self.request), "lifecycle": "started_unready",
            "secret_files": {key + "_file": item for key, item in self.credentials.items()}})
        ready = self.json("old-ready.json", {"status": "ready", "identity": self.old, "process_receipt": old_process,
            "data_dir": str(self.data), "listeners": [self.request["api_url"], self.request["console_url"]]})
        self.request["previous"] = {"identity": self.old, "process_receipt": old_process, "launch_receipt": launch, "ready_receipt": ready}
        self.request_file = Path(self.json("request.json", self.request)["path"])
        self.child = Mock(pid=self.new["pid"])
        self.child.poll.return_value = None
        self.child.wait.return_value = 0

    def file(self, name, contents):
        path = self.root / name
        path.write_bytes(contents)
        return binding(path)

    def json(self, name, value):
        path = self.root / name
        write_json(path, value)
        return binding(path)

    def identity(self, pid):
        if pid == os.getpid():
            return self.controller
        return self.new if pid == self.new["pid"] and self.alive else None

    def popen(self, *_args, **kwargs):
        self.assertNotIn("RUSTFS_UNDECLARED", kwargs["env"])
        self.assertEqual(kwargs["env"]["RUSTFS_SECRET_KEY_FILE"], self.credentials["secret_key"]["path"])
        self.new = {**self.new, "pid": self.new["pid"] + 1, "started": str(int(self.new["started"]) + 1)}
        self.child.pid = self.new["pid"]
        self.alive = True
        return self.child

    def terminate(self, expected):
        self.assertEqual(expected, self.new)
        self.alive = False
        return True

    def patches(self):
        stack = ExitStack()
        for module in (storage, process):
            stack.enter_context(patch.object(module, "process_identity", side_effect=self.identity))
            stack.enter_context(patch.object(module, "require_closed_port"))
        stack.enter_context(patch.object(storage, "producers_stopped"))
        def require_stopped(expected, *, run, counter_run):
            self.assertIs(run, subprocess.run)
            self.assertIs(counter_run, subprocess.run)
            current = self.identity(expected["pid"])
            if current is not None and int(current["started"]) <= int(expected["started"]):
                raise ValueError("登记存储代次仍在运行")
        stack.enter_context(patch.object(storage, "require_recorded_producer_stopped",
                                         side_effect=require_stopped))
        stack.enter_context(patch.object(storage, "protect_binaries", side_effect=lambda *_: nullcontext()))
        stack.enter_context(patch.object(process, "actual_arguments"))
        stack.enter_context(patch.object(process, "verify_listener"))
        stack.enter_context(patch.object(process, "wait_ready"))
        stack.enter_context(patch.object(process, "terminate_owned_process", side_effect=self.terminate))
        stack.enter_context(patch.object(process.subprocess, "Popen", side_effect=self.popen))
        return stack

    @staticmethod
    def denied():
        error = PermissionError("fixture access denied")
        error.winerror = 5
        return error

    def execute(self, mode, *, failure=None, request=None):
        with run_lock(self.directory) as owner:
            number = begin(self.directory, "storage-source", mode, {})
            bind_controller_attempt(self.directory, number, owner)
            try:
                result = storage.execute_storage(self.backend, self.directory, self.value, mode, number, "source",
                                                 request_file=(request or self.request_file) if mode == "restart" else None)
            except BaseException as error:
                finish(self.directory, number, error=error, verify_results=False)
                raise
            finish(self.directory, number, result=result, error=failure, verify_results=False)
            return result

    def test_source_request_binds_original_ready_and_data_directory(self):
        self.assertEqual(model.validate_request(self.backend, self.directory, self.value, self.request, "source"), self.private)

    def test_restart_preflight_is_readonly_before_registration(self):
        before = {str(path.relative_to(self.root)): path.read_bytes()
                  for path in self.root.rglob("*") if path.is_file()}
        with self.patches():
            storage.preflight_restart(self.backend, self.directory, self.value, "source", self.request_file)
        self.assertFalse((self.directory / "storage-source").exists())
        self.assertEqual(before, {str(path.relative_to(self.root)): path.read_bytes()
                                  for path in self.root.rglob("*") if path.is_file()})

    def test_previous_generations_use_readonly_reuse_aware_proof(self):
        recorded = {**self.old, "pid": self.old["pid"] + 1, "started": "150"}
        previous = [(Mock(), {"state": "recorded", "identity": recorded})]
        with patch.object(storage, "previous_attempts", return_value=previous), \
                patch.object(storage, "require_recorded_producer_stopped") as stopped, \
                patch.object(storage, "require_closed_port"), \
                patch("full_stack_process.terminate_owned_process") as terminate:
            storage.require_previous_stopped(self.backend, self.directory, self.request,
                                             binding(self.request_file), None)
        self.assertEqual([call.args[0] for call in stopped.call_args_list], [self.old, recorded])
        self.assertTrue(all(call.kwargs == {"run": subprocess.run, "counter_run": subprocess.run}
                            for call in stopped.call_args_list))
        terminate.assert_not_called()

    def test_protected_reused_pid_is_proven_by_cim_without_termination(self):
        system_root = self.root / "windows"
        powershell = system_root / "System32/WindowsPowerShell/v1.0/powershell.exe"
        powershell.parent.mkdir(parents=True)
        powershell.write_bytes(b"fixture")
        observed = {"count": 1, "pid": self.old["pid"], "kind": "Utc", "started": "120",
                    "precision_ticks": 10}
        response = subprocess.CompletedProcess([], 0, json.dumps(observed).encode(), b"")
        import devex_clone_source_proof as source_proof
        with patch.object(storage, "previous_attempts", return_value=[]), \
                patch.object(storage, "require_closed_port"), \
                patch.object(source_proof, "process_identity", side_effect=self.denied()), \
                patch.object(source_proof, "os", SimpleNamespace(name="nt", environ={"SystemRoot": str(system_root)})), \
                patch.object(storage.subprocess, "run", return_value=response) as query, \
                patch("full_stack_process.terminate_owned_process") as terminate:
            storage.require_previous_stopped(self.backend, self.directory, self.request,
                                             binding(self.request_file), None)
        query.assert_called_once()
        terminate.assert_not_called()

    def test_live_original_generation_rejects_restart_without_termination(self):
        import devex_clone_source_proof as source_proof
        with patch.object(storage, "previous_attempts", return_value=[]), \
                patch.object(storage, "require_closed_port"), \
                patch.object(source_proof, "process_identity", return_value=self.old), \
                patch("full_stack_process.terminate_owned_process") as terminate, \
                self.assertRaises(ValueError):
            storage.require_previous_stopped(self.backend, self.directory, self.request,
                                             binding(self.request_file), None)
        terminate.assert_not_called()

    def test_missing_source_ready_cannot_infer_from_unready_process(self):
        request = copy.deepcopy(self.request)
        request["previous"]["ready_receipt"] = request["previous"]["process_receipt"]
        with self.assertRaises(ValueError):
            model.validate_request(self.backend, self.directory, self.value, request, "source")

    def test_request_rejects_changed_directory_credentials_ports_or_original(self):
        for key, value in (("data_directory", {**self.request["data_directory"], "inode": 1}),
                           ("api_url", self.request["console_url"]), ("original", self.value["initialized"]),
                           ("timeout_seconds", True), ("executable", self.credentials["access_key"])):
            with self.subTest(key=key), self.assertRaises(ValueError):
                model.validate_request(self.backend, self.directory, self.value, {**self.request, key: value}, "source")
        Path(self.credentials["secret_key"]["path"]).write_text("changed", encoding="utf-8")
        with self.assertRaises(ValueError):
            model.validate_request(self.backend, self.directory, self.value, self.request, "source")

    def test_never_registered_returns_none(self):
        self.assertIsNone(storage.current_storage_binding(self.backend, self.directory, "source"))

    def test_status_never_writes_stage_or_evidence(self):
        with self.patches():
            self.execute("restart", failure=ValueError("outer failure"))
            before = {str(path): binding(path) for path in self.directory.rglob("*") if path.is_file()}
            result = storage.storage_status(self.backend, self.directory, "source")
            with self.assertRaises(ValueError):
                storage.execute_storage(self.backend, self.directory, self.value, "status", 2, "source")
            self.assertEqual(result["processes"][0]["state"], "running")
            self.assertFalse(result["copy_usable"])
            self.assertEqual(before, {str(path): binding(path) for path in self.directory.rglob("*") if path.is_file()})

    def test_restart_preserves_originals_and_requires_outer_publication(self):
        original = {key: binding(Path(item["path"])) for key, item in self.request["previous"].items() if key != "identity"}
        with self.patches():
            with run_lock(self.directory) as owner:
                number = begin(self.directory, "storage-source", "restart", {})
                bind_controller_attempt(self.directory, number, owner)
                result = storage.execute_storage(self.backend, self.directory, self.value, "restart", number, "source", self.request_file)
                with self.assertRaises(ValueError):
                    storage.registered_storage_binding(self.backend, self.directory, "source")
                finish(self.directory, number, result=result)
            proof = storage.current_storage_binding(self.backend, self.directory, "source")
        self.assertEqual(proof["storage"]["identity"], self.new)
        self.assertEqual(original, {key: binding(Path(item["path"])) for key, item in self.request["previous"].items() if key != "identity"})

    def test_local_binding_does_not_repeat_native_process_checks(self):
        with self.patches():
            self.execute("restart")
            with patch.object(process, "actual_arguments", side_effect=AssertionError("unexpected CIM")), patch.object(storage, "process_identity", side_effect=AssertionError("unexpected process query")):
                proof = storage.registered_storage_binding(self.backend, self.directory, "source")
        self.assertEqual(proof["attempt"], 1)

    def test_live_duplicate_start_fails_and_cannot_fall_back_to_old_success(self):
        with self.patches():
            self.execute("restart")
            with self.assertRaises(ValueError):
                self.execute("restart")
            with self.assertRaises(ValueError):
                storage.registered_storage_binding(self.backend, self.directory, "source")
            self.execute("recover")
            self.assertFalse(self.alive)
            self.execute("restart")
            self.assertEqual(storage.current_storage_binding(self.backend, self.directory, "source")["attempt"], 4)

    def test_outer_publication_failure_preserves_exact_recoverable_process(self):
        with self.patches():
            self.execute("restart", failure=ValueError("outer publication"))
            self.assertTrue(self.alive)
            with self.assertRaises(ValueError):
                storage.current_storage_binding(self.backend, self.directory, "source")
            self.execute("recover")
            self.assertFalse(self.alive)
            self.execute("recover")
            with self.assertRaises(ValueError):
                storage.registered_storage_binding(self.backend, self.directory, "source")

    def test_stop_does_not_require_current_tool_credentials_or_data_directory(self):
        with self.patches():
            self.execute("restart")
            Path(self.request["executable"]["path"]).unlink()
            Path(self.credentials["secret_key"]["path"]).unlink()
            self.data.rmdir()
            result = self.execute("stop")
        self.assertFalse(self.alive)
        self.assertEqual(result["status"], "storage_stopped")

    def test_pid_reuse_or_wrong_actual_arguments_never_terminates(self):
        with self.patches():
            self.execute("restart")
            with patch.object(process, "process_identity", return_value={**self.new, "started": "other"}), patch.object(process, "terminate_owned_process") as terminate:
                with self.assertRaises(ValueError):
                    self.execute("recover")
                terminate.assert_not_called()
            with patch.object(process, "actual_arguments", side_effect=ValueError("wrong argv")), patch.object(process, "terminate_owned_process") as terminate:
                with self.assertRaises(ValueError):
                    self.execute("stop")
                terminate.assert_not_called()

    def test_unknown_start_intent_blocks_even_when_pid_file_exists(self):
        with self.patches():
            self.execute("restart", failure=ValueError("unknown"))
            (self.directory / "storage-source/a0001/process.json").unlink()
            for mode in ("recover", "restart"):
                with self.assertRaises(ValueError):
                    self.execute(mode)

    def test_post_copy_registration_rejects_restart_but_does_not_block_cleanup(self):
        write_json(self.directory / "post-copy.json", {"registered": True})
        with self.assertRaises(ValueError):
            model.producers_stopped(self.backend, self.directory, self.value)
        with self.patches():
            self.execute("restart")
            self.execute("stop")
        self.assertFalse(self.alive)

    def test_failed_native_start_preserves_error_and_cleans_own_child(self):
        original = TimeoutError("not ready")
        with self.patches(), patch.object(process, "wait_ready", side_effect=original):
            with self.assertRaises(TimeoutError) as caught:
                self.execute("restart")
        self.assertIs(caught.exception, original)
        self.assertFalse(self.alive)
        self.assertTrue((self.directory / "storage-source/a0001/failure-cleanup.json").exists())

    def test_popen_constructor_failure_remains_unknown_without_returned_handle(self):
        with self.patches(), patch.object(process.subprocess, "Popen", side_effect=OSError("launch failed")):
            with self.assertRaises(OSError):
                self.execute("restart")
        self.assertFalse((self.directory / "storage-source/a0001/not-started.json").exists())
        with self.patches():
            with self.assertRaises(ValueError):
                self.execute("restart")

    def test_receipt_write_failure_with_exact_child_reaped_can_restart(self):
        original_write = process.write_json
        def fail_receipt(path, value):
            if path.name == "process.json":
                raise OSError("receipt write")
            return original_write(path, value)
        with self.patches(), patch.object(process, "write_json", side_effect=fail_receipt):
            with self.assertRaises(OSError):
                self.execute("restart")
        self.assertFalse(self.alive)
        with self.patches():
            self.execute("restart")

    def test_cleanup_failure_does_not_replace_original_start_error(self):
        original = TimeoutError("not ready")
        with self.patches(), patch.object(process, "wait_ready", side_effect=original), patch.object(process, "terminate_owned_process", side_effect=PermissionError("no access")):
            with self.assertRaises(TimeoutError) as caught:
                self.execute("restart")
        self.assertIs(caught.exception, original)
        self.assertIn("PermissionError", caught.exception.__notes__[0])

    def test_unverified_executable_only_uses_original_popen_handle_for_cleanup(self):
        with self.patches(), patch.object(process, "process_identity", side_effect=lambda pid: {**self.new, "executable": "other.exe"}), patch.object(process, "terminate_owned_process") as terminate, patch.object(self.child, "kill") as kill:
            with self.assertRaises(ValueError):
                self.execute("restart")
            terminate.assert_not_called()
            kill.assert_called_once_with()
        cleanup = read_json(self.directory / "storage-source/a0001/failure-cleanup.json")
        self.assertIsNone(cleanup["identity"])

    def test_wrong_run_controller_and_reentrant_guard_rejected(self):
        with self.patches(), run_lock(self.directory) as owner:
            number = begin(self.directory, "storage-source", "restart", {})
            bind_controller_attempt(self.directory, number, owner)
            with storage.process_guard(self.directory, "storage-source.guard"):
                with self.assertRaises(ValueError):
                    storage.execute_storage(self.backend, self.directory, self.value, "restart", number, "source", self.request_file)
            path = self.directory / "controller-0001.json"
            receipt = read_json(path)
            receipt["owner"]["directory"] = str(self.root)
            write_receipt(path, receipt)
            with self.assertRaises(ValueError):
                storage.execute_storage(self.backend, self.directory, self.value, "restart", number, "source", self.request_file)
            finish(self.directory, number, error=ValueError("test complete"))


if __name__ == "__main__":
    unittest.main()
