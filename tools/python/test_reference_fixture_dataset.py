import contextlib
import copy
import io
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

BACKEND = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(BACKEND / "scripts"))

import reference_fixture_dataset as dataset
from reference_fixture_control_protocol import PROTOCOL_KEY, run_private
from workspace_directory import WorkspaceDirectory


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
        self.bootstrap = self.backend / ".local-tests/bootstrap.json"
        self.review = self.backend / ".local-tests/review.json"
        self.bootstrap.parent.mkdir(parents=True, exist_ok=True)
        self.review.write_text(json.dumps(self._review()), encoding="utf-8")
        self.bootstrap.write_text(json.dumps({"plan": {"review": dataset._bound(self.review)}}), encoding="utf-8")
        (self.runtime / "source-pair.json").write_text(json.dumps({"sources": {"backend": {"head": "a" * 40}}}), encoding="utf-8")
        self.patches = [
            patch.object(dataset, "_bootstrap", return_value=(self.bootstrap, self.execution, {"APP_ENV": "test"})),
            patch.object(dataset, "_output", side_effect=lambda _execution, value, **_kwargs: Path(value)),
            patch.object(dataset, "verify_runtime", return_value={"scope_id": "fixture-seed-r1", "runtime": dataset._bound(self.runtime / "source-pair.json")}),
            patch.object(dataset, "_runtime_environment", return_value={"APP_ENV": "test"}),
            patch.object(dataset, "validate_review"),
            patch.object(dataset, "_node", return_value={"path": str(self.tool), "sha256": dataset.file_digest(self.tool)["sha256"]}),
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
                 for name in ("mysql", "mysqldump", "aws")}
        return {"scopes": {side: self._scope(side) for side in ("seed", "base", "candidate")},
                "tools": tools}

    def test_build_plan_binds_current_runtime_and_required_dataset_scale(self):
        value = dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work, "base")
        self.assertEqual(value["source"]["runtime_dir"], str(self.runtime))
        self.assertEqual(value["source"]["scope_id"], "fixture-seed-r1")
        self.assertEqual(value["target"]["scope_id"], "fixture-base-r1")
        self.assertEqual(value["target"]["worker_ready_url"], "http://127.0.0.1:18081/readyz")
        self.assertEqual(value["target_side"], "base")
        self.assertEqual(value["dataset"]["records"], 100_000)
        self.assertEqual(value["dataset"]["object_count"] * value["dataset"]["object_bytes"], 1024**3)
        self.assertEqual(value["dataset"]["tenant_targets"].count("shared"), 8)
        self.assertEqual(set(value["tools"]), {"mysql", "mysqldump", "aws", "node"})

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
        with patch.object(dataset, "verify_runtime", return_value={"scope_id": "other", "runtime": {}}):
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
            observed.append(PROTOCOL_KEY in os.environ)

        with (
            patch.dict(os.environ, {PROTOCOL_KEY: raw}),
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
                ),
                0,
            )
            self.assertIn(PROTOCOL_KEY, os.environ)
        self.assertEqual(observed, [False])
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
        error = subprocess.CalledProcessError(1, ["node"], output="password=secret", stderr="secret")
        path = dataset._write_prepare_failure(
            self.runtime,
            self.runtime / "dataset-plan.json",
            error,
            {"APP_DATABASE_PASSWORD": "secret"},
            self.runtime / "dataset-plan.node-reports",
        )
        value = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(value["status"], "failed_unknown_writes")
        self.assertNotIn("secret", value["stdout"] + value["stderr"])
        with self.assertRaisesRegex(ValueError, "禁止覆盖"):
            dataset._write_prepare_failure(self.runtime, self.runtime / "dataset-plan.json", error, {}, self.runtime / "other")
