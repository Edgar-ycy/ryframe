"""监控投递在启动进程前必须绑定唯一正式运行来源、工具和秘密文件。"""

from __future__ import annotations

import copy
from contextlib import nullcontext
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
import restore_monitoring_permissions as permissions
import restore_monitoring_staging as staging
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
        self.run_directory = self.backend / ".local-tests/run"
        self.run_directory.mkdir(parents=True)
        (self.backend / ".local-tests/tools").mkdir()
        (self.backend / "deploy/prometheus").mkdir(parents=True)
        self.token = "private-monitoring-token"
        self.credential = self.run_directory / staging.TOKEN_NAME
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
        self.staged_sources = (
            (staging.ENVIRONMENT_CHECK, b"# environment check\n"),
            (staging.REQUIREMENTS, b"fixture==1\n"),
            (staging.RUNNER, b"# monitoring runner\n"),
        )
        self.acl = {
            "platform": "windows" if os.name == "nt" else "posix",
            "owner": "fixture",
        }
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
        if name == "python":
            return subprocess.CompletedProcess(
                command, 0, stdout=("Python " + sys.version.split()[0] + "\n").encode()
            )
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
            patch.object(staging, "_committed_files", return_value=self.staged_sources),
            patch.object(staging, "protect_binaries", side_effect=lambda *_args: nullcontext()),
        )
        with (
            patches[0], patches[1], patches[2], patches[3], patches[4], patches[5],
            patches[6], patches[7],
        ):
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
                acl_reader=lambda _path: self.acl,
            )
        return output, result

    def test_bind_publishes_exact_inputs_without_secret_or_remote_contact(self):
        output, result = self._bind()
        observed = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(observed, result)
        self.assertEqual(evidence.validate_binding(observed), observed)
        self.assertNotIn(self.token, output.read_text(encoding="utf-8"))
        self.assertEqual(observed["credential"]["path"], str(self.credential))
        self.assertEqual(observed["credential"]["bytes"], self.credential.stat().st_size)
        self.assertRegex(observed["credential"]["sha256"], r"^[a-f0-9]{64}$")
        self.assertEqual(observed["contact_policy"]["external_receivers"], [])
        self.assertEqual(observed["authority"]["source_generation"], self.authority["source_generation"])
        self.assertEqual(
            {item.name for item in output.parent.iterdir()},
            {output.name, staging.TOKEN_NAME, staging.DIRECTORY},
        )
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
        self.assertEqual(
            {item.name for item in output.parent.iterdir()},
            {staging.TOKEN_NAME, staging.DIRECTORY},
        )

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
        shutil.rmtree(output.parent / staging.DIRECTORY)

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
            evidence.verify_binding_inputs(
                self.backend,
                binding,
                lambda *_args: self.authority,
                self._run,
                acl_reader=lambda _path: self.acl,
            )
        self.credential.write_text(self.token + "\n", encoding="utf-8")
        staged_tool = Path(binding["tools"]["prometheus"]["path"])
        staged_tool.write_bytes(b"changed-binary")
        with (
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            patch.object(evidence, "coordinator_binding", return_value=self.coordinator),
            self.assertRaises(ValueError),
        ):
            evidence.verify_binding_inputs(
                self.backend,
                binding,
                lambda *_args: self.authority,
                self._run,
                acl_reader=lambda _path: self.acl,
            )
        changed = copy.deepcopy(binding)
        changed["extra"] = True
        with self.assertRaises(ValueError):
            evidence.validate_binding(changed)
        changed = copy.deepcopy(binding)
        changed["authority"]["source"]["frontend_sha"] = "9" * 40
        with self.assertRaisesRegex(ValueError, "执行来源"):
            evidence.validate_binding(changed)
        self.assertTrue(output.is_file())

    def test_verification_executes_only_staged_tools_and_ignores_changed_sources(self):
        _output, binding = self._bind()
        for source in self.tools.values():
            source.write_bytes(b"source-changed-after-bind")
        commands = []

        def observed_run(command, **kwargs):
            commands.append(Path(command[0]).resolve())
            return self._run(command, **kwargs)

        with (
            patch.object(evidence, "repository", side_effect=lambda path, _label: path),
            patch.object(evidence, "coordinator_binding", return_value=self.coordinator),
            patch.object(rules, "git", side_effect=self._git),
        ):
            evidence.verify_binding_inputs(
                self.backend,
                binding,
                lambda *_args: self.authority,
                observed_run,
                acl_reader=lambda _path: self.acl,
            )
        staging_root = Path(binding["staging"]["path"]).parent.resolve()
        self.assertTrue(commands)
        self.assertTrue(all(command.is_relative_to(staging_root) for command in commands))

    def test_staging_rejects_unknown_member_and_same_byte_path_replacement(self):
        _output, binding = self._bind()
        staging_root = Path(binding["staging"]["path"]).parent
        unknown = staging_root / "unknown.bin"
        unknown.write_bytes(b"unknown")
        with self.assertRaisesRegex(ValueError, "未知成员"):
            staging.verify_staging(
                self.run_directory,
                binding["staging"],
                binding["coordinator"],
                run=self._run,
                acl_reader=lambda _path: self.acl,
            )
        unknown.unlink()
        runner = Path(binding["runner"]["path"])
        content = runner.read_bytes()
        original = self.backend / ".local-tests/original-runner.py"
        runner.replace(original)
        runner.write_bytes(content)
        with self.assertRaisesRegex(ValueError, "文件身份"):
            staging.verify_staging(
                self.run_directory,
                binding["staging"],
                binding["coordinator"],
                run=self._run,
                acl_reader=lambda _path: self.acl,
            )

    def test_changed_execution_bytes_fail_before_any_process_starts(self):
        _output, binding = self._bind()
        executable = Path(binding["tools"]["promtool"]["path"])
        executable.write_bytes(b"different-execution-bytes")
        invoked = Mock(side_effect=self._run)
        with self.assertRaisesRegex(ValueError, "执行字节"):
            staging.verify_staging(
                self.run_directory,
                binding["staging"],
                binding["coordinator"],
                run=invoked,
                acl_reader=lambda _path: self.acl,
            )
        invoked.assert_not_called()

    def test_execution_context_only_exposes_secret_through_child_environment(self):
        _output, binding = self._bind()
        private = {
            "APP_SCOPE_ID": "restore-scope",
            "APP_MONITOR_METRICS_BEARER_TOKEN": self.token,
        }
        with staging.verified_staging_execution(
            self.run_directory,
            binding["staging"],
            binding["coordinator"],
            acl_reader=lambda _path: self.acl,
            private_environment=private,
        ) as execution:
            self.assertNotIn("secret", execution)
            self.assertEqual(
                execution["environment"]["APP_MONITOR_METRICS_BEARER_TOKEN"], self.token
            )
            self.assertTrue(Path(execution["runner"]).is_relative_to(execution["root"]))
            self.assertEqual(Path(execution["runner_command"][0]), execution["python"])
            self.assertEqual(Path(execution["runner_command"][-1]), execution["runner"])
        with self.assertRaisesRegex(ValueError, "私有环境"):
            with staging.verified_staging_execution(
                self.run_directory,
                binding["staging"],
                binding["coordinator"],
                acl_reader=lambda _path: self.acl,
                private_environment={"APP_MONITOR_METRICS_BEARER_TOKEN": "wrong"},
            ):
                self.fail("错误凭据不能进入执行阶段")

    @unittest.skipUnless(os.name == "nt", "需要真实 Windows 无写共享句柄")
    def test_staging_guard_blocks_token_a_b_a_during_execution(self):
        _output, binding = self._bind()
        original = self.credential.read_bytes()
        blocked = []

        def attacking_run(command, **kwargs):
            try:
                self.credential.write_bytes(b"B" * len(original))
                self.credential.write_bytes(original)
            except OSError:
                blocked.append(True)
            return self._run(command, **kwargs)

        staging.verify_staging(
            self.run_directory,
            binding["staging"],
            binding["coordinator"],
            run=attacking_run,
            acl_reader=lambda _path: self.acl,
        )
        self.assertTrue(blocked)
        self.assertEqual(self.credential.read_bytes(), original)

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
                acl_reader=lambda _path: self.acl,
            )
        marker = output.parent / "unknown.txt"
        marker.write_text("unknown", encoding="utf-8")
        with (
            patch.object(delivery, "repository", side_effect=lambda path, _label: path),
            patch.object(delivery, "validate_new_output", side_effect=lambda path, _root: path),
            self.assertRaisesRegex(ValueError, "新 run"),
        ):
            delivery.bind(
                *common,
                {"prometheus": 29090, "alertmanager": 29093, "webhook": 29094},
                preflight=lambda *_args: self.authority,
                run=self._run,
                port_check=lambda _url: None,
                acl_reader=lambda _path: self.acl,
            )

    def test_duplicate_cli_option_is_argument_error_before_preflight(self):
        with self.assertRaises(SystemExit) as caught:
            delivery.main(["bind", "--backend-dir", str(self.backend), "--backend-dir", str(self.backend)])
        self.assertEqual(caught.exception.code, 2)

    def test_python_must_be_the_explicit_fixed_interpreter(self):
        with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(ValueError, "RYFRAME_PYTHON"):
            staging._source_python()
        with patch.dict(os.environ, {"RYFRAME_PYTHON": str(self.credential)}), self.assertRaisesRegex(
            ValueError, "指定的解释器"
        ):
            staging._source_python()


