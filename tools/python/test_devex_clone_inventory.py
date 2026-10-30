"""库存原生端口的输入隔离、完整类型还原和失败不重放；采集行为由Rust数据库契约覆盖。"""
from dataclasses import asdict
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from mysql_verification_fixture import mysql_input, mysql_output
from devex_clone_source_fixture import SourceFixture
from devex_clone_export import schema_models
from restore_reference_io import ExternalTools
import test_devex_clone as clone_fixture
import devex_clone_inventory as inventory
from restore_build import file_digest
from restore_reference_plan import plan_hash
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


UUID = clone_fixture.UUID


class InventoryFixture(unittest.TestCase):
    def setUp(self):
        fixture = clone_fixture.CloneTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.backend, self.local = fixture.backend, fixture.backend / ".local-tests"
        self.scope = "inventory-test-source"
        self.client = self.local / "inventory-client.cnf"
        self.client.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n", encoding="utf-8")
        self.selected = {"scope_id": self.scope, "runtime_dir": str(self.local / "runtime"), "api_url": "http://127.0.0.1:18210",
                         "s3": {}, "databases": []}
        for key in inventory.KEYS:
            self.selected["databases"].append({"key": key, "kind": "combined" if key == "shared-control" else "tenant",
                "mode": "dedicated" if key.startswith("dedicated-") else "shared", "server_uuid": UUID,
                "database": "inventory_" + key.replace("-", "_"), "defaults_file": str(self.client),
                "defaults_sha256": file_digest(self.client)["sha256"]})
        SourceFixture.write_config(self, self.selected)
        self.environment = {"APP_ENV": "test", "APP_SCOPE_ID": self.scope, "APP_SOURCE_PASSWORD": "fixture-secret",
                            "APP_CONFIG_DIR": str(self.backend / "config"), "APP_OPTIONAL_EMPTY": "",
                            "AUTH_ACCESS": "access-fixture", "AUTH_PRIVATE": "secret-fixture"}
        self.external = self.local / "mysql-fixture.exe"
        self.external.write_bytes(b"fixed-mysql-fixture")
        self.maintenance = {"source": {"snapshot": {"head": "a" * 40, "clean": False}, "worktree_fingerprint": "sha256:" + "b" * 64}, "artifacts": {}}
        for role in ("reset", "migrate", "tenant-data"):
            path = self.local / ("inventory-" + role + ".exe")
            path.write_bytes(role.encode())
            self.maintenance["artifacts"][role] = {"executable": str(path), **file_digest(path)}
        self.maintenance_file = self.local / "inventory-build.json"
        self.maintenance_file.write_text(json.dumps(self.maintenance), encoding="utf-8")
        self.tools = ExternalTools({"source": self.selected, "tools": {"mysql": {"path": str(self.external),
                                  "sha256": file_digest(self.external)["sha256"]}}}, self.local, self.run_external)
        self.calls = []
        self.models = schema_models(self.backend)
        self._table_digests = {}

    def raw(self, key):
        declared = next(item for item in self.selected["databases"] if item["key"] == key)
        combined = key == "shared-control"
        control, tenant, full_control, _ = self.models
        business = set(tenant) | (set(control) if combined else set())
        preserved = {"ryframe_resource_ownership", "seaql_tenant_data_migrations",
                     "biz_data_scope_owner", "biz_data_scope_dept", "biz_data_scope_projection"}
        if combined:
            preserved |= {"seaql_migrations", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}
        all_tables = business | preserved | (set(full_control) if combined else set())
        def table(name):
            rows = 2 if name == "ryframe_resource_ownership" and combined else 1 if name in preserved or name == "biz_tenant_target_slot" else 0
            key = (name, rows)
            if key not in self._table_digests:
                self._table_digests[key] = plan_hash({"table": name, "rows": rows})
            return {"table": name, "rows": rows, "sha256": self._table_digests[key]}
        return {"scope_id": self.scope, "control_schema_fingerprint": "c" * 16, "tenant_schema_fingerprint": "d" * 64,
                "target": {"database": {**{name: declared[name] for name in ("key", "kind", "server_uuid", "database")},
                    "shared": declared["mode"] == "shared", "placements": [], "tables": [table(name) for name in sorted(all_tables - preserved)]},
                    "preserved_tables": [table(name) for name in sorted(preserved)]}}

    def run_external(self, command, **kwargs):
        self.calls.append(command)
        self.assertEqual(kwargs["env"]["APP_SCOPE_ID"], self.scope)
        self.assertEqual(kwargs["env"]["APP_OPTIONAL_EMPTY"], "")
        self.assertEqual(kwargs["env"]["AUTH_PRIVATE"], "secret-fixture")
        self.assertNotIn("MYSQL_PWD", kwargs["env"])
        if command[0] == str(self.external):
            db_name = next(value.removeprefix("--database=") for value in command if value.startswith("--database="))
            declared = next(item for item in self.selected["databases"] if item["database"] == db_name)
            sql = mysql_input(command, kwargs)
            if sql == "SELECT @@server_uuid, DATABASE();":
                result = f"{UUID}\t{db_name}"
            else:
                self.assertEqual(sql, "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;")
                owners = inventory.expected_owners(self.scope, declared["kind"] == "combined")
                result = "\n".join("\t".join(row[name] for name in ("resource_kind", "scope_id", "marker")) for row in owners)
            return mysql_output(command, kwargs, result.encode())
        # capture_inventory仍由完整image编排实际调用；这不是已删除_Capture的执行替身。
        self.assertEqual(command[0], self.maintenance["artifacts"]["tenant-data"]["executable"])
        self.assertEqual(command[1:4], ["--config-dir", str(self.backend / "config"), "target-inventory"])
        self.assertEqual(kwargs["cwd"], self.backend)
        value = self.raw(command[command.index("--target") + 1])
        Path(command[command.index("--output") + 1]).write_text(json.dumps(value), encoding="utf-8")
        return subprocess.CompletedProcess(command, 0, b"observed", b"")


