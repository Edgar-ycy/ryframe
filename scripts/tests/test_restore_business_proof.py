import copy
import json
import shutil
import sys
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))

import restore_business_proof as proof
from restore_runtime_evidence import read_json_document


class RestoreBusinessProofTests(unittest.TestCase):
    def setUp(self):
        scratch = Path(".local-tests/python-unit")
        scratch.mkdir(parents=True, exist_ok=True)
        root = (scratch / f"restore-proof-{uuid.uuid4().hex}").resolve()
        root.mkdir()
        self.addCleanup(shutil.rmtree, root, True)
        self.runner = root / "runner"
        self.verifier = root / "backend"
        self.evidence = self.runner / ".local-tests" / "playwright-real"
        self.evidence.mkdir(parents=True)
        (self.verifier / ".local-tests").mkdir(parents=True)
        self.target_path = self.verifier / ".local-tests" / "target.json"
        self.runtime_path = self.evidence / "restore-drill-runtime.json"
        self.tests_path = self.evidence / "restore-drill-tests.json"
        self.proof_path = self.evidence / "restore-drill.json"
        self.write(self.target_path, {"target": True})
        self.write(self.runtime_path, {"runtime": True})
        self.record = {
            "plan": {
                "id": "drill",
                "backup_id": "backup",
                "scope_id": "restore-proof",
                "fault_at": "2026-09-11T00:00:00Z",
                "databases": [],
                "object_endpoint": "http://127.0.0.1:9000",
                "object_prefix": "restore-proof/",
                "api_ready_url": "http://127.0.0.1:18080/readyz",
                "worker_ready_url": "http://127.0.0.1:19091/readyz",
                "frontend_sha": "d" * 40,
            },
            "plan_hash": "a" * 64,
            "status": "data_verified",
            "started_at": "2026-09-11T00:01:00Z",
            "data_verified_at": "2026-09-11T00:02:00Z",
            "completed_at": None,
            "recovered_at": "2026-09-11T00:00:00Z",
            "failure": None,
        }
        self.authority = {
            "format_version": 2,
            "kind": "restore-runtime-authority",
            "restore_id": "drill",
            "backup_id": "backup",
            "plan_hash": "a" * 64,
            "scope_id": "restore-proof",
            "data_verified_at": "2026-09-11T00:02:00Z",
            "backup_source_sha": "b" * 40,
            "backend_product_sha": "c" * 40,
            "backend_execution_sha": "c" * 40,
            "backend_adapter_contract": None,
            "frontend_sha": "d" * 40,
            "api_endpoint": "http://127.0.0.1:18080/readyz",
            "worker_endpoint": "http://127.0.0.1:19091/readyz",
            "frontend_endpoint": "http://127.0.0.1:14174",
        }
        self.dataset = {}
        for key in ("source_generation", "dataset_lineage"):
            path = self.verifier / ".local-tests" / (key + ".json")
            self.write(path, {key: True})
            self.dataset[key] = proof._descriptor(read_json_document(path))
        self.runner_sha = "e" * 40
        self.verifier_sha = "f" * 40
        self.manifest = {
            "id": "backup",
            "scope_id": "source",
            "source_sha": "b" * 40,
            "quiesced_at": "2026-09-10T00:00:00Z",
            "captured_at": "2026-09-10T00:01:00Z",
            "completed_at": "2026-09-10T00:02:00Z",
            "retention_until": "2026-09-18T00:00:00Z",
            "control_schema_fingerprint": "control",
            "tenant_schema_fingerprint": "tenant",
            "databases": [],
            "objects": [],
            "artifacts": [],
        }

    @staticmethod
    def write(path: Path, value: object) -> None:
        path.write_text(json.dumps(value, separators=(",", ":")), encoding="utf-8")

    def documents(self):
        runtime = read_json_document(self.runtime_path)
        target = read_json_document(self.target_path)
        sources = proof._source_fields(
            self.authority,
            self.runner,
            self.runner_sha,
            self.verifier,
            self.verifier_sha,
        )
        tests = {
            "format_version": 1,
            "kind": "restore-browser-tests",
            "restore": {"id": "drill", "plan_hash": "a" * 64, "scope_id": "restore-proof"},
            "runtime": proof._descriptor(runtime),
            "target_plan": proof._descriptor(target),
            **self.dataset,
            "sources": sources,
            "frontend_url": self.authority["frontend_endpoint"],
            "started_at": "2026-09-11T00:03:00Z",
            "completed_at": "2026-09-11T00:04:00Z",
            "runs": [
                {
                    "title": ["正式恢复"],
                    "status": "passed",
                    "retry": 0,
                    "scenarios": proof.REQUIRED_SCENARIOS,
                }
            ],
        }
        self.write(self.tests_path, tests)
        tests_document = read_json_document(self.tests_path)
        summary = {
            "restore_id": "drill",
            "plan_hash": "a" * 64,
            **{key: self.authority[key] for key in (
                "backup_source_sha",
                "backend_product_sha",
                "backend_execution_sha",
                "backend_adapter_contract",
                "frontend_sha",
            )},
            "runner_sha": self.runner_sha,
            "verifier_sha": self.verifier_sha,
            "scope_id": "restore-proof",
            "frontend_url": self.authority["frontend_endpoint"],
            "runtime_receipt_sha256": runtime.sha256,
            "tests_receipt_sha256": tests_document.sha256,
            "target_plan_sha256": target.sha256,
            **{key + "_sha256": value["sha256"] for key, value in self.dataset.items()},
            "started_at": tests["started_at"],
            "completed_at": tests["completed_at"],
            "scenarios": [
                {"name": name, "succeeded": True} for name in proof.REQUIRED_SCENARIOS
            ],
            "unexpected_console_messages": 0,
            "unexpected_network_failures": 0,
            "axe_serious_or_critical": 0,
        }
        self.write(self.proof_path, summary)
        return (
            read_json_document(self.proof_path),
            tests_document,
            runtime,
            target,
        )

    def validate(self):
        documents = self.documents()
        return proof._validate_evidence(
            self.record,
            self.authority,
            *documents,
            self.runner,
            self.verifier,
            self.dataset,
        )

    def test_exact_authoritative_evidence_is_accepted(self):
        self.assertEqual(self.validate()["restore_id"], "drill")

    def test_authority_requires_data_verified_record_and_exact_fields(self):
        value = {
            "format_version": 1,
            "kind": "restore-business-authority",
            "record": self.record,
            "backup_manifest": self.manifest,
            "tools": {
                "runner": {"root": str(self.runner), "sha": self.runner_sha},
                "verifier": {"root": str(self.verifier), "sha": self.verifier_sha},
                "python": {"path": str(Path(sys.executable).absolute()), "bytes": 1, "sha256": "1" * 64},
            },
        }
        proof._authority(value)
        for change in (
            lambda item: item["record"].update(status="running"),
            lambda item: item["backup_manifest"].update(source_sha="B" * 40),
            lambda item: item["tools"]["runner"].update(sha="E" * 40),
            lambda item: item.update(extra=True),
        ):
            changed = copy.deepcopy(value)
            change(changed)
            with self.assertRaises(ValueError):
                proof._authority(changed)

    def test_self_signed_digest_or_source_change_is_rejected(self):
        proof_document, tests, runtime, target = self.documents()
        for field, value in (
            ("format_version", True),
            ("runtime", {"path": str(runtime.path), "bytes": 1, "sha256": "9" * 64}),
            ("sources", {**tests.value["sources"], "frontend_sha": "9" * 40}),
            ("source_generation", {**self.dataset["source_generation"], "sha256": "9" * 64}),
            ("dataset_lineage", {**self.dataset["dataset_lineage"], "path": str(self.verifier / "other.json")}),
            ("dataset_lineage", {**self.dataset["dataset_lineage"], "extra": True}),
        ):
            changed = copy.deepcopy(tests.value)
            changed[field] = value
            self.write(self.tests_path, changed)
            changed_tests = read_json_document(self.tests_path)
            with self.assertRaises(ValueError):
                proof._validate_evidence(
                    self.record,
                    self.authority,
                    proof_document,
                    changed_tests,
                    runtime,
                    target,
                    self.runner,
                    self.verifier,
                    self.dataset,
                )
            self.write(self.tests_path, tests.value)

    def test_failed_retried_duplicate_or_unknown_scenario_is_rejected(self):
        for runs in (
            [{"title": ["失败"], "status": "failed", "retry": 0, "scenarios": proof.REQUIRED_SCENARIOS}],
            [{"title": ["重试"], "status": "passed", "retry": 1, "scenarios": proof.REQUIRED_SCENARIOS}],
            [{"title": ["布尔重试"], "status": "passed", "retry": False, "scenarios": proof.REQUIRED_SCENARIOS}],
            [{"title": ["重复"], "status": "passed", "retry": 0, "scenarios": [*proof.REQUIRED_SCENARIOS, "login"]}],
            [{"title": ["未知"], "status": "passed", "retry": 0, "scenarios": [*proof.REQUIRED_SCENARIOS[:-1], "unknown"]}],
        ):
            with self.subTest(runs=runs), self.assertRaises(ValueError):
                proof._validate_runs(runs)

    def test_evidence_must_stay_under_runner_directory(self):
        proof_document, tests, runtime, target = self.documents()
        with self.assertRaises(ValueError):
            proof._evidence_root(self.runner / "other", "drill", proof_document, tests, runtime)
        renamed = self.evidence / "other.json"
        self.proof_path.replace(renamed)
        with self.assertRaises(ValueError):
            proof._evidence_root(
                self.runner,
                "drill",
                read_json_document(renamed),
                tests,
                runtime,
            )

    def business_authority(self):
        return {
            "format_version": 1,
            "kind": "restore-business-authority",
            "record": self.record,
            "backup_manifest": self.manifest,
            "tools": {
                "runner": {"root": str(self.runner), "sha": self.runner_sha},
                "verifier": {"root": str(self.verifier), "sha": self.verifier_sha},
                "python": proof.artifact_snapshot(Path(sys.executable)).descriptor(),
            },
        }

    def full_verification(self, target_manifest=None):
        proof_document, tests, runtime, target = self.documents()
        execution = {
            "roots": {
                "source_backend": str(self.verifier),
                "execution_backend": str(self.verifier),
                "frontend": str(self.runner),
            },
            "backend_product_sha": self.authority["backend_product_sha"],
            "backend_execution_sha": self.authority["backend_execution_sha"],
            "frontend_sha": self.authority["frontend_sha"],
            "builds": {},
            "adapter": None,
        }
        target_value = {"product_plan": self.record["plan"], "product_execution": execution}
        reference = {"target": {"frontend_url": self.authority["frontend_endpoint"]}}
        runtime_value = {"paths": {"bindings": str(self.verifier / ".local-tests/bindings.json")}}
        verified_runtime = {"runtime_receipt_sha256": runtime.sha256}
        with patch.object(proof, "repository", side_effect=lambda path, _label: path), patch.object(
            proof,
            "_verified_target",
            return_value=(target, target_value, reference, target_manifest or self.manifest, self.dataset),
        ), patch.object(proof, "validate_runtime_receipt", return_value=runtime_value), patch.object(
            proof, "_clean_source", side_effect=lambda path, _sha, _label: path
        ), patch.object(
            proof, "verify_live_generation", return_value=verified_runtime
        ) as live:
            result = proof.verify_business_proof(
                self.verifier,
                self.runner,
                proof_document.path,
                tests.path,
                runtime.path,
                target.path,
                self.business_authority(),
            )
        return result, live

    def test_full_verification_uses_live_generation_and_database_manifest(self):
        result, live = self.full_verification()
        self.assertEqual(result["restore_id"], "drill")
        live.assert_called_once()

        other = {**self.manifest, "source_sha": "9" * 40}
        with self.assertRaisesRegex(ValueError, "登记库权威备份不同"):
            self.full_verification(other)


if __name__ == "__main__":
    unittest.main()
