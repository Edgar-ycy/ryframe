"""正式备份的共享导出绑定；所有来源与外部命令均使用离线替身。"""

import copy
from datetime import datetime, timedelta
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_reference as reference
import restore_reference_backup as backup
from restore_reference_plan import _native as reference_native
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

    def native_source(self, operation, arguments):
        if operation == "comparison-source-export":
            return self.identity
        return reference_native(operation, arguments)

    def verify(self):
        with patch.object(backup, "_native", side_effect=self.native_source), \
                patch.object(backup, "published_source", return_value=self.source), \
                patch.object(backup, "verify_source_export", return_value=self.exported):
            return backup.backup_source(self.backend, self.plan, self.inventory,
                                        self.generation_path, self.export_path)


    def test_rejects_stale_quiescence_and_mutation_during_verification(self):
        self.write("quiescence.json", {"observed_stopped_at": "2026-01-01T00:00:00Z"})
        with self.assertRaises(ValueError):
            self.verify()
        self.write("quiescence.json", {"observed_stopped_at": self.stopped_at})
        def mutate(*_args):
            self.write("source-export-result.json", {"changed": True})
            return self.exported
        with patch.object(backup, "_native", side_effect=self.native_source), \
                patch.object(backup, "published_source", return_value=self.source), \
                patch.object(backup, "verify_source_export", side_effect=mutate), self.assertRaises(ValueError):
            backup.backup_source(self.backend, self.plan, self.inventory, self.generation_path, self.export_path)


    def test_different_generation_and_inventory_before_stop_fail_closed(self):
        self.source["source_generation"] = {"different": "generation"}
        with self.assertRaisesRegex(ValueError, "运行代次"):
            self.verify()
        self.source["source_generation"] = self.descriptor(self.generation_path)
        self.inventory["quiesced_at"] = self.runtime["verified_at"]
        self.exported["inventory"]["quiesced_at"] = self.runtime["verified_at"]
        with self.assertRaisesRegex(ValueError, "时间顺序"):
            self.verify()

if __name__ == "__main__":
    unittest.main()
