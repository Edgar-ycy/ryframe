import copy
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from restore_reference_plan import safe_file, validate_inventory, validate_plan
from restore_reference_fixture import environment, inventory


class ReferenceTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)

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


if __name__ == "__main__":
    unittest.main()