class InventoryPortTests(unittest.TestCase):
    def setUp(self):
        temporary = WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / "inventory"
        self.maintenance = self.root / "build.json"
        self.maintenance.write_text("frozen-maintenance-input", encoding="utf-8")
        self.private = {"APP_ENV": "test", "APP_UNUSED_PASSWORD": "private-not-protocol"}
        configuration = {"directory": str(self.root / "non-default-E"), "app_env": "test",
                         "files": {"app.test.toml": {"bytes": 1, "sha256": "a" * 64}}, "sha256": "b" * 64}
        self.tools = SimpleNamespace(plan={"source": {"s3": {"configuration": configuration}}, "tools": {}})
        self.effect = None
        self.calls = []

    def emit(self, command, request, runner, *, may_write):
        self.calls.append(copy.deepcopy(request))
        self.assertEqual(command, "__source-evidence")
        self.assertFalse(may_write)
        self.assertTrue(request["write"])
        self.output.mkdir()
        before = {"side": "source", "tools_plan_sha256": digest(self.tools.plan)}
        observations = {key: {"resource": {"kind": "database", "database": key}, "schema_sha256": "c" * 64,
            "tables": {"biz_fixture": {"rows": 100000, "sha256": "d" * 64}}, "preserved": {},
            "all_tables": ["biz_fixture"], "ownership": []} for key in inventory.KEYS}
        receipt = {"format_version": 1, "status": "side_inventory_captured", "side": "source", "scope_id": "port-fixture",
            "binding_sha256": digest(before), "keys": list(inventory.KEYS), "inventories": {}, "observations": observations,
            "remote_writes": 0, "producer_stopped_proven": False, "fresh_target_proven": False,
            "target_ready": False, "clone_verified": False, "restore_qualified": False}
        for name, value in (("binding-before.json", before), ("binding-after.json", before), ("inventory.json", receipt)):
            (self.output / name).write_text(json.dumps(value), encoding="utf-8")
        published = {"path": str(self.output / "inventory.json"), **file_digest(self.output / "inventory.json")}
        files = [{"path": path.name, "type": "file", **file_digest(path)} for path in sorted(self.output.iterdir())]
        if self.effect:
            self.effect()
        value = {"side": "source", "binding": before, "receipt": receipt, "receipt_file": published, "evidence_files": files}
        return subprocess.CompletedProcess([], 0, b"", b""), {"ok": True, "result": {"value": value}}

    def capture(self, **kwargs):
        with patch.object(inventory, "native_invoke", side_effect=self.emit), patch.object(inventory, "plan_hash", side_effect=digest):
            return inventory.capture_side_inventory(ROOT, "source", self.tools, self.maintenance, self.output,
                environment=self.private, **kwargs)

    def test_nondefault_configuration_and_full_observations_survive_the_port(self):
        result = self.capture()
        arguments = self.calls[0]["arguments"]
        self.assertEqual(arguments["configuration"], self.tools.plan["source"]["s3"]["configuration"])
        self.assertNotIn("private-not-protocol", json.dumps(self.calls))
        self.assertNotIn("environment", arguments)
        self.assertEqual([asdict(item)["tables"]["biz_fixture"]["rows"] for item in result.observations], [100000] * 4)
        self.assertEqual(result.observations[0].all_tables, ("biz_fixture",))
        self.assertEqual(result.observations[0].tables["biz_fixture"]["sha256"], "d" * 64)
        self.assertEqual(self.maintenance.read_text(encoding="utf-8"), "frozen-maintenance-input")

    def test_input_drift_or_unknown_evidence_revokes_only_this_publication_without_retry(self):
        for kind in ("plan", "environment", "maintenance", "unknown"):
            with self.subTest(kind=kind):
                self.output = self.root / kind
                original = copy.deepcopy(self.tools.plan)
                def change():
                    if kind == "plan": self.tools.plan["changed"] = True
                    elif kind == "environment": self.private["APP_ENV"] = "prod"
                    elif kind == "maintenance": self.maintenance.write_text("changed", encoding="utf-8")
                    else: (self.output / "unknown.json").write_text("unknown", encoding="utf-8")
                self.effect = change
                prior = len(self.calls)
                with self.assertRaises(inventory.InventoryCaptureError):
                    self.capture()
                self.assertEqual(len(self.calls), prior + 1)
                self.assertFalse((self.output / "inventory.json").exists())
                self.assertTrue((self.output / "failure.json").is_file())
                self.tools.plan, self.private["APP_ENV"] = original, "test"
        self.assertEqual((self.output / "unknown.json").read_text(encoding="utf-8"), "unknown")

    def test_native_transport_uses_safe_os_context_without_new_overall_deadline(self):
        with patch.dict(os.environ, {"CARGO_HOME": "D:/fixture-cargo", "APP_PRIVATE": "never-forward"}), \
                patch.object(subprocess, "run") as run:
            inventory._inventory_transport(["fixed-tool", "__source-evidence"], timeout=1810,
                env={"CARGO_HOME": "D:/fixture-cargo", "APP_PRIVATE": "never-forward"})
        options = run.call_args.kwargs
        self.assertNotIn("timeout", options)
        self.assertNotIn("APP_PRIVATE", options["env"])
        self.assertEqual(options["env"]["CARGO_HOME"], "D:/fixture-cargo")


if __name__ == "__main__":
    unittest.main()
