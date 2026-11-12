import copy
import contextlib
import io
import json
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_reference as reference
import restore_reference_execution as execution
from restore_reference_plan import safe_file, validate_inventory, validate_plan, verify_artifacts
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

    def test_plan_rejects_inline_s3_credentials_and_session_tokens_without_echoing(self):
        validate_plan(self.plan, self.backend)
        for field in ("access_key", "secret_key", "session_token", "security_token"):
            plan=copy.deepcopy(self.plan)
            plan["source"]["s3"][field]="private-plan-value"
            with self.assertRaises(ValueError) as error:
                validate_plan(plan,self.backend)
            self.assertNotIn("private-plan-value",str(error.exception))
        encoded=json.dumps(self.plan)
        self.assertNotIn("access-fixture",encoded)
        self.assertNotIn("secret-fixture",encoded)

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
        self.assertEqual(tools.create_object_if_absent.call_args.args[:2],
                         ("uploads", "target/tenant/file.txt"))
        for call in tools.restore_database.call_args_list:
            _, tables, path, expected = call.args
            self.assertEqual(tables, ["sys_post"])
            artifact = next(item for item in manifest["artifacts"]
                            if item["relative_path"] == path.relative_to(root).as_posix())
            self.assertEqual(expected, {key: artifact[key] for key in ("bytes", "sha256")})

    def test_restore_requires_running_record_and_same_physical_targets(self):
        manifest = inventory(self.plan)
        for change in (lambda value: value.update(status="succeeded"),
                       lambda value: value["plan"]["databases"][0].update(database="source_control"),
                       lambda value: value.update(started_at="2000-01-01T00:00:00Z")):
            record = restore_record(self.plan, manifest["captured_at"])
            change(record)
            with self.assertRaises(ValueError):
                execution.validate_restore_record(self.plan, manifest, record, record["plan"])

    def test_called_process_failure_records_redacted_output(self):
        error = subprocess.CalledProcessError(1, ["node"], output="normal", stderr="password=private")
        with patch.dict(reference.os.environ, {"APP_PASSWORD": "private"}, clear=True):
            result = reference.failure_diagnostic(error)
        self.assertEqual(result["error_type"], "CalledProcessError")
        self.assertEqual(result["stdout"], "normal")
        self.assertEqual(result["stderr"], "password=[REDACTED]")


if __name__ == "__main__":
    unittest.main()
