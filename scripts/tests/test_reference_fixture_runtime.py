"""Device 夹具运行时只能使用已绑定的私有环境与构建来源。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_runtime as runtime
from devex_clone_capture import write_json
from workspace_directory import WorkspaceDirectory


class ReferenceFixtureRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests", prefix="reference-runtime-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.execution = self.root / "device"
        self.reference = self.execution / ".local-tests/reference-fixture"
        self.reference.mkdir(parents=True)
        (self.execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        self.environment = self.root / "environment"
        self.environment.mkdir()
        self.bootstrap = self.environment / "bootstrap.json"
        write_json(self.bootstrap, {"format_version": 1, "kind": "reference-fixture-environment",
                                    "status": "prepared", "services_started": False, "remote_writes": 0,
                                    "execution_backend": str(self.execution)})
        write_json(self.environment / "environment.json", {"environment": {
            "APP_ENV": "test", "APP_SCOPE_ID": "fixture-source", "APP_CONFIG_DIR": str(self.execution),
            "APP_APP_HOST": "127.0.0.1", "APP_APP_PORT": "18200", "APP_JOBS_MODE": "external",
            "APP_JOBS_HEALTH_HOST": "127.0.0.1", "APP_JOBS_HEALTH_PORT": "19200",
            "TEMP": str(self.root / "tmp"), "TMP": str(self.root / "tmp"),
        }})
        self.binaries = {}
        for name in ("ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate"):
            path = self.root / (name + ".exe")
            path.write_bytes(name.encode())
            self.binaries[name] = str(path)

    def _pair(self, _execution, output):
        value = {"format_version": 1, "kind": "reference-fixture-source-pair",
                 "fixture_receipt": {"path": str(self.root / "fixture.json"), "bytes": 1, "sha256": "a" * 64},
                 "sources": {name: {"head": "b" * 40, "patch_sha256": "c" * 64, "files": []}
                             for name in ("backend", "frontend")}}
        write_json(output, value)
        return value

    def test_build_binds_device_source_before_writing_runtime_receipts(self):
        captured = {}
        output = self.reference / "runtime-r1"

        def build_binaries(_run, execution, directory):
            captured["environment"] = dict(os.environ)
            self.assertEqual(execution, self.execution)
            self.assertEqual(directory, output)
            return self.binaries

        def register_runtime(_execution, directory):
            value = {"scope_id": "fixture-source"}
            write_json(directory / "runtime.json", value)
            return value

        source = {"head": "b" * 40, "patch_sha256": "d" * 64, "files": [], "clean": False}
        inventory = {"source": {"snapshot": source, "worktree_fingerprint": "sha256:" + "e" * 64},
                     "files": [], "guard": {"head": source["head"], "index_sha256": "f" * 64,
                                             "modes_sha256": "1" * 64}}
        context = {"commands": {role: runtime.build_command(role) for role in ("api", "worker")},
                   "profile": "dev", "target": "x86_64-pc-windows-msvc", "jobs": "cargo-default",
                   "toolchain": {"cargo": "cargo fixture", "rustc": "rustc fixture"},
                   "environment": {"variables": [], "sha256": runtime.build_source_domains(
                       inventory, "backend")["tools"]["sha256"]}}
        with patch.object(runtime, "write_pair", side_effect=self._pair), \
                patch.object(runtime, "source_snapshot", return_value=source) as source_probe, \
                patch.object(runtime, "capture_inventory", return_value=inventory) as inventory_probe, \
                patch.object(runtime, "build_context", return_value=context) as context_probe, \
                patch.object(runtime, "build_binaries", side_effect=build_binaries), \
                patch.object(runtime, "register_runtime", side_effect=register_runtime):
            result = runtime.build(self.backend, self.bootstrap, output)

        self.assertEqual(result["status"], "reference_fixture_runtime_built")
        self.assertEqual(captured["environment"]["APP_API_DOCS_ENABLED"], "false")
        self.assertEqual(captured["environment"]["RYFRAME_E2E_FIXTURE"], "device")
        self.assertEqual(captured["environment"]["RYFRAME_CODE_SHA"], "b" * 40)
        self.assertEqual(source_probe.call_count, 1)
        self.assertEqual(inventory_probe.call_args_list[0].args, (self.execution, source))
        self.assertGreaterEqual(inventory_probe.call_count, 2)
        self.assertGreaterEqual(context_probe.call_count, 2)
        build = json.loads((output / "backend-build.json").read_text(encoding="utf-8"))
        self.assertEqual(set(build["artifacts"]), {"api", "worker"})
        self.assertTrue((output / "source-pair.json").is_file())

    def test_output_rejects_paths_outside_the_device_reference_root(self):
        with self.assertRaisesRegex(ValueError, "参考夹具根"):
            runtime._output(self.execution, self.root / "outside", new=True)

    def test_browser_cli_rejects_missing_write_before_reading_inputs(self):
        missing = self.root / "missing"
        with patch.object(sys, "argv", ["reference_fixture_runtime.py", "bind",
                                        "--backend-dir", str(missing), "--environment", str(missing),
                                        "--output", str(missing), "--browser-binding", str(missing),
                                        "--run-id", "r24-device"]):
            with self.assertRaises(SystemExit) as raised:
                runtime.main()
        self.assertEqual(raised.exception.code, 2)
