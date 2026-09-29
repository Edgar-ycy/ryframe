"""由库存组装复制计划的纯回归；不连接数据库或对象存储。"""
import copy
from dataclasses import replace
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import test_devex_clone as fixtures
from devex_clone_factory_plan import declaration
from devex_clone_transfer import DatabaseObservation


class FactoryPlanTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.CloneTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.value = self.fixture.value
        self.exported = {key: copy.deepcopy(self.value[key]) for key in ("artifact_root", "source_snapshot", "databases", "objects")}
        self.exported["enabled_system_schedule_rows"] = []
        self.exported["worktree_fingerprint"] = "sha256:" + self.exported["source_snapshot"].pop("worktree_sha256")
        self.exported["source_snapshot"].update(patch_sha256="f" * 64, files=[])
        self.configs, self.images = {}, {}
        for side in ("source", "target"):
            config = self.value[side]
            self.configs[side] = {"scope_id": config["scope_id"], "s3": {"endpoint": config["object_endpoint"]},
                                  "databases": config["databases"]}
            self.images[side] = {db["key"]: DatabaseObservation(
                {"kind": "database", "scope_id": config["scope_id"], "server_uuid": db["server_uuid"], "database": db["database"]},
                db["schema_sha256"], {}, {}, (), tuple(db["ownership"])) for db in config["databases"]}
        self.exported["source"] = self.configs["source"]

    def build(self):
        return declaration(self.fixture.backend, self.exported, self.images["source"], self.configs["target"],
                           self.images["target"], self.value["evidence"], copy_id="clone-case", stage="source_to_seed")

    def test_constructs_same_complete_plan_without_changing_source_or_inventory(self):
        original = copy.deepcopy((self.exported, self.configs, self.images))
        self.assertEqual(self.build(), self.value)
        self.assertEqual(original, (self.exported, self.configs, self.images))

    def test_missing_target_image_cannot_be_replaced_by_declared_database(self):
        self.images["target"].pop("dedicated-a")
        with self.assertRaises(ValueError):
            self.build()

    def test_wrong_physical_inventory_is_rejected(self):
        observed = self.images["target"]["dedicated-a"]
        self.images["target"]["dedicated-a"] = replace(observed, resource={**observed.resource, "database": "unrelated"})
        with self.assertRaises(ValueError):
            self.build()

    def test_incompatible_schema_cannot_be_filled_from_source_declaration(self):
        observed = self.images["target"]["dedicated-a"]
        self.images["target"]["dedicated-a"] = replace(observed, schema_sha256="d" * 64)
        with self.assertRaises(ValueError):
            self.build()

    def test_target_owner_cannot_be_copied_from_source(self):
        observed = self.images["target"]["dedicated-a"]
        self.images["target"]["dedicated-a"] = replace(observed, ownership=self.images["source"]["dedicated-a"].ownership)
        with self.assertRaises(ValueError):
            self.build()

    def test_changed_payload_is_checked_before_a_plan_is_returned(self):
        (self.fixture.root / "file.bin").write_bytes(b"other")
        with self.assertRaises(ValueError):
            self.build()


if __name__ == "__main__":
    unittest.main()
