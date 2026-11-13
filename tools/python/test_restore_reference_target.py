"""严格目标计划的来源关系和失败关闭回归。"""

import copy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_reference as reference
import restore_reference_target as target
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

    def test_reference_plan_extra_fields_and_nested_unknowns_fail_closed(self):
        original = copy.deepcopy(self.plan)
        for change in (lambda value: value.update(extra=True), lambda value: value["target"].update(extra=True),
                       lambda value: value["tools"]["mysql"].update(extra=True),
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


if __name__ == "__main__":
    unittest.main()
