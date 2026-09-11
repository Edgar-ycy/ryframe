"""严格目标计划的来源关系、不可覆盖发布和失败关闭回归。"""

import contextlib
import copy
import io
import json
import os
from pathlib import Path
import sys
import subprocess
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_reference as reference
import restore_reference_target as target
import restore_reference_target_cli as cli
from restore_reference_target_fixture import setup


class TargetPlanTests(unittest.TestCase):
    def setUp(self):
        setup(self)

    def write(self, name, value):
        path = self.work / name
        path.parent.mkdir(parents=True, exist_ok=True)
        reference.write_json(path, value, new=False)
        return path

    def descriptor(self, path):
        return {"path": str(path), **reference.file_digest(path)}

    def write_binding(self, name, value):
        return self.descriptor(self.write(name, value))

    def capture(self):
        return target.capture_target_plan(self.backend, self.plan, **self.paths)

    def test_both_sides_bind_one_backup_and_the_corresponding_comparison_arm(self):
        base = self.capture()
        self.assertEqual(base["comparison_arm"], "b0")
        self.assertEqual(base["product_plan"], self.product)
        self.assertEqual(base["maintenance_execution"], {"root": str(self.backend), "binding": self.maintenance_binding,
                                                        "build": self.request["maintenance_build"]})
        self.assertEqual(base["product_execution"], target._product_execution(self.comparison["arms"]["b0"]))
        self.assertNotEqual(base["maintenance_execution"]["root"], base["product_execution"]["roots"]["execution_backend"])
        self.assertEqual(base["fresh_target"]["ownership_sha256"], target._ownership(self.arm))
        self.assertEqual(target.verify_target_plan(self.backend, self.plan, self.write("target-plan.json", base)), base)
        self.plan["target_side"] = self.arm["target_side"] = self.request["side"] = "candidate"
        self.product.update(frontend_sha="c" * 40, id="restore-run-candidate")
        self.write("product.json", self.product)
        candidate = self.capture()
        self.assertEqual(candidate["comparison_arm"], "b1")
        self.assertEqual(candidate["backup_receipt"], base["backup_receipt"])
        self.assertEqual(candidate["maintenance_execution"], base["maintenance_execution"])
        self.assertEqual(candidate["product_execution"], target._product_execution(self.comparison["arms"]["b1"]))
        self.assertNotEqual(candidate["comparison_arm_sha256"], base["comparison_arm_sha256"])

    def test_unknown_fields_and_missing_bindings_are_rejected(self):
        original = self.capture()
        changes = [lambda value: value.update(extra=True), lambda value: value.pop("backup_receipt"),
                   lambda value: value["fresh_target"].update(extra=True),
                   lambda value: value["backup_receipt"].update(extra=True),
                   lambda value: value.update(format_version=True),
                   lambda value: value.update(product_plan_sha256="f" * 64),
                   lambda value: value.update(comparison_arm_sha256="f" * 64),
                   lambda value: value["fresh_target"].update(ownership_sha256="f" * 64)]
        for change in changes:
            value = copy.deepcopy(original)
            change(value)
            with self.subTest(change=change), self.assertRaises(ValueError):
                target.verify_target_plan(self.backend, self.plan, self.write("invalid.json", value))

    def test_all_bound_files_reject_byte_drift_before_reconstruction(self):
        value = self.capture()
        plan_path = self.write("target-plan.json", value)
        descriptors = [value[key] for key in ("backup_receipt", "comparison_sources", "arm_input", "product_plan_file")]
        descriptors += [value["fresh_target"][key] for key in target.FRESH_FIELDS - {"ownership_sha256"}]
        descriptors += [value["maintenance_execution"]["build"], *value["product_execution"]["builds"].values()]
        for descriptor in descriptors:
            path = Path(descriptor["path"])
            before = path.read_bytes()
            path.write_bytes(before + b"\n")
            try:
                with self.subTest(path=path), patch.object(target, "capture_target_plan") as capture, self.assertRaises(ValueError):
                    target.verify_target_plan(self.backend, self.plan, plan_path)
                capture.assert_not_called()
            finally:
                path.write_bytes(before)

    def test_side_or_arm_swap_is_rejected(self):
        original = self.capture()
        for field, value in (("target_side", "candidate"), ("comparison_arm", "b1")):
            changed = {**original, field: value}
            with self.assertRaises(ValueError):
                target.verify_target_plan(self.backend, self.plan, self.write("invalid.json", changed))
        self.arm["target_side"] = "candidate"
        with self.assertRaises(ValueError):
            self.capture()

    def test_different_export_is_rejected_even_when_backup_id_matches(self):
        self.comparison["source_export"]["result"]["sha256"] = "f" * 64
        self.write("comparison.json", self.comparison)
        with self.assertRaisesRegex(ValueError, "共享导出"):
            self.capture()

    def test_arm_cannot_substitute_another_export_or_maintenance_backend(self):
        original = copy.deepcopy(self.arm)
        for key in ("source_export", "source_export_result"):
            self.arm = copy.deepcopy(original)
            self.arm["result"][key]["sha256"] = "e" * 64
            with self.assertRaises(ValueError):
                self.capture()
        self.arm = original
        with patch.object(target, "execution_backend", return_value=(self.backend / "other", {})), self.assertRaises(ValueError):
            self.capture()

    def test_execution_authorities_cannot_be_swapped_or_extended(self):
        original = self.capture()
        changes = [lambda value: value["maintenance_execution"].update(extra=True),
                   lambda value: value["maintenance_execution"].update(root=str(self.backend / "other")),
                   lambda value: value["maintenance_execution"]["binding"].update(kind="other"),
                   lambda value: value["product_execution"].update(extra=True),
                   lambda value: value["product_execution"].update(backend_execution_sha="f" * 40),
                   lambda value: value["product_execution"].update(backend_product_sha="f" * 40),
                   lambda value: value["product_execution"].update(adapter=None),
                   lambda value: value["product_execution"]["roots"].update(execution_backend=str(self.backend)),
                   lambda value: value.update(product_execution=target._product_execution(self.comparison["arms"]["b1"]))]
        for change in changes:
            value = copy.deepcopy(original)
            change(value)
            with self.subTest(change=change), self.assertRaises(ValueError):
                target.verify_target_plan(self.backend, self.plan, self.write("invalid-execution.json", value))

    def test_product_plan_rejects_extra_fields_sha_target_and_endpoint_drift(self):
        changes = [lambda value: value.update(extra=True), lambda value: value.update(frontend_sha="e" * 40),
                   lambda value: value.update(backup_id="other"), lambda value: value.update(scope_id="other"),
                   lambda value: value.update(object_prefix="source/"), lambda value: value.update(fault_at="today"),
                   lambda value: value.update(worker_ready_url=value["api_ready_url"]),
                   lambda value: value["databases"][0].update(database="other"),
                   lambda value: value["databases"][0].update(extra=True), lambda value: value["databases"].reverse()]
        for change in changes:
            product = copy.deepcopy(self.product)
            change(product)
            self.write("product.json", product)
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.capture()

    def test_product_fault_time_must_survive_rust_serialization_unchanged(self):
        for value in ("2026-09-05T00:00:00Z", "2026-09-05T00:00:00.100Z", "2026-09-05T00:00:00.123450Z"):
            self.write("product.json", {**self.product, "fault_at": value})
            self.assertEqual(self.capture()["product_plan"]["fault_at"], value)
        for value in ("2026-09-05T00:00:00+00:00", "2026-09-05T08:00:00+08:00",
                      "2026-09-05T00:00:00.1Z", "2026-09-05T00:00:00.100000Z",
                      "2026-09-05T00:00:00.000Z", "2026-09-05T00:00:00.1234567Z"):
            self.write("product.json", {**self.product, "fault_at": value})
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "Rust 原生"):
                self.capture()

    def test_original_backup_receipt_and_owner_are_strict(self):
        original = reference.read_json(self.paths["backup_receipt"])
        for change in (lambda value: value.update(status="failed"), lambda value: value.update(extra=True),
                       lambda value: value["result"].update(extra=True),
                       lambda value: value["result"].pop("source_export"),
                       lambda value: value["result"]["manifest"].update(sha256="f" * 64),
                       lambda value: value["result"]["reference_plan"]["source"].update(scope_id="other")):
            value = copy.deepcopy(original)
            change(value)
            self.write("backup.json", value)
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.capture()
        self.write("backup.json", original)
        self.write("reference-owner.json", {"wrong": True})
        with self.assertRaisesRegex(ValueError, "ownership"):
            self.capture()

    def test_fresh_verify_requires_same_initialized_complete_before_image_and_no_failure(self):
        for change in (lambda value: value.update(status="failed"), lambda value: value.update(extra=True),
                       lambda value: value.update(remote_writes=True),
                       lambda value: value["initialized"].update(sha256="f" * 64),
                       lambda value: value["inventory"]["observations"].pop(next(iter(value["inventory"]["observations"]))),
                       lambda value: value["inventory"].update(extra=True)):
            value = copy.deepcopy(self.fresh)
            change(value)
            self.write("observe/verify.json", value)
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.capture()
        self.write("observe/verify.json", self.fresh)
        self.write("observe/inventory/failure.json", {"failed": True})
        with self.assertRaises(ValueError):
            self.capture()

    def test_ownership_and_raw_inventory_changes_are_rejected(self):
        original = copy.deepcopy(self.arm)
        for change in (lambda value: value["initialized"]["objects"]["uploads"]["owner"].update(sha256="e" * 64),
                       lambda value: value["initialized"]["objects"]["uploads"]["keys"].append("target/unknown"),
                       lambda value: value["initialized"]["inventory"]["observations"]["target_0"]["ownership"][0].update(scope_id="other")):
            self.arm = copy.deepcopy(original)
            change(self.arm)
            with self.assertRaises(ValueError):
                self.capture()
        self.arm = original
        self.write("observe/inventory/before-target-shared.json", {"tampered": True})
        with self.assertRaises(ValueError):
            self.capture()

    def test_reference_plan_extra_fields_and_nested_unknowns_fail_closed(self):
        original = copy.deepcopy(self.plan)
        for change in (lambda value: value.update(extra=True), lambda value: value["target"].update(extra=True),
                       lambda value: value["tools"]["aws"].update(extra=True),
                       lambda value: value["target"]["s3"].update(extra=True),
                       lambda value: value["target"]["databases"][0].update(extra=True)):
            self.plan = copy.deepcopy(original)
            change(self.plan)
            with self.assertRaises(ValueError):
                self.capture()

    def test_duplicate_json_fields_and_link_or_reparse_are_rejected(self):
        value = self.capture()
        path = self.write("target-plan.json", value)
        path.write_text('{"format_version": 1, "format_version": 1}', encoding="utf-8")
        with self.assertRaises(ValueError):
            target.verify_target_plan(self.backend, self.plan, path)
        with patch("restore_runtime_evidence.is_reparse", return_value=True), self.assertRaises(ValueError):
            self.capture()
        with patch("devex_clone_model.linked", return_value=True), self.assertRaises(ValueError):
            target.plan_output(self.backend, self.work / "new.json", value)

    def test_new_plan_output_rejects_collision_and_protected_artifact_directories(self):
        value = self.capture()
        for path in (self.paths["backup_receipt"], self.paths["product_plan"], self.work / "backup/new.json",
                     self.work / "fresh/target/new.json", self.backend / "outside.json"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                target.plan_output(self.backend, path, value)

    def test_plan_preview_is_read_only_and_publication_is_explicit_and_exclusive(self):
        path = self.write("reference-plan.json", self.plan)
        argv = ["restore_reference", "plan", "--backend-dir", str(self.backend), "--plan", str(path)]
        for key, value in self.paths.items():
            argv += ["--" + key.replace("_", "-"), str(value)]
        before = {str(item): item.read_bytes() for item in self.work.rglob("*") if item.is_file()}
        with patch.object(sys, "argv", argv), contextlib.redirect_stdout(io.StringIO()) as output:
            reference.main()
        self.assertEqual(json.loads(output.getvalue())["comparison_arm"], "b0")
        self.assertEqual(before, {str(item): item.read_bytes() for item in self.work.rglob("*") if item.is_file()})
        destination = self.work / "published-target-plan.json"
        with patch.object(sys, "argv", [*argv, "--output", str(destination), "--write"]), contextlib.redirect_stdout(io.StringIO()):
            reference.main()
        self.assertTrue(destination.is_file())
        with patch.object(sys, "argv", [*argv, "--output", str(destination), "--write"]), \
                patch.object(cli, "capture_target_plan") as capture, self.assertRaises(ValueError):
            reference.main()
        capture.assert_not_called()

    def test_partial_or_mixed_plan_arguments_fail_before_reading_input(self):
        for command, extra in (("plan", ["--arm-input", "unused"]), ("plan", ["--write"]),
                               ("plan", ["--output", "unused"]), ("dataset", ["--target-plan", "unused"]),
                               ("restore", ["--arm-input", "unused"])):
            argv = ["restore_reference", command, "--backend-dir", str(self.backend), "--plan", "unused", *extra]
            with patch.object(sys, "argv", argv), patch.object(reference, "read_json") as read, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                reference.main()
            read.assert_not_called()

    def test_plan_cli_with_cold_bytecode_prefix_does_not_create_any_file(self):
        plan = self.write("reference-plan.json", self.plan)
        cache = self.work / "cold-bytecode"
        before = {str(path): (path.stat().st_size, path.stat().st_mtime_ns) for path in self.backend.rglob("*")}
        result = subprocess.run([sys.executable, "-B", str(Path(reference.__file__).absolute()), "plan",
            "--backend-dir", str(self.backend), "--plan", str(plan)], cwd=self.backend,
            env={**os.environ, "PYTHONPYCACHEPREFIX": str(cache)}, capture_output=True, check=True)
        self.assertEqual(json.loads(result.stdout)["id"], self.plan["id"])
        self.assertFalse(cache.exists())
        self.assertEqual(before, {str(path): (path.stat().st_size, path.stat().st_mtime_ns) for path in self.backend.rglob("*")})


if __name__ == "__main__":
    unittest.main()
