"""真实完整像阶段调用离线协议替身；保留四库、五桶、owner 与 Redis 的证据。"""
import copy
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch

import devex_clone_seed_generation_images as images
import test_devex_clone_inventory as inventory_fixtures
from devex_clone_source_fixture import SourceFixture
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import BUCKETS, plan_hash


class GenerationImageTests(unittest.TestCase):
    def setUp(self):
        self.f = inventory_fixtures.InventoryTests()
        self.f.setUp()
        self.addCleanup(self.f.doCleanups)
        self.backend, self.output = self.f.backend, self.f.local / "i"
        self.tools = {name: {"path": str(self.f.external), "sha256": self.f.tools.plan["tools"]["mysql"]["sha256"]}
                      for name in ("mysql", "aws")}
        runtime = Path(self.f.selected["runtime_dir"])
        runtime.mkdir()
        write_json(runtime / "runtime.json", {"backend_root": str(self.backend)})
        review = self.file("review.json", {"scopes": {"seed": {"scope_id": self.f.scope}}})
        source_registration = self.file("c52.json", {"c52": "immutable"})
        self.source = {"request": {"source": self.f.selected, "tools": self.tools, "max_object_bytes": 1024},
                       "seed_target": {"review": review}, "directory": self.f.local,
                       "review_successor": {"source_result": source_registration}}
        build = self.file("source-build.json", {"sources": {"full": {"source": self.f.maintenance["source"]}}})
        self.request = {"backend_build": build, "maintenance_build": binding(self.f.maintenance_file)}
        self.resources = Mock()
        self.resources.storage_identity.return_value = {"same": "owned-storage"}
        self.resources.redis_state.return_value = {"same": "owned-namespace"}
        self.count = 0

    def file(self, name, value):
        path = self.f.local / name
        write_json(path, value)
        return binding(path)

    def run_external(self, command, **kwargs):
        if "s3api" in command:
            self.f.calls.append(command)
            return SourceFixture.aws(self.f, command)
        if "backup-inventory" not in command:
            return self.f.run_external(command, **kwargs)
        self.f.calls.append(command)
        self.assertIn("--observed-at", command)
        self.assertNotIn("--quiesced-at", command)
        value = {"scope_id": self.f.scope, "source_sha": "a" * 40,
                 "observed_at": command[command.index("--observed-at") + 1], "captured_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                 "control_schema_fingerprint": "c" * 16, "tenant_schema_fingerprint": "d" * 64, "databases": [], "objects": []}
        for db in self.f.selected["databases"]:
            raw = self.f.raw(db["key"])["target"]
            known = set(self.f.models[1]) | (set(self.f.models[2]) - {"ryframe_resource_ownership", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}
                                               if db["kind"] == "combined" else set())
            value["databases"].append({**raw["database"], "tables": [row for row in raw["database"]["tables"] + raw["preserved_tables"] if row["table"] in known]})
        for bucket in sorted(BUCKETS):
            entries = [{"key": self.f.scope + "/post/file.txt", "bytes": 4, "sha256": hashlib.sha256(b"data").hexdigest()}] if bucket == "uploads" else []
            value["objects"].append({"bucket": bucket, "prefix": self.f.scope + "/", "entries": entries})
        self.count += 1
        if self.count == 2 and getattr(self, "inventory_drift", False):
            value["databases"][0]["tables"][0]["sha256"] = "f" * 64
        write_json(Path(command[command.index("--output") + 1]), value)
        return subprocess.CompletedProcess(command, 0, b"observed", b"")

    def capture(self):
        with patch.dict("os.environ", self.f.environment, clear=True), patch.object(images, "Resources", return_value=self.resources), \
                patch.object(images, "redis_configuration"), patch.object(images, "verify_tools", return_value=self.f.maintenance), \
                patch.object(images, "verify_migrations"), patch.object(images, "schema_snapshot", side_effect=self.schema):
            return images.capture_image(self.backend, self.backend, self.f.selected, self.request, self.source,
                                        self.f.environment, self.output, self.run_external)

    def schema(self, tools, models, phase):
        result = []
        for db in self.f.selected["databases"]:
            columns = {**(models[2] if db["kind"] == "combined" else {}), **models[3],
                       "seaql_tenant_data_migrations": {"version": "VARCHAR"}}
            if db["kind"] == "combined":
                columns["seaql_migrations"] = {"version": "VARCHAR"}
            value = {"key": db["key"], "columns": columns}
            write_json(tools.work / f"schema-{phase}-{db['key']}.json", value)
            result.append({**value, "sha256": plan_hash(columns)})
        return result

    def verify(self, descriptor):
        return images.verify_image(self.backend, descriptor, self.f.selected, self.source["request"],
                                   source_registration=self.source["review_successor"]["source_result"])

    def test_capture_and_readonly_verify_retain_all_raw_databases_five_owners_and_observed_inventory(self):
        descriptor = self.capture()
        before = {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        with patch("subprocess.run", side_effect=AssertionError("保存像复核不得执行命令")):
            value = self.verify(descriptor)
        self.assertEqual(len(value["raw_inventories"]), 8)
        self.assertEqual(len(value["owner_evidence"]), 10)
        self.assertEqual(len(value["schema_evidence"]), 8)
        self.assertEqual(set(value["image"]["databases"]), set(images.KEYS))
        self.assertEqual(set(value["image"]["objects"]), set(BUCKETS))
        self.assertEqual(value["scale"]["objects"], 1)
        self.assertEqual(value["scale"]["object_bytes"], 4)
        self.assertEqual(len(value["object_evidence"]), 1)
        self.assertEqual(value["source_registration"], self.source["review_successor"]["source_result"])
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()})
        self.assertEqual(self.count, 2)
        self.assertFalse(any("put-object" in command or "backup-register" in command for command in self.f.calls))

    def test_readonly_verify_rejects_raw_table_drift_and_different_c52(self):
        descriptor = self.capture()
        with self.assertRaises(ValueError):
            images.verify_image(self.backend, descriptor, self.f.selected, self.source["request"], source_registration={"different": True})
        raw = self.output / "databases/after-target-shared.json"
        value = read_json(raw)
        value["target"]["preserved_tables"][0]["sha256"] = "e" * 64
        raw.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(ValueError):
            self.verify(descriptor)

    def test_redis_or_inventory_drift_prevents_successful_image(self):
        self.resources.redis_state.side_effect = [{"same": "owner"}, {"different": "external-key"}]
        with self.assertRaisesRegex(ValueError, "采集期间"):
            self.capture()
        self.assertFalse((self.output / "image.json").exists())
        self.output = self.f.local / "j"
        self.resources.redis_state.side_effect = None
        self.inventory_drift, self.count = True, 0
        with self.assertRaisesRegex(ValueError, "采集期间"):
            self.capture()
        self.assertFalse((self.output / "image.json").exists())

    def test_schema_binding_and_extra_image_fields_fail_closed_after_receipt_rebinding(self):
        descriptor = self.capture()
        path = Path(descriptor["path"])
        value = read_json(path)
        value["image"]["extra"] = "unknown"
        value["image_sha256"] = plan_hash(value["image"])
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(ValueError):
            self.verify(binding(path))
        del value["image"]["extra"]
        value["image_sha256"] = plan_hash(value["image"])
        path.write_text(json.dumps(value), encoding="utf-8")
        schema_path = self.output / "schema-after-shared-control.json"
        schema_path.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "schema"):
            self.verify(binding(path))


if __name__ == "__main__":
    unittest.main()
