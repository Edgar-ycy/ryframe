"""存储代次的固定输入、外层发布与失败关闭，不连接真实服务。"""
import copy
from pathlib import Path
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

import devex_clone_storage as storage
import devex_clone_storage_process as process
import devex_clone_storage_request as model
from devex_clone_capture import write_json
from devex_clone_run_state import (binding, begin, bind_controller_attempt, finish, initialize_state, run_lock)
from restore_reference_plan import configuration_binding


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
        self.credentials = {key: self.file(key + ".txt", (key + "-private").encode()) for key in ("access_key", "secret_key")}
        self.private = {"KEY": "access_key-private", "SECRET": "secret_key-private", "RUSTFS_UNDECLARED": "forbidden",
                        "rustfs_access_key_file": "wrong-file", "MINIO_ROOT_PASSWORD": "forbidden",
                        "minio_secret_key": "forbidden", "APP_ENV": "test", "PATH": "fixture-bin",
                        "CARGO_HOME": "fixture-cargo", "COREPACK_HOME": "fixture-corepack"}
        environment = self.json("private.json", {"environment": self.private})
        configuration = self.root / "config"
        configuration.mkdir()
        (configuration / "app.test.toml").write_text(
            '[object_storage]\nbackend="rustfs"\nendpoint="http://127.0.0.1:18290"\nregion="us-east-1"\n'
            'access_key="access_key-private"\nsecret_key="secret_key-private"\n', encoding="utf-8")
        source = self.json("source.json", {"source": {"s3": {"configuration": configuration_binding(self.backend, configuration)}}})
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

    def file(self, name, contents):
        path = self.root / name
        path.write_bytes(contents)
        return binding(path)

    def json(self, name, value):
        path = self.root / name
        write_json(path, value)
        return binding(path)

    def test_source_request_binds_original_ready_and_data_directory(self):
        self.assertEqual(model.validate_request(self.backend, self.directory, self.value, self.request, "source"), self.private)

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

    def frozen_restart(self):
        """静态普通文件夹具；不调用已退休生命周期或启动任何进程。"""
        root = self.directory / "storage-source"
        output = root / "a0001"
        output.mkdir(parents=True)
        write_json(root / "request.json", self.request)
        descriptor = binding(root / "request.json")
        write_json(root / "registration.json", {"request": descriptor,
            "manifest": binding(self.directory / "manifest.json"), "side": "source"})
        with run_lock(self.directory) as owner:
            number = begin(self.directory, "storage-source", "restart", {})
            bind_controller_attempt(self.directory, number, owner)
            attempt = storage.records(self.directory, "source")[-1]
            controller = storage.controller_binding(self.backend, self.directory, attempt)
            write_json(output / "intent.json", {"format_version": 1, "kind": "devex-clone-storage-intent",
                "request": descriptor, "controller": controller, "attempt": number,
                "arguments": model.arguments(self.request)})
            write_json(output / "process.json", {"format_version": 1, "role": "rustfs",
                "scope_id": self.request["scope_id"], "lifecycle": "running", "identity": self.new,
                "api_url": self.request["api_url"], "console_url": self.request["console_url"],
                "data_dir": str(self.data), "intent": binding(output / "intent.json")})
            write_json(output / "launch.json", {"format_version": 1, "scope_id": self.request["scope_id"],
                "identity": self.new, "arguments": model.arguments(self.request), "credential_files": self.credentials})
            process_binding = {"identity": self.new, "sha256": self.request["executable"]["sha256"],
                "process_receipt": binding(output / "process.json"), "launch_receipt": binding(output / "launch.json")}
            write_json(output / "ready.json", {"storage": process_binding, "request": descriptor,
                "controller": controller, "attempt": number,
                "listeners": [self.request["api_url"], self.request["console_url"]]})
            finish(self.directory, number, result={"status": "storage_restarted", "side": "source",
                "request": descriptor, "storage": process_binding, "ready": binding(output / "ready.json"),
                "attempt": number, "data_directory": self.request["data_directory"],
                "api_url": self.request["api_url"], "console_url": self.request["console_url"]})

    def test_local_binding_does_not_repeat_native_process_checks(self):
        self.frozen_restart()
        before = {str(path): binding(path) for path in self.directory.rglob("*") if path.is_file()}
        with patch.object(process, "actual_arguments", side_effect=AssertionError("unexpected CIM")), \
                patch.object(process, "process_identity", side_effect=AssertionError("unexpected process query")):
            proof = storage.registered_storage_binding(self.backend, self.directory, "source")
        self.assertEqual(proof["attempt"], 1)
        self.assertEqual(before, {str(path): binding(path) for path in self.directory.rglob("*") if path.is_file()})

    def test_failed_later_attempt_cannot_fall_back_to_frozen_success(self):
        self.frozen_restart()
        self.assertEqual(storage.registered_storage_binding(self.backend, self.directory, "source")["attempt"], 1)
        with run_lock(self.directory) as owner:
            number = begin(self.directory, "storage-source", "status", {})
            bind_controller_attempt(self.directory, number, owner)
            finish(self.directory, number, error=ValueError("fixture failure"))
        before = binding(self.directory / "state.json")
        with self.assertRaises(ValueError):
            storage.registered_storage_binding(self.backend, self.directory, "source")
        self.assertEqual(binding(self.directory / "state.json"), before)


if __name__ == "__main__":
    unittest.main()
