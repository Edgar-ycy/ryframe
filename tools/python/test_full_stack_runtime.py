"""用本地配置、合成二进制与绑定的配置读取工具验证运行收据，不启动服务。"""
import hashlib
import json
import os
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
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
        self.backend.mkdir()
        self.configuration = self.root / "runtime/config"
        self.configuration.mkdir(parents=True)
        self.config = self.configuration / "app.toml"
        self.config.write_text('scope_id="runtime-fixture"\nsnowflake_worker_id=3\n[app]\nport=18210\n[jobs]\nmode="external"\nhealth_host="127.0.0.1"\nhealth_port=19210\n', encoding="utf-8")
        (self.configuration / "app.test.toml").write_text('[database]\npassword="fixture-private-password"\n', encoding="utf-8")
        self.worker_configuration = self.root / "runtime/worker-config"
        self.worker_configuration.mkdir()
        (self.worker_configuration / "app.toml").write_bytes(self.config.read_bytes())
        (self.worker_configuration / "app.test.toml").write_text('snowflake_worker_id=2\n[database]\npassword="fixture-private-password"\n', encoding="utf-8")
        self.directory = self.root / "runtime"
        self.directory.mkdir(exist_ok=True)
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
        source = {"format_version": 1, "fixture": "business"}
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
        source = {"format_version": 1, "fixture": "business"}
        with patch.object(runtime, "full_stack_source_evidence", return_value=source), \
                patch.object(runtime, "verify_build_evidence", return_value={"source": {}}), \
                patch.object(runtime, "write_receipt") as write:
            with self.assertRaisesRegex(ValueError, "构建来源"):
                runtime.register_runtime(self.backend, self.directory)
        write.assert_not_called()
        self.assertFalse(self.receipt.exists())

    def test_configuration_digest_tracks_bound_toml_and_ignores_old_environment(self):
        original = runtime.configuration_digest(self.backend, self.configuration)
        with patch.dict(os.environ, {"UNRELATED_VALUE": "another"}):
            self.assertEqual(runtime.configuration_digest(self.backend, self.configuration), original)
        (self.configuration / "notes.txt").write_text("辅助文本", encoding="utf-8")
        self.assertEqual(runtime.configuration_digest(self.backend, self.configuration), original)
        with patch.dict(os.environ, {"APP_DB_PASSWORD": "changed-private-password"}):
            self.assertEqual(runtime.configuration_digest(self.backend, self.configuration), original)
        additional = self.configuration / "app.test.toml"
        additional.write_text("[app]\nport=18211\n", encoding="utf-8")
        added = runtime.configuration_digest(self.backend, self.configuration)
        self.assertNotEqual(added, original)
        additional.write_text("[app]\nport=18212\n", encoding="utf-8")
        self.assertNotEqual(runtime.configuration_digest(self.backend, self.configuration), added)

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
        for values in ({"APP_ENV": "dev"},):
            with self.subTest(values=values), patch.dict(os.environ, values):
                self.assert_rejected_without_replacing(original)
        self.assertEqual(runtime.verify_runtime(self.backend, self.directory), json.loads(original))

    def test_invalid_environment_and_worker_address_never_publish_receipt(self):
        with patch.dict(os.environ, {"APP_ENV": "prod"}), self.assertRaises(ValueError):
            runtime.register_runtime(self.backend, self.directory)
        original = self.config.read_text(encoding="utf-8")
        cases = [('scope_id="runtime-fixture"', 'scope_id="../scope"'),
                 ('mode="external"', 'mode="inline"'),
                 ('health_host="127.0.0.1"', 'health_host="192.0.2.1"'),
                 ('health_port=19210', 'health_port=0'),
                 ('health_port=19210', 'health_port=65536'),
                 ('snowflake_worker_id=3', 'snowflake_worker_id=2')]
        for old, new in cases:
            with self.subTest(new=new):
                self.config.write_text(original.replace(old, new), encoding="utf-8")
                with self.assertRaises(ValueError):
                    runtime.register_runtime(self.backend, self.directory)
                self.assertFalse(self.receipt.exists())
        self.config.write_text(original, encoding="utf-8")

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

    def test_managed_directory_and_both_file_digests_are_bound(self):
        registered = runtime.register_runtime(self.backend, self.directory)
        self.assertEqual(registered["api_config_dir"], str(self.configuration))
        self.assertEqual(registered["worker_config_dir"], str(self.worker_configuration))
        self.assertNotEqual(registered["api_configuration_sha256"], registered["worker_configuration_sha256"])
        original = self.receipt.read_bytes()
        path = self.worker_configuration / "app.test.toml"
        contents = path.read_bytes()
        path.write_bytes(contents + b"\n# changed\n")
        self.assert_rejected_without_replacing(original)
        path.write_bytes(contents)
        moved = self.directory / "worker-config-moved"
        self.worker_configuration.rename(moved)
        with self.assertRaisesRegex(ValueError, "目录"):
            runtime.verify_runtime(self.backend, self.directory)
        moved.rename(self.worker_configuration)
        tampered = {**registered, "worker_config_dir": str(moved)}
        self.receipt.write_text(json.dumps(tampered), encoding="utf-8")
        self.assert_rejected_without_replacing(self.receipt.read_bytes())

    def test_legacy_environment_cannot_override_bound_configuration_or_process(self):
        original = runtime.runtime_contract(self.backend, self.directory)
        with patch.dict(os.environ, {"APP_SCOPE_ID": "foreign", "APP_JOBS_HEALTH_PORT": "1",
                                    "APP_CONFIG_DIR": "missing", "SNOWFLAKE_WORKER_ID": "3",
                                    "RYFRAME_PRIVATE": "secret", "VITE_APP_TARGET": "foreign",
                                    "UNDECLARED_TOOL_SECRET": "secret", "AWS_SECRET_ACCESS_KEY": "secret"}):
            self.assertEqual(runtime.runtime_contract(self.backend, self.directory), original)
            child = runtime.process_environment()
            self.assertEqual({key for key in child if key.startswith("APP_")}, {"APP_ENV"})
            self.assertNotIn("SNOWFLAKE_WORKER_ID", child)
            self.assertNotIn("RYFRAME_PRIVATE", child)
            self.assertNotIn("VITE_APP_TARGET", child)
            self.assertNotIn("UNDECLARED_TOOL_SECRET", child)
            self.assertNotIn("AWS_SECRET_ACCESS_KEY", child)
            self.assertEqual(child.get("PATH"), os.environ.get("PATH"))


if __name__ == "__main__":
    unittest.main()
