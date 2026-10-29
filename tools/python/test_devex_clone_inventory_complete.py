"""完整库存只接受实际逐目标结果，遗漏的非空目标不能补作空目标。"""
import copy
import unittest

from devex_clone_inventory_complete import complete_inventory


class CompleteInventoryTests(unittest.TestCase):
    def setUp(self):
        self.database = {"key": "empty", "placements": [], "tables": [{"table": "biz_order", "rows": 0, "sha256": "a" * 64}]}
        self.runtime = {"scope_id": "scope", "control_schema_fingerprint": "c", "tenant_schema_fingerprint": "t", "databases": []}
        self.target = {**{key: self.runtime[key] for key in self.runtime if key != "databases"},
                       "target": {"database": self.database, "preserved_tables": []}}
        self.request = {"source": {"databases": [{"key": "empty"}]}}

    def test_empty_actual_target_is_retained_without_mutating_inputs(self):
        result = complete_inventory(self.runtime, [self.target], self.request)
        self.assertEqual(result["databases"], [self.database])
        result["databases"][0]["tables"].clear()
        self.assertTrue(self.database["tables"])
        self.assertEqual(self.runtime["databases"], [])

    def test_omitted_nonempty_target_and_placement_are_rejected(self):
        for field, value in (("tables", [{"table": "biz_order", "rows": 1}]), ("placements", [{"tenant_id": "tenant"}])):
            target = copy.deepcopy(self.target)
            target["target"]["database"][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "非空"):
                complete_inventory(self.runtime, [target], self.request)

    def test_projection_is_copied_after_raw_comparison_and_nonempty_omission_is_rejected(self):
        target = copy.deepcopy(self.target)
        projection = {"table": "biz_data_scope_owner", "rows": 2, "sha256": "b" * 64}
        target["target"]["preserved_tables"] = [projection]
        runtime = {**self.runtime, "databases": [copy.deepcopy(self.database)]}
        result = complete_inventory(runtime, [target], self.request)
        self.assertIn(projection, result["databases"][0]["tables"])
        self.assertNotIn(projection, runtime["databases"][0]["tables"])
        with self.assertRaisesRegex(ValueError, "非空"):
            complete_inventory(self.runtime, [target], self.request)

    def test_scope_schema_duplicate_missing_and_unknown_target_are_rejected(self):
        for field in ("scope_id", "control_schema_fingerprint", "tenant_schema_fingerprint"):
            target = copy.deepcopy(self.target)
            target[field] = "wrong"
            with self.subTest(field=field), self.assertRaises(ValueError):
                complete_inventory(self.runtime, [target], self.request)
        for targets in ([], [self.target, self.target], [{**self.target, "target": {
                "database": {**self.database, "key": "unknown"}, "preserved_tables": []}}]):
            with self.subTest(targets=targets), self.assertRaises(ValueError):
                complete_inventory(self.runtime, targets, self.request)

    def test_runtime_mismatch_duplicate_and_unknown_target_are_rejected(self):
        for databases in ([{**self.database, "database": "wrong"}], [self.database, self.database], [{**self.database, "key": "unknown"}]):
            with self.subTest(databases=databases), self.assertRaises(ValueError):
                complete_inventory({**self.runtime, "databases": databases}, [self.target], self.request)
