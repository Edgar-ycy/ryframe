"""seed 到 arm 清单发布的离线证据与失败关闭。"""
from __future__ import annotations

import copy
from contextlib import nullcontext
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_seed_arm as arm
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding


class SeedArmTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.directory = self.local / "run"
        (self.directory / "seed-runtime").mkdir(parents=True)
        (self.local / "copies").mkdir()
        self.source_registration = self.file("source-registration", {"published": True})
        self.source_request = self.file("source-request", {"source": True})
        self.source_environment = self.file("source-environment", {"environment": {"SIDE": "source"}})
        self.source_storage = self.file("source-storage", {"storage": True})
        self.generation_verified = self.file("generation-verified", {"generation": True})
        self.initialized = self.file("initialized", {"initialized": True})
        self.target_registration = self.file("target-registration", {"registered": True})
        self.target_initialized_files = self.file("target-initialized-files", {"files": True})
        self.target_environment = self.file("target-environment", {"environment": {"SIDE": "target"}})
        self.target_storage_run = {
            "path": str(self.directory), "manifest": self.file("storage-manifest", {"run": True}),
            "storage": {"generation": "rustfs"}, "cache": {"generation": "redis"},
        }
        self.request = {
            "format_version": 1,
            "kind": "devex-clone-seed-arm-input",
            "source_registration": self.source_registration,
            "id": "arm-base",
            "initialized": self.initialized,
            "target_registration": self.target_registration,
            "target_initialized_files": self.target_initialized_files,
            "target_environment": self.target_environment,
            "copy_directory": str(self.local / "copies/arm-base"),
            "build_bridges": [],
        }
        self.request_file = self.local / "arm-request.json"
        write_json(self.request_file, self.request)
        self.inputs = self.fake_inputs()

    def file(self, name: str, value: dict) -> dict:
        path = self.local / f"{name}.json"
        write_json(path, value)
        return binding(path)

    def fake_inputs(self, *, side: str = "base") -> dict:
        source = {
            "registration": {
                "source_request": self.source_request,
                "source_environment": self.source_environment,
                "source_storage": self.source_storage,
                "generation_verified": self.generation_verified,
            },
            "request": {
                "source": {
                    "scope_id": "seed-scope",
                    "databases": [{"server_uuid": "source-server", "database": "control"}],
                }
            },
            "seed_target": {"side": "seed"},
        }
        target = {
            "side": side,
            "target": {
                "scope_id": f"{side}-scope",
                "databases": [{"server_uuid": "target-server", "database": "control"}],
            },
            "maintenance_build": self.file(f"{side}-build", {"build": side}),
        }
        return {
            "source": source,
            "initialized": {"initialized": side},
            "target": target,
            "target_registration": {"environment": self.target_environment},
            "target_storage_run": self.target_storage_run,
            "target_side": side,
            "copy_directory": Path(self.request["copy_directory"]),
        }

    def result(self, *, side: str = "base", source_registration: dict | None = None) -> dict:
        return {
            "status": "seed_arm_input_published",
            "request": binding(self.request_file),
            "manifest": self.file(f"{side}-manifest", {"manifest": side}),
            "source_registration": source_registration or self.source_registration,
            "source_request": self.source_request,
            "source_storage": self.source_storage,
            "generation_verified": self.generation_verified,
            "initialized": self.initialized,
            "target_environment": self.target_environment,
            "target_registration": self.target_registration,
            "target_initialized_files": self.target_initialized_files,
            "target_storage_run": self.target_storage_run,
            "target_side": side,
            "remote_writes": 0,
            "outbox_drained": True,
            "restore_qualified": False,
        }

    def test_inputs_close_request_fields_and_manifest_derives_source_bindings(self):
        source = self.inputs["source"]
        target = self.inputs["target"]
        with patch.object(arm, "published_source", return_value=source) as published, \
                patch.object(arm, "target_lifecycle_binding", return_value={
                    "registration": self.inputs["target_registration"],
                    "initialized": self.inputs["initialized"], "target": target,
                    "target_storage_run": self.target_storage_run,
                }), \
                patch.object(arm, "request_binding", return_value=("same-review", {})), \
                patch.object(arm, "_target_build") as target_build:
            observed = arm._inputs(self.backend, self.request, live_storage=True)
        published.assert_called_once_with(self.backend, self.source_registration, live_storage=True)
        target_build.assert_called_once_with(self.backend, target, [])
        manifest = arm._manifest(self.request, observed)
        self.assertEqual(set(manifest), {
            "format_version", "kind", "id", "source_request", "source_export", "initialized",
            "source_environment", "target_environment", "copy_directory", "copy_stage",
            "build_bridges", "source_registration", "target_registration",
            "target_initialized_files", "target_storage_run",
        })
        self.assertEqual(manifest["source_request"], self.source_request)
        self.assertEqual(manifest["source_environment"], self.source_environment)
        self.assertEqual(manifest["source_registration"], self.source_registration)
        self.assertEqual(manifest["target_registration"], self.target_registration)
        self.assertEqual(manifest["target_initialized_files"], self.target_initialized_files)
        self.assertEqual(manifest["target_storage_run"], self.target_storage_run)
        self.assertIsNone(manifest["source_export"])
        self.assertEqual(manifest["copy_stage"], "seed_to_arm")

        for field in ("source_request", "source_environment", "source_storage"):
            changed = {**self.request, field: self.source_request}
            with self.subTest(unknown_field=field), self.assertRaises(ValueError):
                arm._inputs(self.backend, changed, live_storage=False)

    def test_target_build_uses_current_audited_bridge_and_verified_tool_receipt(self):
        target = self.inputs["target"]
        receipt = {
            "kind": "devex-clone-tool-build",
            "source": {"worktree_fingerprint": "product-current"},
        }
        with patch.object(arm, "artifact_sources", return_value=nullcontext()) as audited, \
                patch.object(arm, "verify_tools", return_value=receipt) as verify:
            arm._target_build(self.backend, target, [])
        audited.assert_called_once_with(self.backend, [])
        verify.assert_called_once()

        with patch.object(arm, "artifact_sources", return_value=nullcontext()), \
                patch.object(arm, "verify_tools", return_value={"kind": "devex-clone-tool-build"}), \
                self.assertRaises(ValueError):
            arm._target_build(self.backend, target, [])

    def test_publish_resumes_only_identical_failed_prefix(self):
        partial = self.directory / "seed-runtime/attempt-0001"
        partial.mkdir()
        write_json(partial / "request.json", self.request)
        attempts = [{"number": 1, "stage": "seed-runtime", "mode": "arm-input", "status": "failed"}]
        with patch.object(arm, "load_state", return_value={"attempts": attempts}), \
                patch.object(arm, "_inputs", return_value=self.inputs), \
                patch.object(arm, "target_storage_control", return_value=nullcontext()), \
                patch("devex_clone_run.target_lifecycle_binding", return_value={}), \
                patch("devex_clone_run.inherited_build_bridges", return_value=[]):
            result = arm.publish_arm_input(self.backend, self.directory, self.request_file, 2)
        self.assertEqual(result["status"], "seed_arm_input_published")
        self.assertEqual(result["remote_writes"], 0)
        self.assertEqual(read_json(partial / "request.json"), self.request)
        self.assertEqual(read_json(partial / "manifest.json")["source_registration"], self.source_registration)
        self.assertFalse((self.directory / "seed-runtime/attempt-0002").exists())

    def test_partial_resume_rejects_non_prefix_unknown_and_ambiguous_history(self):
        first = self.directory / "seed-runtime/attempt-0001"
        first.mkdir()
        write_json(first / "manifest.json", {"out_of_order": True})
        attempts = [{"number": 1, "stage": "seed-runtime", "mode": "arm-input", "status": "failed"}]
        with patch.object(arm, "load_state", return_value={"attempts": attempts}), self.assertRaises(ValueError):
            arm._existing_output(self.directory, 2)

        (first / "manifest.json").unlink()
        (first / "unknown.txt").write_text("unknown", encoding="utf-8")
        with patch.object(arm, "load_state", return_value={"attempts": attempts}), self.assertRaises(ValueError):
            arm._existing_output(self.directory, 2)

        (first / "unknown.txt").unlink()
        second = self.directory / "seed-runtime/attempt-0002"
        second.mkdir()
        attempts.append({"number": 2, "stage": "seed-runtime", "mode": "arm-input", "status": "failed"})
        with patch.object(arm, "load_state", return_value={"attempts": attempts}), self.assertRaises(ValueError):
            arm._existing_output(self.directory, 3)

    def test_repeated_target_side_and_changed_source_registration_fail_closed(self):
        saved = self.result()
        result_path = self.directory / "prior-result.json"
        write_json(result_path, saved)
        attempts = [{"number": 1, "stage": "seed-runtime", "mode": "arm-input", "status": "passed",
                     "result": binding(result_path)}]
        with patch.object(arm, "load_state", return_value={"attempts": attempts}), \
                patch.object(arm, "_inputs", return_value=self.inputs), \
                patch.object(arm, "target_storage_control", return_value=nullcontext()), \
                patch("devex_clone_run.target_lifecycle_binding", return_value={}), \
                patch("devex_clone_run.inherited_build_bridges", return_value=[]), \
                self.assertRaises(ValueError):
            arm.publish_arm_input(self.backend, self.directory, self.request_file, 2)

        other_registration = self.file("other-registration", {"published": "other"})
        saved = self.result(side="candidate", source_registration=other_registration)
        result_path.unlink()
        write_json(result_path, saved)
        with patch.object(arm, "load_state", return_value={"attempts": attempts}), \
                patch.object(arm, "_inputs", return_value=self.inputs), \
                patch.object(arm, "target_storage_control", return_value=nullcontext()), \
                patch("devex_clone_run.target_lifecycle_binding", return_value={}), \
                patch("devex_clone_run.inherited_build_bridges", return_value=[]), \
                self.assertRaises(ValueError):
            arm.publish_arm_input(self.backend, self.directory, self.request_file, 2)

    def test_publish_keeps_partial_evidence_when_inputs_drift(self):
        changed = copy.deepcopy(self.inputs)
        changed["source"]["registration"]["source_storage"] = self.file("changed-storage", {"changed": True})
        with patch.object(arm, "load_state", return_value={"attempts": []}), \
                patch.object(arm, "_inputs", side_effect=[self.inputs, self.inputs, changed]), \
                patch.object(arm, "target_storage_control", return_value=nullcontext()), \
                patch("devex_clone_run.target_lifecycle_binding", return_value={}), \
                patch("devex_clone_run.inherited_build_bridges", return_value=[]), \
                self.assertRaises(ValueError):
            arm.publish_arm_input(self.backend, self.directory, self.request_file, 1)
        output = self.directory / "seed-runtime/attempt-0001"
        self.assertEqual(read_json(output / "request.json"), self.request)
        self.assertEqual(read_json(output / "manifest.json")["source_registration"], self.source_registration)


if __name__ == "__main__":
    unittest.main()
