"""来源业务验收的会话副作用与精确缓存键模型。"""

import copy
import hashlib
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from restore_reference_plan import plan_hash
from restore_source_runtime_model import (
    authorization_cache_plan,
    image_write_effects,
    validate_login_audit,
    validate_node_result,
)


def digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


class SourceRuntimeModelTests(unittest.TestCase):
    def setUp(self):
        self.lineage = {
            "verification": {"scope_id": "perf-seed-unit"},
            "scopes": {"origin_tenant_scope_id": "restore-source-unit"},
            "scale": {"tenants": 11, "post_samples": 33, "business_objects": 256},
            "tenants": [
                {"tenant_id": "system" if index == 0 else f"restore-source-unit-{index:02d}"}
                for index in range(11)
            ],
        }
        self.start = {"path": "start", "bytes": 1, "sha256": "a" * 64}
        self.lineage_descriptor = {"path": "lineage", "bytes": 1, "sha256": "b" * 64}
        self.subjects = [
            {
                "tenant_id": tenant["tenant_id"],
                "user_id": str(9_007_199_254_740_993 + index),
                "user_authorization_version": index,
            }
            for index, tenant in enumerate(self.lineage["tenants"])
        ]

    def node_result(self):
        return {
            "format_version": 1,
            "kind": "restore-source-existing-verification",
            "status": "source_existing_data_verified",
            "scope_id": "perf-seed-unit",
            "origin_tenant_scope_id": "restore-source-unit",
            "lineage_sha256": plan_hash(self.lineage),
            "actions": {
                "business": "read_only",
                "objects": "read_only",
                "session": "login_logout",
            },
            "restore_success": False,
            "subjects": self.subjects,
            "tenants": 11,
            "posts": 33,
            "files": 256,
            "source_generation_sha256": "a" * 64,
            "lineage_file_sha256": "b" * 64,
        }

    @staticmethod
    def database(key: str, *, login_rows: int = 5, suffix: str = "before") -> dict:
        tables = [
            {"table": "sys_login_info", "rows": login_rows, "sha256": digest(f"login:{suffix}")},
            {"table": "sys_oper_log", "rows": 7, "sha256": digest("oper")},
            {"table": "sys_outbox_event", "rows": 0, "sha256": digest("outbox")},
            {"table": "sys_post", "rows": 100_000, "sha256": digest("post")},
        ]
        return {
            "binding": {"key": key},
            "target": {
                "database": {
                    "key": key,
                    "placements": [{"tenant_id": "system"}],
                    "tables": tables,
                },
                "preserved_tables": [],
            },
        }

    def images(self):
        before = {
            "databases": {
                "shared-control": self.database("shared-control"),
                "shared": self.database("shared"),
                "dedicated-a": self.database("dedicated-a"),
                "dedicated-b": self.database("dedicated-b"),
            },
            "schema": [{"key": "shared-control", "columns": []}],
            "objects": {"uploads": {}},
            "owners": [{"bucket": "uploads", "key": "owner"}],
            "redis": {"keys": ["owner"]},
            "storage": {"endpoint": "http://127.0.0.1:9000"},
        }
        after = copy.deepcopy(before)
        after["databases"]["shared-control"] = self.database(
            "shared-control", login_rows=16, suffix="after"
        )
        return before, after

    def test_accepts_exact_node_result_and_builds_only_registered_keys(self):
        result = validate_node_result(
            self.node_result(), self.lineage, self.start, self.lineage_descriptor
        )
        self.assertEqual(result["subjects"], self.subjects)
        plan = authorization_cache_plan("ryframe:{perf-seed-unit}:", result["subjects"])
        keys = [key for row in plan for key in row["keys"].values()]
        self.assertEqual(len(plan), 11)
        self.assertEqual(len(keys), len(set(keys)), 33)
        self.assertEqual(
            plan[1]["keys"]["snapshot"],
            "ryframe:{perf-seed-unit}:ryframe:authorization:{restore-source-unit-01}:user:9007199254740994:snapshots",
        )

    def test_rejects_wrong_subject_or_unregistered_namespace(self):
        result = self.node_result()
        result["subjects"][1]["tenant_id"] = "perf-seed-unit-01"
        with self.assertRaisesRegex(ValueError, "登录主体"):
            validate_node_result(result, self.lineage, self.start, self.lineage_descriptor)
        for namespace in ("", "ryframe:perf-seed-unit:", "ryframe:{bad scope}:"):
            with self.assertRaisesRegex(ValueError, "namespace"):
                authorization_cache_plan(namespace, self.subjects)

    def test_accepts_only_eleven_login_rows_and_zero_other_changes(self):
        before, after = self.images()
        self.assertEqual(
            image_write_effects(before, after),
            {
                "api_requests": {"login": 11, "logout": 11},
                "database_rows": {
                    "sys_login_info": 11,
                    "sys_oper_log": 0,
                    "sys_outbox_event": 0,
                },
                "authorization_cache": {"created_and_removed_keys": 33},
                "business_mutations": 0,
                "object_mutations": 0,
            },
        )

    def test_login_audit_binds_unchanged_old_rows_and_each_new_subject(self):
        rows = [
            {
                "id": str(101 + index),
                "tenant_id": tenant["tenant_id"],
                "user_name": f"user-{index}",
                "ipaddr": f"198.18.20.{index + 1}",
                "login_location": None,
                "browser": "Node",
                "os": None,
                "status": "1",
                "msg": None,
                "login_time": "2026-09-12T01:00:01.000000",
            }
            for index, tenant in enumerate(self.lineage["tenants"])
        ]
        for index, tenant in enumerate(self.lineage["tenants"]):
            tenant["username"] = f"user-{index}"
        result = validate_login_audit(
            b"old rows\n",
            b"old rows\n",
            100,
            rows,
            self.lineage,
            self.subjects,
            "2026-09-12T01:00:00+00:00",
            "2026-09-12T01:00:02+00:00",
        )
        self.assertTrue(result["old_rows_unchanged"])
        self.assertEqual(result["upper_id"], "100")
        self.assertEqual(result["new_rows"], rows)

        for change in (
            lambda value: value[0].update(tenant_id="other"),
            lambda value: value[0].update(status="0"),
            lambda value: value[0].update(login_time="2026-09-12T01:00:03.000000"),
            lambda value: value[0].update(id="102"),
        ):
            changed = copy.deepcopy(rows)
            change(changed)
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_login_audit(
                    b"old rows\n",
                    b"old rows\n",
                    100,
                    changed,
                    self.lineage,
                    self.subjects,
                    "2026-09-12T01:00:00+00:00",
                    "2026-09-12T01:00:02+00:00",
                )

    def test_rejects_unknown_database_object_redis_or_audit_drift(self):
        before, after = self.images()
        changes = [
            lambda value: value["objects"]["uploads"].update({"unknown": {}}),
            lambda value: value["redis"].update({"keys": ["owner", "unknown"]}),
            lambda value: value["databases"]["shared"]["target"]["database"]["tables"][3].update(rows=1),
            lambda value: value["databases"]["shared-control"]["target"]["database"]["tables"][1].update(
                sha256=digest("changed")
            ),
            lambda value: value["databases"]["shared-control"]["target"]["database"]["tables"][0].update(
                rows=15
            ),
        ]
        for change in changes:
            candidate = copy.deepcopy(after)
            change(candidate)
            with self.subTest(change=change):
                with self.assertRaises(ValueError):
                    image_write_effects(before, candidate)


if __name__ == "__main__":
    unittest.main()
