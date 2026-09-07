import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import reference_fixture_dataset as dataset


class ReferenceFixtureDatasetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.backend = Path(self.temp.name) / "backend"
        self.execution = self.backend / ".local-tests/device/backend"
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
            patch.object(dataset, "validate_plan"),
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
        return {"scopes": {"seed": self._scope("seed"), "base": self._scope("base")}, "tools": tools}

    def test_build_plan_binds_current_runtime_and_required_dataset_scale(self):
        value = dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work)
        self.assertEqual(value["source"]["runtime_dir"], str(self.runtime))
        self.assertEqual(value["source"]["scope_id"], "fixture-seed-r1")
        self.assertEqual(value["target"]["scope_id"], "fixture-base-r1")
        self.assertEqual(value["dataset"]["records"], 100_000)
        self.assertEqual(value["dataset"]["object_count"] * value["dataset"]["object_bytes"], 1024**3)
        self.assertEqual(value["dataset"]["tenant_targets"].count("shared"), 8)
        self.assertEqual(set(value["tools"]), {"mysql", "mysqldump", "aws", "node"})

    def test_build_plan_rejects_runtime_scope_mismatch(self):
        with patch.object(dataset, "verify_runtime", return_value={"scope_id": "other", "runtime": {}}):
            with self.assertRaisesRegex(ValueError, "同一代次"):
                dataset.build_plan(self.backend, self.bootstrap, self.runtime, self.work)

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