@unittest.skipUnless(os.name == "nt", "Windows ACL 合同")
class MonitoringCredentialAclTests(unittest.TestCase):
    def setUp(self):
        self.current = "S-1-5-21-1-2-3-1001"
        self.valid = {
            "current_sid": self.current,
            "owner_sid": self.current,
            "protected": True,
            "entries": [
                {"sid": self.current, "type": 0, "rights": 1, "inherited": False},
                {
                    "sid": permissions.SYSTEM_SID,
                    "type": 0,
                    "rights": 2032127,
                    "inherited": False,
                },
                {
                    "sid": permissions.ADMINISTRATORS_SID,
                    "type": 0,
                    "rights": 2032127,
                    "inherited": False,
                },
            ],
        }

    def test_acl_accepts_only_explicit_current_system_and_administrators(self):
        result = permissions.validate_windows_acl(copy.deepcopy(self.valid))
        self.assertEqual(result["owner_sid"], self.current)
        self.assertEqual({entry["sid"] for entry in result["entries"]}, {
            self.current,
            permissions.SYSTEM_SID,
            permissions.ADMINISTRATORS_SID,
        })

    def test_acl_rejects_users_everyone_write_and_wrong_owner(self):
        for sid in ("S-1-1-0", "S-1-5-32-545"):
            changed = copy.deepcopy(self.valid)
            changed["entries"].append(
                {"sid": sid, "type": 0, "rights": permissions.WRITE_RIGHTS, "inherited": False}
            )
            with self.subTest(sid=sid), self.assertRaisesRegex(ValueError, "未授权"):
                permissions.validate_windows_acl(changed)
        changed = copy.deepcopy(self.valid)
        changed["owner_sid"] = "S-1-5-32-545"
        with self.assertRaisesRegex(ValueError, "独占管理"):
            permissions.validate_windows_acl(changed)


class MonitoringStagingModelTests(unittest.TestCase):
    def test_python_closure_rejects_dynamic_import_and_resolves_local_module(self):
        available = {"scripts/local_module.py"}
        self.assertEqual(
            staging._local_imports(
                "scripts/entry.py", b"import json\nfrom local_module import value\n", available
            ),
            available,
        )
        for source in (
            b"__import__('local_module')\n",
            b"import importlib\nimportlib.import_module('local_module')\n",
        ):
            with self.subTest(source=source), self.assertRaisesRegex(ValueError, "动态"):
                staging._local_imports("scripts/entry.py", source, available)


if __name__ == "__main__":
    unittest.main()
