"""监控投递在启动进程前必须绑定唯一正式运行来源、工具和秘密文件。"""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import restore_monitoring_authority as authority_module
import restore_monitoring_delivery as delivery
import restore_monitoring_evidence as evidence
import restore_monitoring_rules as rules
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


class FakeDocument:
    def __init__(self, path: Path, value: dict, sha: str):
        self.path = path
        self.value = value
        self.raw = b"{}"
        self.sha256 = sha
        self.unchanged = 0

    def assert_unchanged(self):
        self.unchanged += 1


class MonitoringAuthorityTests(unittest.TestCase):
    def test_preflight_uses_static_authority_and_live_generation(self):
        backend = Path("C:/coordinator")
        runtime_path = Path("C:/evidence/runtime.json")
        target_path = Path("C:/evidence/target.json")
        launch_path = Path("C:/evidence/runtime-launch.json")
        runtime = {
            "paths": {
                "launch": str(launch_path),
                "bindings": "C:/evidence/bindings.json",
            },
            "digests": {"launch": "c" * 64},
            "restore": {
                "id": "restore-one",
                "backup_id": "backup-one",
                "plan_hash": "d" * 64,
                "scope_id": "restore-scope",
                "data_verified_at": "2026-09-12T08:00:00Z",
            },
            "source": {
                "backup_source_sha": "1" * 40,
                "backend_product_sha": "2" * 40,
                "backend_execution_sha": "3" * 40,
                "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
                "frontend_sha": "4" * 40,
            },
            "endpoints": {
                "api": "http://127.0.0.1:18080/readyz",
                "worker": "http://127.0.0.1:19091/readyz",
                "frontend": "http://127.0.0.1:14174",
            },
            "processes": {
                role: {"identity": {"pid": index + 10, "started": str(index + 20), "executable": f"C:/{role}.exe"}}
                for index, role in enumerate(("api", "worker", "frontend"))
            },
        }
        launch_request = {
            name: {}
            for name in (
                "authority",
                "registration",
                "roots",
                "paths",
                "digests",
                "artifacts",
                "tools",
            )
        }
        launch_request.update(
            {
                "format_version": 1,
                "kind": "restore-runtime-launch-request",
                "environment": {
                    "document": {"path": "C:/evidence/environment.json", "bytes": 2, "sha256": "e" * 64},
                    "variables": [],
                    "sha256": "f" * 64,
                },
            }
        )
        runtime_document = FakeDocument(runtime_path, runtime, "a" * 64)
        launch_document = FakeDocument(launch_path, {"request": launch_request}, "c" * 64)
        target_document = FakeDocument(target_path, {}, "b" * 64)
        target = {
            "maintenance_execution": {
                "root": "C:/maintenance",
                "binding": {"kind": "current-backend", "path": "C:/maintenance"},
                "build": {"path": "C:/evidence/build.json", "bytes": 2, "sha256": "8" * 64},
            },
            "product_execution": {
                "roots": {
                    "source_backend": "C:/product",
                    "execution_backend": "C:/execution",
                    "frontend": "C:/frontend",
                }
            },
        }
        runtime_authority = {"kind": "restore-runtime-authority"}
        dataset = {
            "source_generation": {"path": "C:/evidence/stop.json", "bytes": 2, "sha256": "9" * 64},
            "dataset_lineage": {"path": "C:/evidence/lineage.json", "bytes": 2, "sha256": "0" * 64},
        }
        descriptors = {
            runtime_path: {"path": str(runtime_path), "bytes": 2, "sha256": "a" * 64},
            target_path: {"path": str(target_path), "bytes": 2, "sha256": "b" * 64},
            launch_path: {"path": str(launch_path), "bytes": 2, "sha256": "c" * 64},
        }
        live = Mock(return_value={"runtime_receipt_sha256": "a" * 64})
        with (
            patch.object(authority_module, "repository", return_value=backend),
            patch.object(authority_module, "read_json_document", side_effect=[runtime_document, launch_document]),
            patch.object(authority_module, "validate_runtime_receipt", return_value=runtime),
            patch.object(
                authority_module,
                "_verified_target",
                return_value=(target_document, target, {"target": {}}, {}, dataset),
            ),
            patch.object(authority_module, "verify_static_runtime", return_value=runtime_authority),
            patch.object(authority_module, "verify_live_generation", live),
            patch.object(authority_module, "_descriptor", side_effect=lambda document: descriptors[document.path]),
        ):
            result = authority_module.monitoring_preflight(backend, runtime_path, target_path)
        self.assertEqual(result["runtime"], descriptors[runtime_path])
        self.assertEqual(result["target_plan"], descriptors[target_path])
        self.assertEqual(result["launch"], descriptors[launch_path])
        self.assertEqual(result["source_generation"], dataset["source_generation"])
        self.assertEqual(result["endpoints"]["api_metrics"], "http://127.0.0.1:18080/api/v1/monitor/metrics")
        self.assertEqual(result["endpoints"]["worker_metrics"], "http://127.0.0.1:19091/metrics")
        self.assertEqual(live.call_args.args[1:4], (backend, Path("C:/execution"), Path("C:/frontend")))
        self.assertEqual(live.call_args.kwargs["product_backend"], Path("C:/product"))
        self.assertGreaterEqual(runtime_document.unchanged, 1)
        self.assertGreaterEqual(target_document.unchanged, 1)
        self.assertGreaterEqual(launch_document.unchanged, 1)


class MonitoringBindingTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        self.directory = WorkspaceDirectory(local)
        self.addCleanup(self.directory.cleanup)
        self.backend = Path(self.directory.name).resolve()
        self.enterContext(patch.dict(os.environ, {"RYFRAME_PYTHON": str(Path(sys.executable).resolve())}))
        (self.backend / ".local-tests/run").mkdir(parents=True)
        (self.backend / ".local-tests/tools").mkdir()
        (self.backend / "deploy/prometheus").mkdir(parents=True)
        self.token = "private-monitoring-token"
        self.credential = self.backend / ".local-tests/metrics-token.txt"
        self.credential.write_text(self.token + "\n", encoding="utf-8")
        self.environment = self._json(
            self.backend / ".local-tests/environment.json",
            {"environment": {"APP_SCOPE_ID": "restore-scope", "APP_MONITOR_METRICS_BEARER_TOKEN": self.token}},
        )
        self.files = {}
        for name in ("runtime", "target", "launch", "source-generation", "lineage", "maintenance", "build", "backend-build", "frontend-build"):
            self.files[name] = self._json(self.backend / ".local-tests" / f"{name}.json", {"name": name})
        self.tools = {}
        for name in evidence.TOOL_VERSIONS:
            path = self.backend / ".local-tests/tools" / (name + ".exe")
            path.write_bytes((name + "-binary").encode())
            self.tools[name] = path
        self.rule_heads = {}
        for relative in rules.RULE_PATHS.values():
            source = ROOT / relative
            target = self.backend / relative
            shutil.copyfile(source, target)
            self.rule_heads[relative] = source.read_bytes()
        self.coordinator = {"root": str(self.backend), "head": "a" * 40, "inventory_sha256": "b" * 64}
        self.authority = self._authority()

    @staticmethod
    def _json(path: Path, value: dict) -> Path:
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def _authority(self) -> dict:
        source = {
            "backup_source_sha": "1" * 40,
            "backend_product_sha": "2" * 40,
            "backend_execution_sha": "2" * 40,
            "backend_adapter_contract": None,
            "frontend_sha": "3" * 40,
        }
        return {
            "format_version": 1,
            "kind": "restore-monitoring-authority",
            "runtime": evidence.descriptor(self.files["runtime"]),
            "target_plan": evidence.descriptor(self.files["target"]),
            "launch": evidence.descriptor(self.files["launch"]),
            "environment": evidence.descriptor(self.environment),
            "restore": {
                "id": "restore-one",
                "backup_id": "backup-one",
                "plan_hash": "4" * 64,
                "scope_id": "restore-scope",
                "data_verified_at": "2026-09-12T08:00:00Z",
            },
            "source": source,
            "endpoints": {
                "api_ready": "http://127.0.0.1:18080/readyz",
                "worker_ready": "http://127.0.0.1:19091/readyz",
                "frontend": "http://127.0.0.1:14174",
                "api_metrics": "http://127.0.0.1:18080/api/v1/monitor/metrics",
                "worker_metrics": "http://127.0.0.1:19091/metrics",
            },
            "maintenance_execution": {
                "root": str(self.backend),
                "binding": {"kind": "current-backend", "path": str(self.backend)},
                "build": evidence.descriptor(self.files["build"]),
            },
            "product_execution": {
                "roots": {"source_backend": str(self.backend), "execution_backend": str(self.backend), "frontend": str(self.backend)},
                "backend_product_sha": source["backend_product_sha"],
                "backend_execution_sha": source["backend_execution_sha"],
                "frontend_sha": source["frontend_sha"],
                "builds": {
                    "backend": evidence.descriptor(self.files["backend-build"]),
                    "frontend": evidence.descriptor(self.files["frontend-build"]),
                },
                "adapter": None,
            },
            "processes": {
                role: {"pid": index + 10, "started": str(index + 100), "executable": str(Path(sys.executable))}
                for index, role in enumerate(("api", "worker", "frontend"))
            },
            "source_generation": evidence.descriptor(self.files["source-generation"]),
            "dataset_lineage": evidence.descriptor(self.files["lineage"]),
        }

    @staticmethod
    def _run(command, **_kwargs):
        if "check_python_environment.py" in str(command[-1]):
            return subprocess.CompletedProcess(command, 0, stdout=b"Python environment check passed\n")
        name = Path(command[0]).stem
        return subprocess.CompletedProcess(command, 0, stdout=(evidence.TOOL_VERSIONS[name] + ", test\n").encode())

    @staticmethod
    def _write(path, value, _root):
        path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    def _git(self, _root, *arguments):
        if arguments[:3] == ("ls-files", "--error-unmatch", "--"):
            return (arguments[3] + "\n").encode()
        if arguments[0] == "show":
            return self.rule_heads[arguments[1].removeprefix("HEAD:")]
        raise AssertionError(f"未登记的 Git 调用：{arguments}")

    def _bind(self):
        output = self.backend / ".local-tests/run/binding.json"
        patches = (
            patch.object(delivery, "repository", side_effect=lambda path, _label: path),
            patch.object(delivery, "validate_new_output", side_effect=lambda path, _root: path),
            patch.object(delivery, "write_new", side_effect=self._write),
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            patch.object(evidence, "coordinator_binding", return_value=self.coordinator),
            patch.object(rules, "git", side_effect=self._git),
        )
        with patches[0], patches[1], patches[2], patches[3], patches[4], patches[5]:
            result = delivery.bind(
                self.backend,
                self.files["runtime"],
                self.files["target"],
                output,
                "r24-monitoring",
                self.credential,
                self.tools,
                {"prometheus": 29090, "alertmanager": 29093, "webhook": 29094},
                preflight=lambda *_args: copy.deepcopy(self.authority),
                run=self._run,
                port_check=lambda _url: None,
            )
        return output, result

    def test_bind_publishes_exact_inputs_without_secret_or_remote_contact(self):
        output, result = self._bind()
        observed = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(observed, result)
        self.assertEqual(evidence.validate_binding(observed), observed)
        self.assertNotIn(self.token, output.read_text(encoding="utf-8"))
        self.assertEqual(observed["credential"], {"path": str(self.credential), "bytes": self.credential.stat().st_size})
        self.assertEqual(observed["contact_policy"]["external_receivers"], [])
        self.assertEqual(observed["authority"]["source_generation"], self.authority["source_generation"])
        self.assertEqual(set(output.parent.iterdir()), {output})
        self.assertEqual(
            {name: item["path"] for name, item in observed["rules"].items()},
            rules.expected_paths(self.backend),
        )

    def test_rules_must_be_tracked_head_files_with_all_fixed_alerts(self):
        alerts = self.backend / rules.RULE_PATHS["alerts"]
        original_alerts = alerts.read_bytes()
        with (
            patch.object(rules, "git", side_effect=subprocess.CalledProcessError(1, ["git"])),
            self.assertRaisesRegex(ValueError, "HEAD 跟踪"),
        ):
            rules.bind_rules(self.backend)
        changed = alerts.read_text(encoding="utf-8").replace(
            "alert: RyFrameBackupAging", "alert: RyFrameBackupAgingChanged", 1
        )
        alerts.write_text(changed, encoding="utf-8")
        output = self.backend / ".local-tests/run/binding.json"
        with self.assertRaisesRegex(ValueError, "HEAD 跟踪内容不同"):
            self._bind()
        self.assertFalse(output.exists())

        self.rule_heads[rules.RULE_PATHS["alerts"]] = alerts.read_bytes()
        with self.assertRaisesRegex(ValueError, "固定的 8 个"):
            self._bind()
        self.assertFalse(output.exists())

        alerts.write_bytes(original_alerts)
        self.rule_heads[rules.RULE_PATHS["alerts"]] = original_alerts
        tests = self.backend / rules.RULE_PATHS["tests"]
        changed_tests = tests.read_text(encoding="utf-8").replace(
            "alertname: RyFrameRestoreOverdue", "alertname: RyFrameRestoreOverdueChanged"
        )
        tests.write_text(changed_tests, encoding="utf-8")
        self.rule_heads[rules.RULE_PATHS["tests"]] = tests.read_bytes()
        with self.assertRaisesRegex(ValueError, "精确覆盖固定的 8 个"):
            self._bind()
        self.assertFalse(output.exists())

    def test_binding_rejects_noncanonical_run_id_and_rule_path(self):
        for value in ("", "1-monitor", "Monitor", "monitor-", "monitor--run", "monitor_run", "a" * 65):
            with self.subTest(value=value), self.assertRaises(ValueError):
                evidence.canonical_run_id(value)
        self.assertEqual(evidence.canonical_run_id("r24-monitoring"), "r24-monitoring")
        _output, binding = self._bind()
        changed = copy.deepcopy(binding)
        changed["run_id"] = "monitor--run"
        with self.assertRaisesRegex(ValueError, "run-id"):
            evidence.validate_binding(changed)
        changed = copy.deepcopy(binding)
        changed["rules"]["alerts"]["path"] = str(self.backend / ".local-tests/alerts.yml")
        with self.assertRaisesRegex(ValueError, "固定的正式文件"):
            evidence.validate_binding(changed)

    def test_failed_final_verification_never_publishes_binding(self):
        output = self.backend / ".local-tests/run/binding.json"
        with (
            patch.object(delivery, "verify_binding_inputs", side_effect=ValueError("最终复核失败")),
            self.assertRaisesRegex(ValueError, "最终复核失败"),
        ):
            self._bind()
        self.assertFalse(output.exists())
        self.assertEqual(list(output.parent.iterdir()), [])

    def test_unknown_sibling_during_bind_or_read_fails_closed(self):
        output = self.backend / ".local-tests/run/binding.json"
        marker = output.parent / "unknown.txt"

        def inject_unknown(*_args, **_kwargs):
            marker.write_text("unknown", encoding="utf-8")
            return _args[1], ()

        with (
            patch.object(delivery, "verify_binding_inputs", side_effect=inject_unknown),
            self.assertRaisesRegex(ValueError, "未知文件"),
        ):
            self._bind()
        self.assertFalse(output.exists())
        marker.unlink()

        output, _binding = self._bind()
        marker.write_text("unknown", encoding="utf-8")
        with (
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            self.assertRaisesRegex(ValueError, "未知文件"),
        ):
            evidence.read_binding(self.backend, output)

    def test_binding_rejects_secret_tool_authority_and_unknown_fields_drift(self):
        output, binding = self._bind()
        self.credential.write_text("x" * len(self.token) + "\n", encoding="utf-8")
        with (
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            patch.object(evidence, "coordinator_binding", return_value=self.coordinator),
            self.assertRaisesRegex(ValueError, "凭据"),
        ):
            evidence.verify_binding_inputs(self.backend, binding, lambda *_args: self.authority, self._run)
        self.credential.write_text(self.token + "\n", encoding="utf-8")
        self.tools["prometheus"].write_bytes(b"changed-binary")
        with (
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            patch.object(evidence, "coordinator_binding", return_value=self.coordinator),
            self.assertRaises(ValueError),
        ):
            evidence.verify_binding_inputs(self.backend, binding, lambda *_args: self.authority, self._run)
        changed = copy.deepcopy(binding)
        changed["extra"] = True
        with self.assertRaises(ValueError):
            evidence.validate_binding(changed)
        changed = copy.deepcopy(binding)
        changed["authority"]["source"]["frontend_sha"] = "9" * 40
        with self.assertRaisesRegex(ValueError, "执行来源"):
            evidence.validate_binding(changed)
        self.assertTrue(output.is_file())

    def test_bind_rejects_product_port_and_nonempty_or_replayed_run(self):
        output = self.backend / ".local-tests/run/binding.json"
        common = (
            self.backend,
            self.files["runtime"],
            self.files["target"],
            output,
            "r24-monitoring",
            self.credential,
            self.tools,
        )
        with (
            patch.object(delivery, "repository", side_effect=lambda path, _label: path),
            patch.object(delivery, "validate_new_output", side_effect=lambda path, _root: path),
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            patch.object(evidence, "coordinator_binding", return_value=self.coordinator),
            self.assertRaisesRegex(ValueError, "产品端口"),
        ):
            delivery.bind(
                *common,
                {"prometheus": 18080, "alertmanager": 29093, "webhook": 29094},
                preflight=lambda *_args: self.authority,
                run=self._run,
                port_check=lambda _url: None,
            )
        marker = output.parent / "unknown.txt"
        marker.write_text("unknown", encoding="utf-8")
        with (
            patch.object(delivery, "repository", side_effect=lambda path, _label: path),
            patch.object(delivery, "validate_new_output", side_effect=lambda path, _root: path),
            self.assertRaisesRegex(ValueError, "空 run"),
        ):
            delivery.bind(
                *common,
                {"prometheus": 29090, "alertmanager": 29093, "webhook": 29094},
                preflight=lambda *_args: self.authority,
                run=self._run,
                port_check=lambda _url: None,
            )

    def test_duplicate_cli_option_is_argument_error_before_preflight(self):
        with self.assertRaises(SystemExit) as caught:
            delivery.main(["bind", "--backend-dir", str(self.backend), "--backend-dir", str(self.backend)])
        self.assertEqual(caught.exception.code, 2)

    def test_python_must_be_the_explicit_fixed_interpreter(self):
        with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(ValueError, "RYFRAME_PYTHON"):
            evidence.python_binding(self.backend, self._run)
        with patch.dict(os.environ, {"RYFRAME_PYTHON": str(self.credential)}), self.assertRaisesRegex(
            ValueError, "指定的解释器"
        ):
            evidence.python_binding(self.backend, self._run)


if __name__ == "__main__":
    unittest.main()
