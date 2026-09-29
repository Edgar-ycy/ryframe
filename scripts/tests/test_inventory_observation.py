"""运行中观察库存与正式停机库存严格分型；采集核心仍覆盖完整数据库和五桶。"""
import copy
import unittest
from unittest.mock import patch

from devex_clone_export import capture_inventory, logical_inventory, validate_inventory
from restore_reference_plan import BUCKETS


class InventoryObservationTests(unittest.TestCase):
    def setUp(self):
        self.instant = "2000-01-01T00:00:00+00:00"
        self.sha = "a" * 40
        self.models = ({}, {"sys_post": []}, {}, {})
        self.database = {"key": "shared", "kind": "tenant", "database": "seed", "server_uuid": "uuid", "mode": "shared"}
        self.request = {"source": {"scope_id": "seed-scope", "databases": [self.database]}}
        self.value = {"scope_id": "seed-scope", "source_sha": self.sha, "observed_at": self.instant,
                      "captured_at": "2000-01-01T00:00:01+00:00", "control_schema_fingerprint": "a" * 16,
                      "tenant_schema_fingerprint": "b" * 64,
                      "databases": [{**{key: value for key, value in self.database.items() if key != "mode"},
                        "shared": True, "placements": [], "tables": [{"table": "sys_post", "rows": 0, "sha256": "c" * 64}]}],
                      "objects": [{"bucket": bucket, "prefix": "seed-scope/", "entries": []} for bucket in sorted(BUCKETS)]}

    def test_observed_and_quiesced_cannot_be_interchanged_or_combined(self):
        validate_inventory(self.value, self.request, self.models, self.sha, self.instant, observed=True)
        stopped = copy.deepcopy(self.value)
        stopped["quiesced_at"] = stopped.pop("observed_at")
        validate_inventory(stopped, self.request, self.models, self.sha, self.instant)
        for value, observed in ((self.value, False), (stopped, True), ({**self.value, "quiesced_at": self.instant}, True),
                                ({**stopped, "observed_at": self.instant}, False)):
            with self.subTest(observed=observed, fields=set(value)), self.assertRaises(ValueError):
                validate_inventory(value, self.request, self.models, self.sha, self.instant, observed=observed)

    def test_observed_inventory_retains_complete_scope_and_stable_comparison(self):
        for mutation in (lambda value: value["objects"].pop(), lambda value: value["databases"][0]["tables"].clear(),
                         lambda value: value.update(scope_id="other"), lambda value: value.update(source_sha="b" * 40)):
            value = copy.deepcopy(self.value)
            mutation(value)
            with self.assertRaises(ValueError):
                validate_inventory(value, self.request, self.models, self.sha, self.instant, observed=True)
        later = copy.deepcopy(self.value)
        later["captured_at"] = "2000-01-01T00:00:02+00:00"
        self.assertEqual(logical_inventory(self.value), logical_inventory(later))
        later["databases"][0]["tables"][0]["sha256"] = "d" * 64
        self.assertNotEqual(logical_inventory(self.value), logical_inventory(later))

    def test_capture_routes_only_selected_flag_without_register_or_write_operation(self):
        from pathlib import Path
        from types import SimpleNamespace

        tools = SimpleNamespace(work=Path("inventory"))
        generation = {"maintenance": {}, "source": {"head": self.sha}}
        with patch("devex_clone_export.cli_executable", return_value="tenant-data"), \
                patch("devex_clone_export.run_cli") as execute, patch("devex_clone_export.read_json", return_value=self.value):
            capture_inventory(tools, Path("backend"), self.request, generation, self.models, "before", self.instant, observed=True)
        command = execute.call_args.args[2]
        self.assertEqual(command[1], "backup-inventory")
        self.assertIn("--observed-at", command)
        self.assertNotIn("--quiesced-at", command)

    def test_formal_backup_rejects_observed_before_inspecting_any_resources(self):
        from restore_reference_plan import validate_inventory as formal_inventory

        for value in (self.value, {**self.value, "quiesced_at": self.instant}):
            with self.assertRaisesRegex(ValueError, "停机库存"):
                formal_inventory({}, value)


if __name__ == "__main__":
    unittest.main()
