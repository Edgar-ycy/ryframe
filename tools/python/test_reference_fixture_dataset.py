import contextlib
import copy
import io
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

BACKEND = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BACKEND / "scripts"))

import reference_fixture_dataset as dataset
from reference_fixture_control_protocol import run_private
from workspace_directory import WorkspaceDirectory


@contextlib.contextmanager
def _stage_runner(*, side_effect=None):
    """仅替换外部数据准备阶段，原生只读证据调用继续真实执行。"""
    native_run = subprocess.run
    stage = Mock(side_effect=side_effect)
    def dispatch(command, **options):
        if any(Path(str(argument)).name == "restore_reference.py" for argument in command):
            return stage(command, **options)
        return native_run(command, **options)
    with patch.object(dataset.subprocess, "run", side_effect=dispatch):
        yield stage


class ReferenceFixtureDatasetTests(unittest.TestCase):
    def setUp(self):
        self.temp = WorkspaceDirectory(BACKEND / ".local-tests/t", prefix="rfd-")
        self.addCleanup(self.temp.cleanup)
        self.backend = self.temp.path / "backend"
        self.execution = self.backend / ".local-tests/business/backend"
        self.runtime = self.execution / ".local-tests/reference-fixture/source-runtime-r1"
        self.runtime.mkdir(parents=True)
        self.work = self.runtime / "dataset-r1"
        self.secret = self.execution / ".local-tests/reference-fixture/secrets/mysql.cnf"
        self.secret.parent.mkdir(parents=True)
        self.secret.write_text("[client]\n", encoding="utf-8")
        self.tool = self.backend / "tool.exe"
        self.tool.parent.mkdir(parents=True, exist_ok=True)
        self.tool.write_bytes(b"tool")
        self.bootstrap = self.execution / ".local-tests/bootstrap.json"
        self.review = self.backend / ".local-tests/review.json"
        self.bootstrap.parent.mkdir(parents=True, exist_ok=True)
        self.review.parent.mkdir(parents=True, exist_ok=True)
        self.review.write_text(json.dumps(self._review()), encoding="utf-8")
        self.credentials = self.bootstrap.parent / "administrator.toml"
        self.credentials.write_text("[administrator]\ntenant_id='system'\nusername='admin'\npassword='Current!Changed123'\n", encoding="utf-8")
        self.bootstrap.write_text(json.dumps({"plan": {"review": dataset._bound(self.review)}, "administrator_credentials": dataset._bound(self.credentials)}), encoding="utf-8")
        (self.runtime / "source-pair.json").write_text(json.dumps({"sources": {"backend": {"head": "a" * 40}}}), encoding="utf-8")
        self.private = {"APP_ENV": "test", "access_key": "fixture-access", "secret_key": "fixture-secret", "admin_password":"Initial!Only123"}
        config=self.runtime / "config"
        config.mkdir()
        (config / "app.test.toml").write_text('[bootstrap]\nadmin_password="Initial!Only123"\n[object_storage]\naccess_key="fixture-access"\nsecret_key="fixture-secret"\n',encoding="utf-8")
        dataset.write_json(self.runtime / "runtime.json", {"api_config_dir":str(config)})
        self.verified={"scope_id":"fixture-seed-r1","runtime":dataset._bound(self.runtime / "runtime.json")}
        self.configuration=dataset._configuration(self.execution,self.runtime,self.verified)
        self.patches = [
            patch.object(dataset, "_bootstrap", return_value=(self.bootstrap, self.execution, self.private)),
            patch.object(dataset, "_output", side_effect=lambda _execution, value, **_kwargs: Path(value)),
            patch.object(dataset, "verify_runtime", return_value=self.verified),
            patch.object(dataset, "validate_review"),
        ]
        for value in self.patches:
            value.start()
            self.addCleanup(value.stop)

    def _scope(self, side):
        return {
            "scope_id": f"fixture-{side}-r1",
            "runtime_dir": str(self.execution / ".local-tests/reference-fixture" / f"runtime-{side}"),
            "api_url": "http://127.0.0.1:18080",
            "worker_ready_url": "http://127.0.0.1:18081/readyz",
            "frontend_url": "http://127.0.0.1:4300",
            "objects": {"endpoint": "http://127.0.0.1:29000", "region": "us-east-1"},
            "databases": [
                {"key": key, "connection_file": str(self.secret), "database": f"{side}_{key.replace('-', '_')}",
                 "expected_server_uuid": "12345678-1234-1234-1234-123456789abc"}
                for key in dataset.KEYS
            ],
        }

    def _review(self):
        tools = {name: {"path": str(self.tool), "sha256": dataset.file_digest(self.tool)["sha256"]}
                 for name in ("mysql", "mysqldump")}
        return {"scopes": {side: self._scope(side) for side in ("seed", "base", "candidate")},
                "tools": tools,
                "dataset": {"tenant_targets": ["shared"] * 9 + ["dedicated-a"],
                            "records": 100_000, "object_count": 256,
                            "object_bytes": 4 * 1024 * 1024, "request_interval_ms": 1000}}

    def test_build_plan_binds_current_runtime_and_required_dataset_scale(self):
        value = dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")
        from restore_reference_target import _plan
        _plan(value, self.execution)
        self.assertEqual(value["source"]["runtime_dir"], str(self.runtime))
        self.assertEqual(value["source"]["scope_id"], "fixture-seed-r1")
        self.assertEqual(value["target"]["scope_id"], "fixture-base-r1")
        self.assertEqual(value["target"]["worker_ready_url"], "http://127.0.0.1:18081/readyz")
        self.assertEqual(value["target_side"], "base")
        self.assertEqual(value["dataset"]["records"], 100_000)
        self.assertEqual(value["dataset"]["object_count"] * value["dataset"]["object_bytes"], 1024**3)
        self.assertEqual(value["dataset"]["tenant_targets"], self._review()["dataset"]["tenant_targets"])
        self.assertEqual(set(value["tools"]), {"mysql", "mysqldump"})
        self.assertEqual(value["dataset"]["configuration"], self.configuration)
        self.assertEqual(value["dataset"]["administrator_credentials"], dataset._bound(self.credentials))
        self.assertNotIn("Current!Changed123", json.dumps(value))
        self.assertNotIn(self.private["admin_password"], json.dumps(value))
        self.assertNotIn(self.private["access_key"], json.dumps(value))
        self.assertNotIn(self.private["secret_key"], json.dumps(value))
        self.assertNotIn("password", value["dataset"]["admin"])
        self.assertNotIn("owner_password", value["dataset"])
        self.assertNotIn("password_env", value["dataset"]["admin"])

    def test_target_schema_requires_exact_current_credentials_binding_before_native_validation(self):
        import restore_reference_target as target
        value = dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")
        for field in ("administrator_credentials", "path", "bytes", "sha256", "password"):
            changed = copy.deepcopy(value)
            if field == "administrator_credentials":
                del changed["dataset"][field]
            elif field == "password":
                changed["dataset"]["administrator_credentials"][field] = "private-sentinel"
            else:
                del changed["dataset"]["administrator_credentials"][field]
            with self.subTest(field=field), patch.object(target, "validate_plan") as native:
                with self.assertRaises(ValueError):
                    target._plan(changed, self.execution)
                native.assert_not_called()

    def test_missing_or_changed_prebound_current_credentials_fail_without_stage_execution(self):
        original = self.bootstrap.read_text(encoding="utf-8")
        document = json.loads(original)
        del document["administrator_credentials"]
        self.bootstrap.write_text(json.dumps(document), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "当前管理员凭据"):
            dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")
        self.bootstrap.write_text(original, encoding="utf-8")
        self.credentials.write_text("changed-private-sentinel", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "凭据路径或摘要变化"):
            dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")

    def test_side_uses_only_config_binding_and_rejects_inline_credentials(self):
        ambient = {"AWS_ACCESS_KEY_ID": "ambient-access", "AWS_SECRET_ACCESS_KEY": "ambient-secret",
                   "TEST_ACCESS_KEY": "ambient-test-access", "TEST_SECRET_KEY": "ambient-test-secret"}
        with patch.dict(os.environ, ambient):
            side = dataset._side(self._scope("seed"), self.configuration)
            self.assertEqual(side["s3"], {"endpoint": "http://127.0.0.1:29000", "region": "us-east-1",
                                          "configuration": self.configuration})
            for key in ("access_key", "secret_key"):
                for invalid in (None, "", 123):
                    with self.subTest(key=key, invalid=invalid), self.assertRaisesRegex(ValueError, "文件绑定"):
                        dataset._side(self._scope("seed"), {**self.configuration, key: invalid})
                missing = {name: value for name, value in self.private.items() if name != key}
                with self.assertRaisesRegex(ValueError, "文件绑定"):
                    dataset._side(self._scope("seed"), missing)
            with self.assertRaisesRegex(ValueError, "文件绑定"):
                dataset._side(self._scope("seed"), {
                    "access_key_env": "TEST_ACCESS_KEY", "secret_key_env": "TEST_SECRET_KEY",
                })

    def test_dataset_preserves_all_reviewed_inputs_without_aliasing(self):
        review = self._review()
        review["dataset"].update({"tenant_targets": ["shared"] * 8 + ["dedicated-a", "dedicated-b"],
                                  "records": 200_000, "object_count": 512,
                                  "object_bytes": 2 * 1024 * 1024, "request_interval_ms": 1500})
        result = dataset._dataset(review, self.configuration, dataset._bound(self.credentials))
        for key, expected in review["dataset"].items():
            self.assertEqual(result[key], expected)
        result["tenant_targets"].append("shared")
        self.assertEqual(len(review["dataset"]["tenant_targets"]), 10)

    def test_dataset_rejects_missing_reviewed_input(self):
        for key in self._review()["dataset"]:
            with self.subTest(key=key):
                review = self._review()
                del review["dataset"][key]
                with self.assertRaises(KeyError):
                    dataset._dataset(review, self.configuration, dataset._bound(self.credentials))

    def test_build_plan_rejects_overlapping_api_and_worker_probe(self):
        review = self._review()
        review["scopes"]["base"]["worker_ready_url"] = "http://127.0.0.1:18080/readyz"
        self.review.write_text(json.dumps(review), encoding="utf-8")
        self.bootstrap.write_text(
            json.dumps({"plan": {"review": dataset._bound(self.review)}}),
            encoding="utf-8",
        )

        with self.assertRaisesRegex(ValueError, "API 和 Worker 探针不得重叠"):
            dataset.build_plan(
                self.backend, self.bootstrap, self.runtime, self.work, "base"
            )

    def test_build_plan_rejects_runtime_scope_mismatch(self):
        with patch.object(dataset, "verify_runtime", return_value={**self.verified,"scope_id": "other"}):
            with self.assertRaisesRegex(ValueError, "同一代次"):
                dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")

    def test_build_plan_requires_and_selects_exact_restore_side(self):
        candidate = dataset.build_plan(
            self.backend, self.bootstrap, self.runtime, self.work, "candidate"
        )
        self.assertEqual(candidate["target_side"], "candidate")
        self.assertEqual(candidate["target"]["scope_id"], "fixture-candidate-r1")
        self.assertTrue(all(item["database"].startswith("candidate_")
                            for item in candidate["target"]["databases"]))
        for side in ("seed", "source", "target", "", None):
            with self.subTest(side=side), self.assertRaisesRegex(ValueError, "base 或 candidate"):
                dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, side)

    def test_cli_requires_side_before_planning_or_writing(self):
        arguments = [
            "plan", "--backend-dir", str(self.backend),
            "--environment", str(self.bootstrap), "--runtime", str(self.runtime),
            "--work-dir", str(self.work), "--output", str(self.runtime / "plan.json"),
            "--write",
        ]
        with contextlib.redirect_stderr(io.StringIO()), \
                patch.object(dataset, "write_plan") as write, self.assertRaises(SystemExit) as error:
            dataset.main(arguments)
        self.assertEqual(error.exception.code, 2)
        write.assert_not_called()

    def test_write_plan_rejects_shared_work_and_output_path(self):
        with patch.object(dataset, "write_json") as write, self.assertRaisesRegex(
            ValueError, "必须不同"
        ):
            dataset.write_plan(
                self.backend,
                self.bootstrap,
                self.runtime,
                self.work,
                self.work,
                "base",
            )
        write.assert_not_called()

    def test_private_protocol_is_removed_before_business_dependencies_load(self):
        raw = json.dumps({
            "backend_dir": str(self.backend),
            "domain": "dataset",
            "environment": str(self.bootstrap),
            "format_version": 1,
            "kind": "ryframe-reference-fixture-control",
            "operation": "plan",
            "output": str(self.runtime / "dataset-plan.json"),
            "runtime": str(self.runtime),
            "side": "base",
            "work_dir": str(self.work),
            "write": True,
        }, separators=(",", ":"))
        observed = []

        def load_dependencies():
            observed.append(sys.argv[1:] == [])

        with (
            patch.object(sys, "argv", ["reference_fixture_dataset.py"]),
            patch.object(dataset, "_load_business_dependencies", side_effect=load_dependencies),
            patch.object(dataset, "main") as main,
        ):
            self.assertEqual(
                run_private(
                    "dataset",
                    dataset.PROTOCOL_SCHEMAS,
                    dataset._private_main,
                    positional_operation=True,
                    raw=raw,
                ),
                0,
            )
        self.assertEqual(observed, [True])
        main.assert_called_once()

    def test_prepare_rejects_side_different_from_saved_plan_before_execution(self):
        plan = dataset.build_plan(
            self.backend, self.bootstrap, self.runtime, self.work, "base"
        )
        plan_file = self.runtime / "plan.json"
        dataset.write_json(plan_file, plan)
        with patch.object(dataset.subprocess, "run") as run, self.assertRaisesRegex(
            ValueError, "显式恢复目标侧不一致"
        ):
            dataset.prepare(
                self.backend, self.bootstrap, self.runtime, plan_file, "candidate"
            )
        run.assert_not_called()

    def test_prepare_failure_records_redacted_diagnostic_once(self):
        error = subprocess.CalledProcessError(1, ["node"], output="access=fixture-access", stderr="fixture-secret")
        path = dataset._write_prepare_failure(
            self.runtime,
            self.runtime / "dataset-plan.json",
            error,
            self.private,
            self.runtime / "dataset-plan.node-reports",
        )
        value = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(value["status"], "failed_unknown_writes")
        for key in ("access_key", "secret_key"):
            self.assertNotIn(self.private[key], value["stdout"] + value["stderr"])
        self.assertIn("[REDACTED]", value["stdout"] + value["stderr"])
        with self.assertRaisesRegex(ValueError, "禁止覆盖"):
            dataset._write_prepare_failure(self.runtime, self.runtime / "dataset-plan.json", error, {}, self.runtime / "other")

    def test_prepare_uses_stdin_and_safe_environment_and_refuses_replay(self):
        plan = dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")
        plan_file = self.runtime / "dataset-plan.json"
        dataset.write_json(plan_file, plan)
        result_path = self.work / "dataset/result.json"

        def run(command, **options):
            self.assertEqual(command[-1], "--stdin")
            self.assertNotIn(str(plan_file), command)
            self.assertNotIn(self.private["admin_password"], str(command))
            request = json.loads(options["input"])
            self.assertEqual(request["request"]["operation"], "dataset")
            self.assertEqual(request["request"]["plan"], str(plan_file))
            self.assertTrue(request["request"]["write"])
            self.assertFalse(any(key.startswith(("APP_", "RYFRAME_", "AWS_")) for key in options["env"]))
            result_path.parent.mkdir(parents=True)
            dataset.write_json(result_path, {"format_version": 1, "records": 100000})
            return subprocess.CompletedProcess(command, 0, stdout="{}", stderr="")

        with _stage_runner(side_effect=run) as execute:
            result = dataset.prepare(self.backend, self.bootstrap, self.runtime, plan_file, "base")
        self.assertEqual(result["status"], "reference_fixture_dataset_prepared")
        self.assertEqual(result["dataset"], dataset._bound(result_path))
        self.assertEqual(execute.call_count, 1)
        with _stage_runner() as execute, self.assertRaises(ValueError):
            dataset.prepare(self.backend, self.bootstrap, self.runtime, plan_file, "base")
        execute.assert_not_called()

    def test_prepare_preserves_unknown_write_failure_and_never_reexecutes(self):
        plan = dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")
        plan_file = self.runtime / "dataset-plan.json"
        dataset.write_json(plan_file, plan)
        error = subprocess.CalledProcessError(1, ["xtask"], output=self.private["admin_password"], stderr=self.private["secret_key"])
        with _stage_runner(side_effect=error) as execute, self.assertRaisesRegex(RuntimeError, "脱敏诊断"):
            dataset.prepare(self.backend, self.bootstrap, self.runtime, plan_file, "base")
        self.assertEqual(execute.call_count, 1)
        failure = dataset.read_json(self.runtime / "dataset-plan.prepare-failed.json")
        self.assertEqual(failure["status"], "failed_unknown_writes")
        self.assertNotIn(self.private["admin_password"], failure["stdout"])
        self.assertNotIn(self.private["secret_key"], failure["stderr"])
        with _stage_runner() as execute, self.assertRaisesRegex(ValueError, "禁止覆盖或重放"):
            dataset.prepare(self.backend, self.bootstrap, self.runtime, plan_file, "base")
        execute.assert_not_called()
