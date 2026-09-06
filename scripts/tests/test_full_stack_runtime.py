"""用本地配置和合成二进制验证运行收据，不启动服务或执行 Cargo。"""
import hashlib
import json
import os
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import full_stack_runtime as runtime
from ci_full_stack_resources import BINARIES


class RuntimeContractTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.backend = self.root / "中文源码 目录"
        self.configuration = self.backend / "config"
        self.configuration.mkdir(parents=True)
        self.config = self.configuration / "app.toml"
        self.config.write_text('[app]\nport=18210\n', encoding="utf-8")
        self.directory = self.root / "runtime"
        self.directory.mkdir()
        artifacts = self.root / "产物 目录"
        artifacts.mkdir()
        self.binaries = {}
        for _, name in BINARIES:
            path = artifacts / (name + ".exe")
            path.write_bytes(name.encode())
            self.binaries[name] = str(path)
        self.manifest = self.directory / "binaries.json"
        self.manifest.write_text(json.dumps(self.binaries), encoding="utf-8")
        self.receipt = self.directory / "runtime.json"
        environment = patch.dict(os.environ, {
            "APP_ENV": "test", "APP_SCOPE_ID": "runtime-fixture", "APP_JOBS_MODE": "external",
            "APP_JOBS_HEALTH_HOST": "127.0.0.1", "APP_JOBS_HEALTH_PORT": "19210",
            "APP_DB_PASSWORD": "fixture-private-password",
        }, clear=True)
        environment.start()
        self.addCleanup(environment.stop)

    def assert_rejected_without_replacing(self, original):
        for operation in (runtime.register_runtime, runtime.verify_runtime):
            with self.subTest(operation=operation.__name__), self.assertRaises(ValueError):
                operation(self.backend, self.directory)
            self.assertEqual(self.receipt.read_bytes(), original)

    def test_registration_roundtrip_binds_exact_artifacts_without_exposing_environment(self):
        value = runtime.register_runtime(self.backend, self.directory)
        self.assertEqual(value["backend_root"], str(self.backend))
        self.assertEqual(value["scope_id"], "runtime-fixture")
        self.assertEqual(value["worker_ready_url"], "http://127.0.0.1:19210/readyz")
        self.assertEqual(value["format_version"], 1)
        self.assertEqual(value["artifacts"], {
            role: {"path": self.binaries[name], "sha256": hashlib.sha256(name.encode()).hexdigest()}
            for role, name in (("api", "ryframe"), ("worker", "ryframe-worker"))
        })
        before = self.receipt.read_bytes()
        self.assertNotIn(b"fixture-private-password", before)
        self.assertNotIn(b"APP_DB_PASSWORD", before)
        with patch.object(runtime, "write_receipt", side_effect=AssertionError("禁止重复写入")):
            self.assertEqual(runtime.register_runtime(self.backend, self.directory), value)
            self.assertEqual(runtime.verify_runtime(self.backend, self.directory), value)
        self.assertEqual(self.receipt.read_bytes(), before)

    def test_full_stack_source_is_checked_before_registration_and_on_verify(self):
        source = {"format_version": 1, "fixture": "device"}
        with patch.object(runtime, "full_stack_source_evidence", return_value=source) as capture, \
                patch.object(runtime, "verify_build_evidence", return_value={"source": source}) as build, \
                patch.object(runtime, "register_runtime_evidence") as register:
            value = runtime.register_runtime(self.backend, self.directory)
        capture.assert_called_once_with(self.backend, self.directory)
        build.assert_called_once_with(self.backend, self.directory)
        register.assert_called_once_with(self.backend, self.directory, self.receipt)

        with patch.object(runtime, "verify_runtime_evidence") as verify:
            self.assertEqual(runtime.verify_runtime(self.backend, self.directory), value)
        verify.assert_called_once_with(self.backend, self.directory, self.receipt)

    def test_build_source_mismatch_does_not_publish_runtime_receipt(self):
        source = {"format_version": 1, "fixture": "device"}
        with patch.object(runtime, "full_stack_source_evidence", return_value=source), \
                patch.object(runtime, "verify_build_evidence", return_value={"source": {}}), \
                patch.object(runtime, "write_receipt") as write:
            with self.assertRaisesRegex(ValueError, "构建来源"):
                runtime.register_runtime(self.backend, self.directory)
        write.assert_not_called()
        self.assertFalse(self.receipt.exists())

    def test_configuration_digest_tracks_all_toml_and_app_values_only(self):
        original = runtime.configuration_digest(self.backend)
        with patch.dict(os.environ, {"UNRELATED_VALUE": "another"}):
            self.assertEqual(runtime.configuration_digest(self.backend), original)
        (self.configuration / "notes.txt").write_text("辅助文本", encoding="utf-8")
        self.assertEqual(runtime.configuration_digest(self.backend), original)
        with patch.dict(os.environ, {"APP_DB_PASSWORD": "changed-private-password"}):
            self.assertNotEqual(runtime.configuration_digest(self.backend), original)
        additional = self.configuration / "app.test.toml"
        additional.write_text("[app]\nport=18211\n", encoding="utf-8")
        added = runtime.configuration_digest(self.backend)
        self.assertNotEqual(added, original)
        additional.write_text("[app]\nport=18212\n", encoding="utf-8")
        self.assertNotEqual(runtime.configuration_digest(self.backend), added)

    def test_config_binary_scope_and_probe_changes_do_not_replace_registered_receipt(self):
        runtime.register_runtime(self.backend, self.directory)
        original = self.receipt.read_bytes()
        for path in (self.config, Path(self.binaries["ryframe"]), Path(self.binaries["ryframe-worker"])):
            with self.subTest(path=path):
                contents = path.read_bytes()
                try:
                    path.write_bytes(contents + b"changed")
                    self.assert_rejected_without_replacing(original)
                finally:
                    path.write_bytes(contents)
        for values in ({"APP_SCOPE_ID": "different-scope"}, {"APP_JOBS_HEALTH_PORT": "19211"}):
            with self.subTest(values=values), patch.dict(os.environ, values):
                self.assert_rejected_without_replacing(original)
        self.assertEqual(runtime.verify_runtime(self.backend, self.directory), json.loads(original))

    def test_invalid_environment_and_worker_address_never_publish_receipt(self):
        cases = [
            {"APP_ENV": "prod"}, {"APP_SCOPE_ID": ""}, {"APP_SCOPE_ID": "../scope"},
            {"APP_JOBS_MODE": "inline"}, {"APP_JOBS_HEALTH_HOST": "192.0.2.1"},
            {"APP_JOBS_HEALTH_HOST": "::1"}, {"APP_JOBS_HEALTH_PORT": ""},
            {"APP_JOBS_HEALTH_PORT": "0"}, {"APP_JOBS_HEALTH_PORT": "65536"},
            {"APP_JOBS_HEALTH_PORT": "invalid"},
        ]
        for values in cases:
            with self.subTest(values=values), patch.dict(os.environ, values), \
                    patch.object(runtime, "write_receipt") as write:
                with self.assertRaises(ValueError):
                    runtime.register_runtime(self.backend, self.directory)
                write.assert_not_called()
                self.assertFalse(self.receipt.exists())

    def test_missing_tampered_or_unknown_receipt_is_not_adopted(self):
        with self.assertRaises(FileNotFoundError):
            runtime.verify_runtime(self.backend, self.directory)
        self.assertFalse(self.receipt.exists())
        valid = runtime.register_runtime(self.backend, self.directory)
        for changed in (valid | {"format_version": 2}, valid | {"extra": "unknown"},
                        valid | {"backend_root": str(self.root / "other")}):
            with self.subTest(changed=changed):
                original = json.dumps(changed).encode()
                self.receipt.write_bytes(original)
                self.assert_rejected_without_replacing(original)

    def test_all_declared_binary_roles_are_required_before_registration(self):
        for value in ({key: item for key, item in self.binaries.items() if key != "ryframe-reset"},
                      self.binaries | {"unregistered": self.binaries["ryframe"]}):
            with self.subTest(value=value):
                self.manifest.write_text(json.dumps(value), encoding="utf-8")
                with self.assertRaises(ValueError):
                    runtime.register_runtime(self.backend, self.directory)
                self.assertFalse(self.receipt.exists())

    def test_explicit_configuration_directory_is_bound_and_empty_directory_is_rejected(self):
        original = runtime.configuration_digest(self.backend)
        alternate = self.backend / "隔离 配置"
        alternate.mkdir()
        with patch.dict(os.environ, {"APP_CONFIG_DIR": alternate.name}):
            with self.assertRaises(ValueError):
                runtime.register_runtime(self.backend, self.directory)
            self.assertFalse(self.receipt.exists())
            (alternate / "app.toml").write_bytes(self.config.read_bytes())
            self.assertNotEqual(runtime.configuration_digest(self.backend), original)
            registered = runtime.register_runtime(self.backend, self.directory)
            self.assertEqual(runtime.verify_runtime(self.backend, self.directory), registered)


if __name__ == "__main__":
    unittest.main()
