"""正式备份的共享导出绑定；所有来源与外部命令均使用离线替身。"""

import contextlib
import copy
from datetime import datetime, timedelta
import io
from pathlib import Path
import sys
from types import SimpleNamespace
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
        observed = datetime.fromisoformat(self.inventory["quiesced_at"].replace("Z", "+00:00"))
        self.stopped_at = (observed - timedelta(seconds=1)).isoformat()
        self.build = {"sources": {"full": {"source": {"snapshot": {"head": "a" * 40, "clean": True}}}}}
        self.runtime = {"build": self.build, "verified_at": (observed - timedelta(seconds=2)).isoformat(),
                        "runtime": {"scope_id": "source"}, "processes": {"api": {"pid": 20}},
                        "physical_binding": {"scope_id": "source"}}
        self.runtime_path = self.write("runtime.json", self.runtime)
        self.quiescence_path = self.write("quiescence.json", {"observed_stopped_at": self.stopped_at})
        self.generation_path = self.write("source-generation.json", {"source_runtime": self.descriptor(self.runtime_path),
                                                                  "runtime_evidence": self.descriptor(self.quiescence_path)})
        self.export_path = self.write("source-export-result.json", {"status": "seed_source_export_published"})
        build_path = self.write("build.json", self.build)
        self.identity = {"result": self.descriptor(self.export_path), "export": {"sha256": "e" * 64},
                         "source_generation": self.descriptor(self.generation_path), "review_successor": {"same": "C52"}}
        self.exported = {
            "inventory": {key: value for key, value in self.inventory.items() if key != "id"},
            "request": {"source": {key: value for key, value in self.plan["source"].items() if key != "frontend_url"},
                        "tools": self.plan["tools"], "backend_build": self.descriptor(build_path)},
            "generation": {"source": self.build["sources"]["full"]["source"]["snapshot"],
                           **{key: self.runtime[key] for key in ("runtime", "processes", "physical_binding")}},
        }
        self.source = {"source_generation": self.descriptor(self.generation_path),
                       "request": copy.deepcopy(self.exported["request"]), "generation": copy.deepcopy(self.exported["generation"])}

    def descriptor(self, path):
        return {"path": str(path), **reference.file_digest(path)}

    def write(self, name, value):
        path = self.work / name
        reference.write_json(path, value, new=False)
        return path

    def verify(self):
        with patch.object(backup, "_source_export", return_value=self.identity), \
                patch.object(backup, "published_source", return_value=self.source), \
                patch.object(backup, "verify_source_export", return_value=self.exported):
            return backup.backup_source(self.backend, self.plan, self.inventory,
                                        self.generation_path, self.export_path)

    def test_binds_same_published_export_and_both_source_proofs(self):
        self.assertEqual(self.verify(), {"source_export": self.identity,
                                       "source_generation": self.descriptor(self.generation_path)})

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
        self.write("quiescence.json", {"observed_stopped_at": "2026-01-01T00:00:00Z"})
        with self.assertRaises(ValueError):
            self.verify()
        self.write("quiescence.json", {"observed_stopped_at": self.stopped_at})
        def mutate(*_args):
            self.write("source-export-result.json", {"changed": True})
            return self.exported
        with patch.object(backup, "_source_export", return_value=self.identity), \
                patch.object(backup, "published_source", return_value=self.source), \
                patch.object(backup, "verify_source_export", side_effect=mutate), self.assertRaises(ValueError):
            backup.backup_source(self.backend, self.plan, self.inventory, self.generation_path, self.export_path)

    def test_backup_requires_export_argument_before_any_resource_access(self):
        path = self.write("plan.json", self.plan)
        argv = ["restore_reference", "backup", "--plan", str(path), "--backend-dir", str(self.backend),
                "--inventory", "unused", "--source-generation", "unused", "--write"]
        with patch.object(sys, "argv", argv), patch.object(reference, "work_directory") as work, \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            reference.main()
        work.assert_not_called()

    def test_execution_consumes_original_export_inventory_without_editing_it(self):
        raw = copy.deepcopy(self.exported["inventory"])
        path = self.write("inventory.json", raw)
        original = path.read_bytes()
        args = SimpleNamespace(command="backup", copy_id=None, inventory=path,
                               source_generation=self.generation_path, source_export_result=self.export_path)
        with patch.object(reference, "source_snapshot", return_value={"clean": True, "head": raw["source_sha"]}), \
                patch.object(reference, "backup", return_value={"done": True}) as execute:
            self.assertEqual(reference.execute(args, self.plan, self.backend, Mock(), self.work), {"done": True})
        self.assertEqual(execute.call_args.args[3], self.inventory)
        self.assertEqual(path.read_bytes(), original)

    def test_hand_edited_inventory_is_rejected_before_any_backup_resource_call(self):
        path = self.write("inventory.json", self.inventory)
        args = SimpleNamespace(command="backup", copy_id=None, inventory=path,
                               source_generation=self.generation_path, source_export_result=self.export_path)
        with patch.object(reference, "source_snapshot") as source, patch.object(reference, "backup") as execute, \
                self.assertRaisesRegex(ValueError, "不能手工补写"):
            reference.execute(args, self.plan, self.backend, Mock(), self.work)
        source.assert_not_called()
        execute.assert_not_called()

    def test_different_generation_and_inventory_before_stop_fail_closed(self):
        self.source["source_generation"] = {"different": "generation"}
        with self.assertRaisesRegex(ValueError, "运行代次"):
            self.verify()
        self.source["source_generation"] = self.descriptor(self.generation_path)
        self.inventory["quiesced_at"] = self.runtime["verified_at"]
        self.exported["inventory"]["quiesced_at"] = self.runtime["verified_at"]
        with self.assertRaisesRegex(ValueError, "时间顺序"):
            self.verify()

    def test_unpublished_export_is_rejected_before_external_backup_or_output(self):
        tools = Mock()
        with patch.object(backup, "_source_export", side_effect=ValueError("未发布")), \
                self.assertRaisesRegex(ValueError, "未发布"):
            reference.backup(self.plan, tools, self.work, self.inventory, self.backend,
                             self.generation_path, self.export_path)
        self.assertEqual(tools.mock_calls, [])
        self.assertFalse((self.work / "backup").exists())

    def test_backup_publishes_manifest_binding_and_refuses_changed_export(self):
        tools = Mock()
        tools.dump.side_effect = lambda _db, _tables, path: path.write_text("fixture")
        inputs = self.verify()
        with patch.object(reference, "backup_source", return_value=inputs), \
                patch.object(reference, "require_stopped"), patch.object(reference, "verify_artifacts"):
            result = reference.backup(self.plan, tools, self.work, self.inventory, self.backend,
                                      self.generation_path, self.export_path)
        backup.validate_backup_result(result)
        self.assertEqual(result["source_export"], self.identity)
        self.assertEqual(result["manifest"], self.descriptor(self.work / "backup/manifest.json"))
        self.assertEqual(result["reference_plan"], self.plan)
        for change in (lambda value: value.update(extra=True), lambda value: value.pop("source_export"),
                       lambda value: value.update(format_version=True), lambda value: value.update(format_version=1)):
            invalid = copy.deepcopy(result)
            change(invalid)
            with self.assertRaises(ValueError):
                backup.validate_backup_result(invalid)

    def test_source_generation_drift_after_copy_cannot_publish_manifest(self):
        tools = Mock()
        tools.dump.side_effect = lambda _db, _tables, path: path.write_text("fixture")
        inputs = self.verify()
        changed = {**inputs, "source_generation": {"changed": True}}
        with patch.object(reference, "backup_source", side_effect=[inputs, changed]), \
                patch.object(reference, "require_stopped"), patch.object(reference, "verify_artifacts"), \
                self.assertRaisesRegex(ValueError, "来源绑定被替换"):
            reference.backup(self.plan, tools, self.work, self.inventory, self.backend,
                             self.generation_path, self.export_path)
        self.assertFalse((self.work / "backup/manifest.json").exists())
        self.assertTrue((self.work / "backup/databases").is_dir())


if __name__ == "__main__":
    unittest.main()
