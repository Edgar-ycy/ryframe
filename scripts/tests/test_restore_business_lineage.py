"""浏览器证明从目标计划的完整来源链派生血缘，不能用另一份文件自证。"""

import contextlib
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

import restore_business_proof as proof
from devex_clone_seed_generation import RESULT_FIELDS, START_FIELDS
from restore_source_runtime import RECEIPT_FIELDS
from workspace_directory import WorkspaceDirectory


class BusinessLineageTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(Path(__file__).resolve().parents[2] / ".local-tests/t", prefix="bp-")
        self.addCleanup(temporary.cleanup)
        self.backend = temporary.path
        self.root = self.backend / ".local-tests"
        self.root.mkdir()

    def write(self, name, value):
        path = self.root / name
        path.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")
        return proof._descriptor(proof.read_json_document(path))

    def chain(self, *, stop_change=None, start_change=None, runtime_change=None):
        lineage = self.write("lineage.json", {"immutable": "derived lineage"})
        start_value = {**dict.fromkeys(START_FIELDS), "status": "seed_source_generation_running",
                       "dataset_lineage": lineage}
        if start_change:
            start_change(start_value)
        start = self.write("start.json", start_value)
        runtime_value = {**dict.fromkeys(RECEIPT_FIELDS), "source_generation": start, "dataset_lineage": lineage}
        if runtime_change:
            runtime_change(runtime_value)
        runtime = self.write("runtime.json", runtime_value)
        stop_value = {**dict.fromkeys(RESULT_FIELDS), "status": "seed_source_generation_published",
                      "start": start, "source_runtime": runtime, "dataset_lineage": lineage}
        if stop_change:
            stop_change(stop_value)
        stop = self.write("stop.json", stop_value)
        return {"source_generation": stop, "source_export": {"source_generation": stop}}, {
            "source_generation": stop, "dataset_lineage": lineage,
        }

    def test_one_recursive_chain_is_readonly_and_returns_only_unique_authority(self):
        backup, expected = self.chain()
        before = {str(path): path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        self.assertEqual(proof._dataset_authority(self.backend, backup), expected)
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.root.rglob("*") if path.is_file()})

    def test_cross_generation_lineage_unknown_fields_and_changed_bytes_fail_closed(self):
        for name, change in (
            ("stop_change", lambda value: value.update(status="seed_source_generation_running")),
            ("stop_change", lambda value: value.update(extra=True)),
            ("start_change", lambda value: value.update(dataset_lineage={"different": True})),
            ("start_change", lambda value: value.update(extra=True)),
            ("runtime_change", lambda value: value.update(source_generation={"different": True})),
            ("runtime_change", lambda value: value.update(dataset_lineage={"different": True})),
            ("runtime_change", lambda value: value.update(extra=True)),
        ):
            with self.subTest(name=name):
                backup, _expected = self.chain(**{name: change})
                with self.assertRaises(ValueError):
                    proof._dataset_authority(self.backend, backup)
        backup, _expected = self.chain()
        backup["source_export"]["source_generation"] = {"different": True}
        with self.assertRaises(ValueError):
            proof._dataset_authority(self.backend, backup)
        backup, _expected = self.chain()
        (self.root / "lineage.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            proof._dataset_authority(self.backend, backup)

    def test_both_sides_use_runtime_registered_reference_instead_of_backup_target(self):
        authority, expected = self.chain()
        origin = {"target_side": "base", "target": {"scope_id": "original-backup-target"}}
        manifest = self.write("manifest.json", {"manifest": True})
        backup = self.write("backup.json", {
            "command": "backup", "plan_sha256": "a" * 64, "started_at": "unused",
            "status": "completed", "completed_at": "unused",
            "result": {"format_version": 2, "kind": "restore-reference-backup",
                       "reference_plan": origin, "manifest": manifest, "backup_root": str(self.root),
                       "artifacts": 1, **authority},
        })
        for side in ("base", "candidate"):
            reference = {"target_side": side, "target": {"scope_id": side}}
            value = {**dict.fromkeys(proof.TARGET_FIELDS), "backup_receipt": backup, "target_side": side}
            target = self.write("target.json", value)
            with patch.object(proof, "_runtime_reference", return_value=reference), \
                    patch.object(proof, "verify_target_plan", return_value=value) as verify:
                actual = proof._verified_target(self.backend, Path(target["path"]), {"runtime": True})
            verify.assert_called_once_with(self.backend, reference, Path(target["path"]))
            self.assertNotEqual(reference, origin)
            self.assertEqual(actual[2], reference)
            self.assertEqual(actual[4], expected)

    def test_runtime_reference_is_bound_to_registration_and_exact_target(self):
        target = proof.read_json_document(Path(self.write("target.json", {"target": True})["path"]))
        reference = proof.read_json_document(Path(self.write("reference.json", {"target_side": "candidate"})["path"]))
        registration = self.write("registration.json", {"reference_plan": proof._descriptor(reference)})
        launch = self.write("launch.json", {"request": {"registration": {
            "registration": registration, "target_plan": proof._descriptor(target),
        }}})
        runtime = {"paths": {"launch": launch["path"]}}
        with patch.object(proof, "validate_launch", side_effect=lambda value, _path: value), \
                patch.object(proof, "registration_binding", return_value=({}, {}, (None, reference, target))) as verify:
            self.assertEqual(proof._runtime_reference(self.backend, runtime, target), reference.value)
            verify.assert_called_once_with(self.backend, Path(registration["path"]), proof._descriptor(target))
        value = json.loads(Path(launch["path"]).read_text(encoding="utf-8"))
        value["request"]["registration"]["target_plan"] = {"different": True}
        self.write("launch.json", value)
        with patch.object(proof, "validate_launch", side_effect=lambda value, _path: value), \
                patch.object(proof, "registration_binding") as verify, self.assertRaisesRegex(ValueError, "目标计划"):
            proof._runtime_reference(self.backend, runtime, target)
        verify.assert_not_called()

    def test_preflight_returns_bound_projection_without_live_calls_or_file_changes(self):
        _backup, dataset = self.chain()
        runtime = proof.read_json_document(Path(self.write("runtime-input.json", {"runtime": True})["path"]))
        target = proof.read_json_document(Path(self.write("target.json", {"target": True})["path"]))
        selected = {"scope_id": "candidate", "api_url": "http://127.0.0.1:3200",
                    "frontend_url": "http://127.0.0.1:4200"}
        verified = {"product_execution": {"roots": {"execution_backend": str(self.backend)}}}
        before = {str(path): path.read_bytes() for path in self.root.iterdir()}
        with patch.object(proof, "repository", return_value=self.backend), \
                patch.object(proof, "validate_runtime_receipt", return_value={"runtime": True}), \
                patch.object(proof, "_verified_target", return_value=(target, verified, {"target": selected}, {}, dataset)) as check, \
                patch.object(proof, "verify_live_generation") as live:
            result = proof.dataset_preflight(self.backend, runtime.path, target.path)
        check.assert_called_once_with(self.backend, target.path, {"runtime": True})
        live.assert_not_called()
        self.assertEqual(result, {"format_version": 1, "kind": "restore-dataset-authority",
                                 "runtime": proof._descriptor(runtime), "target_plan": proof._descriptor(target),
                                 **dataset, "target": selected, "execution_backend": str(self.backend)})
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.root.iterdir()})

    def test_preflight_cli_emits_only_json_and_rejects_writing_or_ambiguous_options(self):
        args = ["restore_business_proof", "--preflight", "--backend-dir", str(self.backend),
                "--runtime-receipt", "runtime.json", "--target-plan", "target.json"]
        result = {"verified": True}
        output = io.StringIO()
        with patch.object(sys, "argv", args), patch.object(sys, "stdin", io.StringIO("must not read")), \
                patch.object(proof, "dataset_preflight", return_value=result) as preflight, contextlib.redirect_stdout(output):
            proof.main()
        self.assertEqual(output.getvalue(), json.dumps(result, separators=(",", ":")) + "\n")
        preflight.assert_called_once_with(self.backend, Path("runtime.json"), Path("target.json"))
        for change in (["--write"], ["--preflight"], ["--output", "new.json"], ["--proof", "proof.json"],
                       ["--runner-root", "runner"], ["--tests-receipt", "tests.json"]):
            with patch.object(sys, "argv", args + change), patch.object(proof, "dataset_preflight") as preflight, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                proof.main()
            self.assertEqual(error.exception.code, 2)
            preflight.assert_not_called()
        with patch.object(sys, "argv", [arg.replace("--preflight", "--prefl") for arg in args]), \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
            proof.main()
        self.assertEqual(error.exception.code, 2)

    def test_preflight_minimal_forged_chain_produces_no_authority(self):
        runtime = self.write("runtime-input.json", {"runtime": True})
        target = self.write("target.json", {"backup_receipt": {"source_generation": {"dataset_lineage": {}}}})
        args = ["restore_business_proof", "--preflight", "--backend-dir", str(self.backend),
                "--runtime-receipt", runtime["path"], "--target-plan", target["path"]]
        output = io.StringIO()
        before = {str(path): path.read_bytes() for path in self.root.iterdir()}
        with patch.object(sys, "argv", args), patch.object(proof, "repository", return_value=self.backend), \
                patch.object(proof, "_verified_target") as check, contextlib.redirect_stdout(output), \
                self.assertRaises(ValueError):
            proof.main()
        self.assertEqual(output.getvalue(), "")
        check.assert_not_called()
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.root.iterdir()})


if __name__ == "__main__":
    unittest.main()
