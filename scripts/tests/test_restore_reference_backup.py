"""正式备份的共享导出绑定；所有来源与外部命令均使用离线替身。"""

import contextlib
import copy
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_reference as reference
import restore_reference_backup as backup
from restore_reference_fixture import environment, inventory


class BackupBindingTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = reference.work_directory(self.plan)
        self.inventory = inventory(self.plan)
        self.build = {"sources": {"full": {"source": {"snapshot": {"head": "a" * 40, "clean": True}}}}}
        self.runtime = {"build": self.build, "plan_sha256": reference.plan_hash(self.plan),
                        "runtime": {"scope_id": "source"}, "processes": {"api": {"pid": 20}},
                        "physical_binding": {"scope_id": "source"}}
        self.runtime_path = self.write("runtime.json", self.runtime)
        self.quiescence_path = self.write("quiescence.json", {
            "source_runtime_sha256": reference.file_digest(self.runtime_path)["sha256"]})
        self.export_path = self.write("source-export-result.json", {"status": "seed_source_export_published"})
        build_path = self.write("build.json", self.build)
        self.identity = {"result": self.descriptor(self.export_path), "export": {"sha256": "e" * 64}}
        self.exported = {
            "inventory": {key: value for key, value in self.inventory.items() if key != "id"},
            "request": {"source": {key: value for key, value in self.plan["source"].items() if key != "frontend_url"},
                        "tools": self.plan["tools"], "backend_build": self.descriptor(build_path)},
            "generation": {"source": self.build["sources"]["full"]["source"]["snapshot"],
                           **{key: self.runtime[key] for key in ("runtime", "processes", "physical_binding")}},
        }

    def descriptor(self, path):
        return {"path": str(path), **reference.file_digest(path)}

    def write(self, name, value):
        path = self.work / name
        reference.write_json(path, value, new=False)
        return path

    def verify(self):
        with patch.object(backup, "_source_export", return_value=self.identity), \
                patch.object(backup, "verify_source_export", return_value=self.exported):
            return backup.backup_source(self.backend, self.plan, self.inventory,
                                        self.runtime_path, self.quiescence_path, self.export_path)

    def test_binds_same_published_export_and_both_source_proofs(self):
        self.assertEqual(self.verify(), {"source_export": self.identity,
                                       "source_runtime": self.descriptor(self.runtime_path),
                                       "source_quiescence": self.descriptor(self.quiescence_path)})

    def test_rejects_different_inventory_configuration_build_and_generation(self):
        original = copy.deepcopy(self.exported)
        changes = [lambda value: value["inventory"].update(source_sha="b" * 40),
                   lambda value: value["inventory"]["databases"][0]["tables"][0].update(rows=2),
                   lambda value: value["request"]["source"].update(scope_id="other"),
                   lambda value: value["request"]["tools"]["aws"].update(sha256="d" * 64),
                   lambda value: value["request"]["backend_build"].update(sha256="d" * 64),
                   lambda value: value["generation"]["source"].update(head="c" * 40),
                   lambda value: value["generation"]["processes"]["api"].update(pid=21),
                   lambda value: value["generation"]["runtime"].update(scope_id="other"),
                   lambda value: value["generation"]["physical_binding"].update(scope_id="other")]
        for change in changes:
            self.exported = copy.deepcopy(original)
            change(self.exported)
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.verify()

    def test_rejects_stale_quiescence_and_mutation_during_verification(self):
        self.write("quiescence.json", {"source_runtime_sha256": "f" * 64})
        with self.assertRaises(ValueError):
            self.verify()
        self.write("quiescence.json", {"source_runtime_sha256": self.descriptor(self.runtime_path)["sha256"]})
        def mutate(*_args):
            self.write("source-export-result.json", {"changed": True})
            return self.exported
        with patch.object(backup, "_source_export", return_value=self.identity), \
                patch.object(backup, "verify_source_export", side_effect=mutate), self.assertRaises(ValueError):
            backup.backup_source(self.backend, self.plan, self.inventory, self.runtime_path,
                                 self.quiescence_path, self.export_path)

    def test_backup_requires_export_argument_before_any_resource_access(self):
        path = self.write("plan.json", self.plan)
        argv = ["restore_reference", "backup", "--plan", str(path), "--backend-dir", str(self.backend),
                "--inventory", "unused", "--source-runtime", "unused", "--source-quiescence", "unused", "--write"]
        with patch.object(sys, "argv", argv), patch.object(reference, "work_directory") as work, \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            reference.main()
        work.assert_not_called()

    def test_unpublished_export_is_rejected_before_external_backup_or_output(self):
        tools = Mock()
        with patch.object(reference, "verify_stopped_source", return_value={}), \
                patch.object(backup, "_source_export", side_effect=ValueError("未发布")), \
                self.assertRaisesRegex(ValueError, "未发布"):
            reference.backup(self.plan, tools, self.work, self.inventory, self.backend,
                             self.runtime_path, self.quiescence_path, self.export_path)
        self.assertEqual(tools.mock_calls, [])
        self.assertFalse((self.work / "backup").exists())

    def test_backup_publishes_manifest_binding_and_refuses_changed_export(self):
        tools = Mock()
        tools.dump.side_effect = lambda _db, _tables, path: path.write_text("fixture")
        inputs = self.verify()
        with patch.object(reference, "verify_stopped_source", return_value={}), \
                patch.object(reference, "backup_source", return_value=inputs), \
                patch.object(reference, "require_stopped"), patch.object(reference, "verify_artifacts"):
            result = reference.backup(self.plan, tools, self.work, self.inventory, self.backend,
                                      self.runtime_path, self.quiescence_path, self.export_path)
        backup.validate_backup_result(result)
        self.assertEqual(result["source_export"], self.identity)
        self.assertEqual(result["manifest"], self.descriptor(self.work / "backup/manifest.json"))
        self.assertEqual(result["reference_plan"], self.plan)
        for change in (lambda value: value.update(extra=True), lambda value: value.pop("source_export"),
                       lambda value: value.update(format_version=True)):
            invalid = copy.deepcopy(result)
            change(invalid)
            with self.assertRaises(ValueError):
                backup.validate_backup_result(invalid)


if __name__ == "__main__":
    unittest.main()
