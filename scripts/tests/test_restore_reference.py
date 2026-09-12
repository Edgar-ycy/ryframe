import copy
import contextlib
import io
import json
import subprocess
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_reference as reference
import restore_reference_execution as execution
from restore_reference_plan import dataset_timeout_seconds, safe_file, validate_inventory, validate_plan, verify_artifacts
from restore_reference_fixture import environment, inventory, restore_record, stored_backup
from restore_reference_execution_fixture import guards


class ReferenceTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        guards(self)

    def restore(self, tools, root, manifest, record):
        reference.write_json(root / "manifest.json", manifest, new=False)
        def descriptor(path):
            return {"path": str(path), **reference.file_digest(path)}
        work = Path(self.plan["work_dir"])
        receipt = work / "backup.json"
        reference.write_json(receipt, {"result": {"backup_root": str(root), "manifest": descriptor(root / "manifest.json")}}, new=False)
        target_path, record_path = work / "target-plan.json", work / "record.json"
        target = {"target_side": self.plan["target_side"], "backup_receipt": descriptor(receipt), "product_plan": record["plan"],
                  "fresh_target": {"initialized": {"path": str(work / "fresh/target/initialized.json")}}}
        reference.write_json(target_path, target, new=False)
        reference.write_json(record_path, record, new=False)
        registration = work / "registration.json"
        reference.write_json(registration, {"target_plan": descriptor(target_path)}, new=False)
        with patch.object(execution, "verify_target_plan", return_value=target):
            inputs = execution.restore_inputs(self.backend, self.plan, target_path, root, record_path, registration, read_only=True)
            return reference.restore(self.plan, tools, self.backend, inputs)

    def test_plan_rejects_overlap_wrong_modes_remote_endpoints_and_changed_tools(self):
        validate_plan(self.plan, self.backend)
        changes = [
            lambda value: value.pop("target_side"),
            lambda value: value.update(target_side="source"),
            lambda value: value["target"].update(scope_id="source"),
            lambda value: value["target"]["databases"][0].update(database="source_control"),
            lambda value: value["target"]["databases"][0].update(key="unknown"),
            lambda value: value["target"]["databases"][1].update(mode="shared"),
            lambda value: value["source"]["s3"].update(endpoint="https://example.com"),
            lambda value: value.update(work_dir=str(self.backend)),
            lambda value: value["tools"]["mysql"].update(sha256="a" * 64),
        ]
        for change in changes:
            plan = copy.deepcopy(self.plan)
            change(plan)
            with self.assertRaises((ValueError, OSError)):
                validate_plan(plan, self.backend)

    def test_plan_ids_and_scopes_use_the_formal_identifier_boundaries(self):
        for identifier in ("a", "a" + "_" * 63):
            candidate = copy.deepcopy(self.plan)
            candidate["id"] = identifier
            validate_plan(candidate, self.backend)
        for identifier in ("", "_restore", "-restore", "Restore", "restore.one", "a" * 65):
            candidate = copy.deepcopy(self.plan)
            candidate["id"] = identifier
            with self.subTest(identifier=identifier), self.assertRaises(ValueError):
                validate_plan(candidate, self.backend)

        for scope in ("a1", "a" + "_" * 46 + "z"):
            candidate = copy.deepcopy(self.plan)
            candidate["target"]["scope_id"] = scope
            validate_plan(candidate, self.backend)
        for scope in ("", "a", "_target", "-target", "target_", "target-", "Target", "target.one", "a" * 49):
            candidate = copy.deepcopy(self.plan)
            candidate["target"]["scope_id"] = scope
            with self.subTest(scope=scope), self.assertRaises(ValueError):
                validate_plan(candidate, self.backend)

    def test_safe_files_reject_escape_missing_absolute_and_link(self):
        good = self.backend / "good"
        good.write_text("data")
        self.assertEqual(safe_file(self.backend, "good"), good)
        for name in ("../escape", "a/../../escape", "D:/escape", "/escape", "a\\b", "a//b", "missing"):
            with self.assertRaises(ValueError):
                safe_file(self.backend, name)
        with patch.object(Path, "is_symlink", return_value=True):
            with self.assertRaisesRegex(ValueError, "链接"):
                safe_file(self.backend, "good")

    def test_inventory_requires_complete_physical_targets_and_exact_object_scope(self):
        value = inventory(self.plan)
        validate_inventory(self.plan, value)
        changes = [
            lambda value: value.update(id="another-backup"),
            lambda value: value.update(id="Backup"),
            lambda value: value["databases"].pop(),
            lambda value: value["databases"][0].update(database="other"),
            lambda value: value["databases"][0].update(shared=False),
            lambda value: value["databases"][0]["tables"][0].update(table="ryframe_resource_ownership"),
            lambda value: value["objects"][0]["entries"].append({"key": "source/.ryframe-owner"}),
            lambda value: value["objects"][0]["entries"].append({"key": "another/file"}),
        ]
        for change in changes:
            candidate = copy.deepcopy(value)
            change(candidate)
            with self.assertRaises(ValueError):
                validate_inventory(self.plan, candidate)

    def test_reference_work_requires_same_plan_owner_and_explicit_write(self):
        path = self.backend / "plan.json"
        path.write_text(json.dumps(self.plan))
        with patch.object(sys, "argv", ["restore_reference", "dataset", "--plan", str(path), "--backend-dir", str(self.backend)]):
            with self.assertRaises(SystemExit) as error:
                reference.main()
        self.assertEqual(error.exception.code, 2)
        self.assertFalse(Path(self.plan["work_dir"]).exists())
        reference.work_directory(self.plan)
        changed = copy.deepcopy(self.plan)
        changed["id"] = "another"
        with self.assertRaisesRegex(ValueError, "ownership"):
            reference.work_directory(changed)

    def test_new_copy_damage_keeps_original_intact_and_detects_missing_and_corrupt(self):
        work = reference.work_directory(self.plan)
        root, manifest = stored_backup(self.plan, work)
        for missing in (False, True):
            copied = reference.copy_backup(work, manifest, root, f"bad-{int(missing)}")
            bad = Path(copied["backup_root"])
            copied_manifest = reference.read_json(bad / "manifest.json")
            reference.damage(work, bad, copied_manifest, "databases/control.sql", missing)
            with self.assertRaises(ValueError):
                verify_artifacts(bad, copied_manifest)
            verify_artifacts(root, manifest)
        with self.assertRaises(ValueError):
            reference.damage(work, root, manifest, "databases/control.sql", False)
        with self.assertRaises(ValueError):
            reference.copy_backup(work, manifest, root, manifest["id"])

    def test_invalid_artifact_stops_restore_before_any_external_call(self):
        work = reference.work_directory(self.plan)
        root, manifest = stored_backup(self.plan, work)
        tools = Mock()
        (root / "databases/control.sql").write_text("tampered")
        with self.assertRaises(ValueError):
            self.restore(tools, root, manifest, restore_record(self.plan, manifest["captured_at"]))
        self.assertEqual(tools.mock_calls, [])

    def test_restore_rejects_backup_swap_and_invalid_record_id_before_external_call(self):
        work = reference.work_directory(self.plan)
        root, manifest = stored_backup(self.plan, work)
        changes = [
            lambda changed_manifest, _record: changed_manifest.update(id="another-backup"),
            lambda _manifest, record: record["plan"].update(id="_restore"),
            lambda _manifest, record: record["plan"].update(backup_id="backup.one"),
        ]
        for change in changes:
            changed_manifest = copy.deepcopy(manifest)
            record = restore_record(self.plan, manifest["captured_at"])
            change(changed_manifest, record)
            tools = Mock()
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.restore(tools, root, changed_manifest, record)
            self.assertEqual(tools.mock_calls, [])

    def test_valid_restore_preserves_target_owner_and_rewrites_only_physical_object_scope(self):
        work = reference.work_directory(self.plan)
        root, manifest = stored_backup(self.plan, work)
        tools = Mock()
        result = self.restore(tools, root, manifest, restore_record(self.plan, manifest["captured_at"]))
        self.assertEqual(result["status"], "external_copy_completed")
        self.assertEqual(set(result["resource_images"]), {"before", "after"})
        self.assertEqual(self.events[0], "lock")
        self.assertEqual(self.events[-1], "unlock")
        self.assertEqual(tools.restore_database.call_count, 2)
        self.assertEqual(tools.aws.call_args.args[:4], ("target", "put-object", "uploads", "target/tenant/file.txt"))
        for call in tools.restore_database.call_args_list:
            self.assertEqual(call.args[1], ["sys_post"])

    def test_restore_requires_running_record_same_physical_targets_and_stopped_processes(self):
        manifest = inventory(self.plan)
        for change in (lambda value: value.update(status="succeeded"),
                       lambda value: value["plan"]["databases"][0].update(database="source_control"),
                       lambda value: value.update(started_at="2000-01-01T00:00:00Z")):
            record = restore_record(self.plan, manifest["captured_at"])
            change(record)
            with self.assertRaises(ValueError):
                execution.validate_restore_record(self.plan, manifest, record, record["plan"])
        with patch.object(reference, "read_process", return_value={"pid": 1}), patch.object(reference, "process_identity", return_value={"pid": 1}):
            with self.assertRaisesRegex(ValueError, "必须停止"):
                reference.require_stopped(self.plan, "source")
        with patch.object(reference, "read_process", return_value={"pid": 1}), patch.object(reference, "process_identity", return_value=None):
            reference.require_stopped(self.plan, "source")

    def test_source_with_existing_tenant_cannot_be_seeded(self):
        tools = Mock()
        tools.mysql.return_value = "1"
        with self.assertRaisesRegex(ValueError, "没有普通租户"):
            reference.require_empty_source(self.plan, tools)
        tools.mysql.return_value = "0"
        reference.require_empty_source(self.plan, tools)

    def test_existing_check_defaults_to_target_and_accepts_explicit_source_without_empty_check(self):
        reference.work_directory(self.plan)
        path = self.backend / "plan.json"
        path.write_text(json.dumps(self.plan))
        original = path.read_bytes()
        for options, side in (([], "target"), (["--side", "source"], "source"),
                              (["--side", "target"], "target")):
            output = io.StringIO()
            argv = ["restore_reference", "check-existing", "--plan", str(path),
                    "--backend-dir", str(self.backend), *options]
            with patch.object(sys, "argv", argv), contextlib.redirect_stdout(output), \
                    patch.object(reference, "ExternalTools"), patch.object(reference, "check_api") as check, \
                    patch.object(reference, "require_empty_source") as empty:
                reference.main()
            self.assertEqual(check.call_args.args[2], side)
            empty.assert_not_called()
            self.assertEqual(json.loads(output.getvalue()), {"plan_sha256": reference.plan_hash(self.plan),
                                                            "side": side, "scope_id": self.plan[side]["scope_id"]})
        self.assertEqual(path.read_bytes(), original)

    def test_side_is_rejected_for_every_non_existing_command_before_resource_access(self):
        for command, side in (("check-existing", "other"), ("check-dataset", "source"),
                              ("dataset", "source"), ("restore", "source"), ("plan", "target"),
                              ("backup", "source"), ("copy", "target"), ("damage", "target")):
            argv = ["restore_reference", command, "--plan", "unused.json", "--backend-dir", str(self.backend),
                    "--side", side]
            with patch.object(sys, "argv", argv), patch.object(reference, "read_json") as read, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                reference.main()
            self.assertEqual(error.exception.code, 2)
            read.assert_not_called()

    def test_dataset_check_stays_source_and_requires_empty_source(self):
        reference.work_directory(self.plan)
        path = self.backend / "plan.json"
        path.write_text(json.dumps(self.plan))
        argv = ["restore_reference", "check-dataset", "--plan", str(path), "--backend-dir", str(self.backend)]
        with patch.object(sys, "argv", argv), contextlib.redirect_stdout(io.StringIO()), \
                patch.object(reference, "ExternalTools"), patch.object(reference, "check_api") as check, \
                patch.object(reference, "require_empty_source") as empty:
            reference.main()
        self.assertEqual(check.call_args.args[2], "source")
        empty.assert_called_once()

    def test_dataset_requires_explicit_bounded_stage_timeout(self):
        self.plan["dataset"] = {"timeout_seconds": 21600}
        self.assertEqual(dataset_timeout_seconds(self.plan), 21600)
        for value in (None, 0, -1, 604801, True, 1.5, "21600"):
            self.plan["dataset"]["timeout_seconds"] = value
            with self.assertRaisesRegex(ValueError, "阶段时限"):
                dataset_timeout_seconds(self.plan)

    def test_dataset_uses_long_stage_timeout_and_preserves_timeout_failure(self):
        self.plan["dataset"] = {"timeout_seconds": 21600}
        work = reference.work_directory(self.plan)
        tools = Mock()
        tools.command.return_value = ["node"]
        tools.execute.side_effect = subprocess.TimeoutExpired(["node"], 21600)
        args = SimpleNamespace(command="dataset", copy_id=None, plan=self.backend / "plan.json")
        with patch.object(reference, "source_snapshot", return_value={"head": "a" * 40, "clean": False}), \
                patch.object(reference, "check_api"), patch.object(reference, "require_empty_source"):
            with self.assertRaises(subprocess.TimeoutExpired):
                reference.execute(args, self.plan, self.backend, tools, work)
        self.assertEqual(tools.execute.call_args.kwargs["timeout"], 21600)
        command = tools.execute.call_args.args[0]
        self.assertEqual(len(command), 2)
        protocol = json.loads(
            tools.execute.call_args.kwargs["env"][reference.DATASET_PROTOCOL_ENV]
        )
        self.assertEqual(protocol["kind"], reference.DATASET_PROTOCOL_KIND)
        self.assertTrue(protocol["write"])
        self.assertEqual(protocol["side"], "source")
        self.assertIsNone(protocol["verify_existing"])
        preflight = reference.read_json(Path(protocol["preflight"]))
        self.assertEqual(preflight["plan_sha256"], reference.plan_hash(self.plan))
        self.assertEqual(preflight["scope_id"], self.plan["source"]["scope_id"])
        self.assertEqual(reference.read_json(work / "dataset.json")["status"], "failed")

    def test_called_process_failure_records_redacted_output(self):
        error = subprocess.CalledProcessError(1, ["node"], output="normal", stderr="password=private")
        with patch.dict(reference.os.environ, {"APP_PASSWORD": "private"}, clear=True):
            result = reference.failure_diagnostic(error)
        self.assertEqual(result["error_type"], "CalledProcessError")
        self.assertEqual(result["stdout"], "normal")
        self.assertEqual(result["stderr"], "password=[REDACTED]")


if __name__ == "__main__":
    unittest.main()
