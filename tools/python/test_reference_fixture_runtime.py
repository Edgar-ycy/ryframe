"""业务 crate 夹具运行时只能使用已绑定的私有环境与构建来源。"""
from __future__ import annotations

from rate_limit_tool import tool_binding
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import reference_fixture_runtime as runtime
import reference_fixture_environment as environment
from devex_clone_capture import write_json
from workspace_directory import WorkspaceDirectory


class ReferenceFixtureRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests", prefix="reference-runtime-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.execution = self.root / "business"
        self.reference = self.execution / ".local-tests/reference-fixture"
        self.reference.mkdir(parents=True)
        (self.execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        self.environment = self.root / "environment"
        self.environment.mkdir()
        self.bootstrap = self.environment / "bootstrap.json"
        private = {"APP_ENV": "test", "TEMP": str(self.root / "tmp"), "TMP": str(self.root / "tmp")}
        config = self.environment / "config"
        config.mkdir()
        (config / "app.test.toml").write_text('scope_id = "fixture-source"\n[app]\nhost = "127.0.0.1"\nport = 18200\n[jobs]\nmode = "external"\nhealth_host = "127.0.0.1"\nhealth_port = 19200\n', encoding="utf-8")
        secrets = self.reference / "secrets"
        secrets.mkdir()
        for name in environment.SECRET_FILES:
            (secrets / name).write_text("secret-" + name, encoding="utf-8")
        write_json(self.bootstrap, {"format_version": 1, "kind": "reference-fixture-environment",
                                    "status": "prepared", "services_started": False, "remote_writes": 0,
                                    "historical_data_used": False, "execution_backend": str(self.execution),
                                    "environment_sha256": environment.plan_hash(private),
                                    "secret_files": {name: {"path": str(secrets / name),
                                                            **runtime.file_digest(secrets / name)}
                                                     for name in environment.SECRET_FILES}})
        write_json(self.environment / "environment.json", {"environment": private})
    def _pair(self, _execution, output):
        value = {"format_version": 1, "kind": "reference-fixture-source-pair",
                 "fixture_receipt": {"path": str(self.root / "fixture.json"), "bytes": 1, "sha256": "a" * 64},
                 "sources": {name: {"head": "b" * 40, "patch_sha256": "c" * 64, "files": []}
                             for name in ("backend", "frontend")}}
        write_json(output, value)
        return value

    def test_output_rejects_paths_outside_the_business_reference_root(self):
        with self.assertRaisesRegex(ValueError, "参考夹具根"):
            runtime._output(self.execution, self.root / "outside", new=True)

    def test_run_controls_worker_through_bound_native_protocol(self):
        output = self.reference / "runtime-native"
        output.mkdir()
        write_json(output / "runtime.json", {"scope_id": "fixture-source"})
        self._pair(self.execution, output / "source-pair.json")
        private = {"APP_ENV": "test", "TEMP": str(self.root), "TMP": str(self.root)}
        with patch.object(runtime, "verify", return_value={"runtime": {}}), \
                patch.object(runtime, "_bootstrap", return_value=(self.bootstrap, self.execution, private)), \
                patch.object(runtime, "_output", return_value=output), \
                patch("full_stack_worker_native.invoke", return_value={"operation": "stop", "scope_id": "fixture-source", "state": "stopped"}) as worker, \
                patch.object(runtime, "control", return_value={"processes": {"api": {"state": "stopped"}}}) as control:
            result = runtime.run(self.backend, self.bootstrap, output, "stop")
        self.assertEqual(result["processes"], {"api": "stopped", "worker": "stopped"})
        self.assertEqual(control.call_args.args[3], ("api",))
        worker.assert_called_once_with("control", action="stop", backend=str(self.execution), directory=str(output), timeout=60)

    def test_verify_returns_build_checked_inside_registered_environment(self):
        output = self.reference / "runtime-verify"
        output.mkdir()
        self._pair(self.execution, output / "source-pair.json")
        write_json(output / "backend-build.json", {"artifacts": {}})
        write_json(output / "runtime.json", {"scope_id": "fixture-source"})
        evidence = {"source": {"fixture": "business", "head": "b" * 40}}
        original = dict(os.environ)
        def check(_execution, _output):
            self.assertEqual(os.environ["APP_ENV"], "test")
            self.assertNotIn("APP_SCOPE_ID", os.environ)
            self.assertNotIn("RYFRAME_CODE_SHA", os.environ)
            return evidence
        with patch.object(runtime, "verify_build_artifacts"), \
                patch.object(runtime, "verify_build_evidence", side_effect=check) as verify, \
                patch.object(runtime, "verify_runtime", return_value={"scope_id": "fixture-source"}):
            result = runtime.verify(self.backend, self.bootstrap, output)
        self.assertEqual(verify.call_count, 1)
        self.assertIs(result["build_evidence"], evidence)
        self.assertEqual(dict(os.environ), original)

    def test_bootstrap_rejects_environment_or_secret_drift(self):
        original = (self.environment / "environment.json").read_bytes()
        (self.environment / "environment.json").unlink()
        write_json(self.environment / "environment.json", {"environment": {"APP_ENV": "other"}})
        with self.assertRaisesRegex(ValueError, "环境.*摘要"):
            runtime._bootstrap(self.backend, self.bootstrap)
        (self.environment / "environment.json").unlink()
        (self.environment / "environment.json").write_bytes(original)

        secret = self.reference / "secrets/rustfs-access-key.txt"
        secret.write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "秘密文件"):
            runtime._bootstrap(self.backend, self.bootstrap)

    def test_browser_close_selects_the_closed_typed_consumer(self):
        result = {"status": "reference_fixture_browser_closed"}
        request = runtime.RuntimeRequest(
            operation="browser-close", backend_dir=self.backend,
            environment=self.bootstrap, output=self.reference / "runtime-r1", write=False,
            browser_binding=self.reference / "runtime-r1/binding.json",
            native_tool=tool_binding())
        with patch.object(runtime, "_native_browser", return_value=result) as verify:
            self.assertEqual(runtime.execute(request), result)
        self.assertEqual(verify.call_args.args[0].operation, "browser-close")
        self.assertFalse(verify.call_args.args[0].write)

    def test_browser_private_protocol_requires_only_a_non_secret_loopback_endpoint(self):
        binding = self.reference / "runtime-r1/binding.json"
        request = {
            "native_tool": tool_binding(), "backend_dir": str(self.backend),
            "environment": str(self.bootstrap),
            "format_version": 1,
            "operation": "browser",
            "output": str(self.reference / "runtime-r1"),
            "write": True,
            "browser_binding": str(binding),
            "credential_endpoint": "http://127.0.0.1:49152/v1/credentials",
        }
        parsed = runtime._request_from_protocol(request)
        self.assertEqual(parsed.credential_endpoint, request["credential_endpoint"])
        self.assertNotIn("password", json.dumps(request).lower())
        for value in (
                "http://example.test:49152/v1/credentials",
                "http://127.0.0.1:49152/other",
                "http://127.0.0.1:49152/v1/credentials\nprivate"):
            invalid = {**request, "credential_endpoint": value}
            with self.assertRaises(runtime.RuntimeProtocolError):
                runtime._request_from_protocol(invalid)
        missing = dict(request)
        missing.pop("credential_endpoint")
        with self.assertRaises(runtime.RuntimeProtocolError):
            runtime._request_from_protocol(missing)

    def test_browser_bridge_passes_exact_file_protocol_without_environment_overrides(self):
        binding = self.reference / "runtime-r1/browser-binding-r1.json"
        request = runtime.RuntimeRequest(
            operation="browser", backend_dir=self.backend, environment=self.bootstrap,
            output=binding.parent, write=True, browser_binding=binding,
            credential_endpoint="http://127.0.0.1:49152/v1/credentials",
            native_tool=tool_binding())
        with patch("fixture_service_client._request", return_value={"value": {"status": "passed"}}) as invoke:
            self.assertEqual(runtime._native_browser(request), {"status": "passed"})
        marker, payload, timeout, error_code = invoke.call_args.args
        self.assertEqual(marker, "__fixture-browser")
        self.assertEqual(timeout, 6000)
        self.assertEqual(error_code, "invalid_fixture_browser")
        value = json.loads(payload)
        self.assertEqual(set(value), {"operation", "arguments", "write"})
        self.assertTrue(value["write"])
        self.assertEqual(value["arguments"]["browser_binding"], str(binding))
        self.assertNotIn("run_id", value["arguments"])
        self.assertNotIn("server", value["arguments"])
