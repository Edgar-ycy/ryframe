"""四目标库存的离线协议回归；运行真实采集入口，但所有外部进程均由明确替身响应。"""
import copy
from mysql_verification_fixture import mysql_input, mysql_output
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_inventory as inventory
from devex_clone_transfer import DatabaseObservation
from devex_clone_export import schema_models
from devex_clone_model import plan_hash
from devex_clone_rows import EXCLUDED
from restore_build import file_digest
from restore_reference_io import ExternalTools
from devex_clone_source_fixture import SourceFixture
import test_devex_clone as clone_fixture

UUID = clone_fixture.UUID


class InventoryTests(unittest.TestCase):
    def setUp(self):
        fixture = clone_fixture.CloneTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.backend, self.local = fixture.backend, fixture.backend / ".local-tests"
        self.scope = "inventory-test-source"
        self.output = self.local / "inventory-output"
        self.client = self.local / "inventory-client.cnf"
        self.client.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n", encoding="utf-8")
        self.selected = {"scope_id": self.scope, "runtime_dir": str(self.local / "runtime"), "api_url": "http://127.0.0.1:18210",
                         "s3": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1",
                                "access_key_env": "AUTH_ACCESS", "secret_key_env": "AUTH_PRIVATE"}, "databases": []}
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
        self.calls, self.cli_count, self.effect = [], 0, None
        self.raw_override = None
        self.uuid_failure = self.owner_failure = False
        self.models = schema_models(self.backend)
        context = patch.object(inventory, "verify_tools", side_effect=lambda *_: copy.deepcopy(self.maintenance))
        self.verify_mock = context.start()
        self.addCleanup(context.stop)

    def raw(self, key):
        declared = next(item for item in self.selected["databases"] if item["key"] == key)
        combined = key == "shared-control"
        control, tenant, full_control, _ = self.models
        business = set(tenant) | (set(control) if combined else set())
        preserved = {"ryframe_resource_ownership", "seaql_tenant_data_migrations"}
        if combined:
            preserved |= {"seaql_migrations", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}
        all_tables = business | preserved | (set(full_control) if combined else set())
        def table(name):
            rows = 2 if name == "ryframe_resource_ownership" and combined else 1 if name in preserved or name == "biz_tenant_target_slot" else 0
            return {"table": name, "rows": rows, "sha256": plan_hash({"table": name, "rows": rows})}
        return {"scope_id": self.scope, "control_schema_fingerprint": "c" * 16, "tenant_schema_fingerprint": "d" * 64,
                "target": {"database": {**{name: declared[name] for name in ("key", "kind", "server_uuid", "database")},
                    "shared": declared["mode"] == "shared", "placements": [], "tables": [table(name) for name in sorted(all_tables - preserved)]},
                    "preserved_tables": [table(name) for name in sorted(preserved)]}}

    def test_separate_execution_root_keeps_runtime_defaults_and_output_in_coordinator(self):
        execution = self.backend / "execution"
        for relative in ("sql/ryframe_config.sql", "crates/ryframe-tenant-db/src/generated/catalog.rs",
                         "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs"):
            destination = execution / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(self.backend / relative, destination)
        maintenance = execution / ".local-tests/build.json"
        maintenance.parent.mkdir()
        maintenance.write_bytes(self.maintenance_file.read_bytes())
        self.execution_backend = execution
        value = inventory.capture_side_inventory(execution, "source", self.tools, maintenance, self.output,
                    environment=self.environment, evidence_root=self.backend)
        self.assertEqual(value.receipt["status"], "side_inventory_captured")
        self.assertEqual(value.binding["configuration"]["directory"], str(self.backend / "config"))
        with patch("subprocess.run", side_effect=AssertionError("只读复核不运行外部命令")):
            result = inventory.verify_side_inventory(self.backend, value.receipt_file, self.selected, execution)
        self.assertEqual(set(result["databases"]), set(inventory.KEYS))

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
                result = f"{'0' * 36 if self.uuid_failure else UUID}\t{db_name}"
            else:
                self.assertEqual(sql, "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;")
                owners = inventory.expected_owners(self.scope, declared["kind"] == "combined")
                result = "\n".join("\t".join(row[name] for name in ("resource_kind", "scope_id", "marker")) for row in owners)
                if self.owner_failure:
                    result += "\nextra\tother\tother"
            return mysql_output(command, kwargs, result.encode())
        self.assertEqual(command[0], self.maintenance["artifacts"]["tenant-data"]["executable"])
        self.assertEqual(command[1], "target-inventory")
        self.assertEqual(len(command), 6)
        self.assertTrue(kwargs["check"])
        self.assertEqual(kwargs["cwd"], getattr(self, "execution_backend", self.backend))
        key = command[3]
        value = self.raw(key)
        self.cli_count += 1
        if self.effect:
            self.effect(self.cli_count, value)
        if self.raw_override != "missing":
            content = json.dumps(value, ensure_ascii=False) if self.raw_override is None else self.raw_override
            Path(command[5]).write_text(content, encoding="utf-8")
        return subprocess.CompletedProcess(command, 0, "完成".encode(), b"")

    def capture(self, side="source"):
        return inventory.capture_side_inventory(self.backend, side, self.tools, self.maintenance_file,
                                                self.output, environment=self.environment)

    def assert_failed(self):
        with self.assertRaises(inventory.InventoryCaptureError):
            self.capture()
        failure = json.loads((self.output / "failure.json").read_text(encoding="utf-8"))
        self.assertFalse(failure["clone_verified"])
        self.assertFalse((self.output / "inventory.json").exists())
        return failure

    def test_four_actual_cli_targets_repeat_without_writes_and_preserve_complete_images(self):
        result = self.capture()
        self.assertEqual(self.cli_count, 8)
        self.assertGreaterEqual(self.verify_mock.call_count, 10)
        self.assertEqual(result.receipt["keys"], list(inventory.KEYS))
        self.assertEqual(len(result.receipt["inventories"]), 8)
        self.assertEqual(result.receipt["remote_writes"], 0)
        self.assertFalse(result.receipt["producer_stopped_proven"])
        self.assertFalse(result.receipt["fresh_target_proven"])
        self.assertFalse(result.receipt["target_ready"])
        self.assertFalse(result.binding["source"]["snapshot"]["clean"])
        first = result.observations[0]
        self.assertIsInstance(first, DatabaseObservation)
        self.assertIn("sys_tenant_data_backup_point", first.preserved)
        self.assertEqual(first.schema_sha256, plan_hash({"control_schema_fingerprint": "c" * 16, "tenant_schema_fingerprint": "d" * 64}))
        self.assertNotIn("sys_tenant_data_backup_point", first.tables)
        self.assertEqual(set(first.tables) | set(first.preserved), set(first.all_tables))
        self.assertTrue(set(first.preserved).issubset(EXCLUDED | {"seaql_tenant_data_migrations"}))
        for current in result.observations[1:]:
            self.assertEqual(current.schema_sha256, plan_hash({"control_schema_fingerprint": None, "tenant_schema_fingerprint": "d" * 64}))
            self.assertEqual(set(current.preserved), {"ryframe_resource_ownership", "seaql_tenant_data_migrations"})
            self.assertEqual(set(current.tables), {"biz_tenant_fence", "biz_tenant_target_slot"})
        text = "\n".join(path.read_text(encoding="utf-8") for path in self.output.glob("*.json"))
        for secret in ("fixture-secret", "access-fixture", "secret-fixture"):
            self.assertNotIn(secret, text)

    def test_target_side_uses_only_explicit_selected_scope(self):
        self.tools.plan = {"target": self.selected, "tools": self.tools.plan["tools"]}
        self.assertEqual(self.capture("target").side, "target")

    def test_missing_key_rejects_before_external_calls(self):
        self.selected["databases"].pop()
        self.assert_failed()
        self.assertEqual(self.calls, [])

    def test_duplicate_key_or_physical_identity_rejects_before_external_calls(self):
        for field in ("key", "database"):
            with self.subTest(field=field):
                self.output = self.local / ("duplicate-" + field)
                original = self.selected["databases"][1][field]
                self.selected["databases"][1][field] = self.selected["databases"][0][field]
                self.assert_failed()
                self.selected["databases"][1][field] = original
                self.assertEqual(self.calls, [])

    def test_wrong_mode_or_config_database_rejects_before_external_calls(self):
        self.selected["databases"][2]["mode"] = "shared"
        self.assert_failed()
        self.assertEqual(self.calls, [])
        self.selected["databases"][2]["mode"] = "dedicated"
        self.output = self.local / "wrong-config"
        self.environment["APP_DATABASE_NAME"] = "another_owned_database"
        self.assert_failed()
        self.assertEqual(self.calls, [])

    def test_source_tool_failure_never_calls_database_or_publishes_success(self):
        self.verify_mock.side_effect = ValueError("changed source")
        self.assert_failed()
        self.assertEqual(self.calls, [])

    def test_ambient_environment_is_not_forwarded_to_explicit_cli_or_mysql(self):
        with patch.dict(os.environ, {"APP_SCOPE_ID": "other-scope", "MYSQL_PWD": "implicit-password"}):
            self.capture()
        self.assertEqual(self.cli_count, 8)

    def test_linked_config_is_rejected_before_external_calls(self):
        original = inventory.linked
        with patch.object(inventory, "linked", side_effect=lambda path: path == self.backend / "config" or original(path)):
            self.assert_failed()
        self.assertEqual(self.calls, [])

    def test_wrong_uuid_or_extra_owner_rejects_before_cli(self):
        self.uuid_failure = True
        self.assert_failed()
        self.assertEqual(self.cli_count, 0)
        self.output = self.local / "owner-failure"
        self.uuid_failure, self.owner_failure = False, True
        self.assert_failed()
        self.assertEqual(self.cli_count, 0)

    def test_changed_rows_between_full_passes_preserve_failure_and_raw_results(self):
        def effect(count, value):
            if count == 5:
                value["target"]["database"]["tables"][0]["sha256"] = "e" * 64
        self.effect = effect
        self.assert_failed()
        self.assertEqual(len(list(self.output.glob("*-target-*.json"))), 8)

    def test_changed_schema_between_targets_rejects(self):
        self.effect = lambda count, value: value.update(tenant_schema_fingerprint="e" * 64) if count == 2 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 2)

    def test_configuration_change_is_detected_before_next_cli(self):
        def effect(count, _value):
            if count == 1:
                path = self.backend / "config/app.toml"
                path.write_text(path.read_text(encoding="utf-8") + "\n# 改变\n", encoding="utf-8")
        self.effect = effect
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_environment_mutation_rejects_without_using_new_binding(self):
        self.effect = lambda count, _: self.environment.update(APP_SOURCE_PASSWORD="changed") if count == 1 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_plan_mutation_rejects_without_using_new_binding(self):
        self.effect = lambda count, _: self.selected.update(scope_id="changed-scope") if count == 1 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_changed_defaults_fail_before_next_cli(self):
        self.effect = lambda count, _: self.client.write_text("changed", encoding="utf-8") if count == 1 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_changed_cli_bytes_fail_before_next_cli(self):
        executable = Path(self.maintenance["artifacts"]["tenant-data"]["executable"])
        self.effect = lambda count, _: executable.write_bytes(b"changed") if count == 1 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_changed_mysql_tool_is_not_called_again(self):
        self.effect = lambda count, _: self.external.write_bytes(b"changed") if count == 1 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_changed_maintenance_receipt_is_not_rebound(self):
        self.effect = lambda count, _: self.maintenance_file.write_text("{}", encoding="utf-8") if count == 1 else None
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)

    def test_modified_saved_inventory_is_detected_before_publish(self):
        def effect(count, _):
            if count == 8:
                (self.output / "before-target-shared.json").write_text("{}", encoding="utf-8")
        self.effect = effect
        self.assert_failed()
        self.assertEqual(self.cli_count, 8)

    def test_missing_malformed_and_duplicate_json_keep_failure(self):
        for index, content in enumerate(("missing", "[", '{"scope_id":"x","scope_id":"y"}')):
            with self.subTest(index=index):
                self.output = self.local / f"invalid-output-{index}"
                self.raw_override = content
                self.assert_failed()
                self.assertEqual(self.cli_count, index + 1)

    def test_final_source_change_cannot_publish_inventory(self):
        calls = 0
        def verify(*_):
            nonlocal calls
            calls += 1
            if calls == 10:
                raise ValueError("final source changed")
            return copy.deepcopy(self.maintenance)
        self.verify_mock.side_effect = verify
        self.assert_failed()
        self.assertEqual(self.cli_count, 8)

    def test_timeout_preserves_diagnostic_without_retry(self):
        def effect(_count, _value):
            raise subprocess.TimeoutExpired(["fixture"], 1800, output=b"fixture-secret", stderr=b"secret-fixture")
        self.effect = effect
        self.assert_failed()
        self.assertEqual(self.cli_count, 1)
        diagnostics = [json.loads(path.read_text(encoding="utf-8")) for path in self.output.glob("command-*.json")]
        timed_out = [value for value in diagnostics if value["error_type"] == "TimeoutExpired"]
        self.assertEqual(len(timed_out), 1)
        self.assertEqual(timed_out[0]["stdout"], "[REDACTED]")
        self.assertEqual(timed_out[0]["stderr"], "[REDACTED]")

    def test_failed_process_records_redacted_diagnostics_and_never_success(self):
        def effect(_count, _value):
            raise subprocess.CalledProcessError(1, ["fixture"], output=b"fixture-secret access-fixture", stderr=b"secret-fixture")
        self.effect = effect
        self.assert_failed()
        text = "\n".join(path.read_text(encoding="utf-8") for path in self.output.glob("*.json"))
        self.assertIn("[REDACTED]", text)
        for value in ("fixture-secret", "access-fixture", "secret-fixture"):
            self.assertNotIn(value, text)

    def test_conversion_rejects_unknown_missing_duplicate_and_wrongly_grouped_tables(self):
        declared = self.selected["databases"][0]
        owners = inventory.expected_owners(self.scope, True)
        changes = (
            lambda value: value["target"]["database"]["tables"].append({"table": "unknown", "rows": 0, "sha256": "f" * 64}),
            lambda value: value["target"]["preserved_tables"].pop(),
            lambda value: value["target"]["database"]["tables"].append(copy.deepcopy(value["target"]["database"]["tables"][0])),
            lambda value: value["target"]["preserved_tables"].append(value["target"]["database"]["tables"].pop()),
            lambda value: value["target"]["database"]["tables"][0].update(rows=True),
            lambda value: value["target"]["database"].update(shared=1),
            lambda value: value.update(scope_id="wrong-scope"),
            lambda value: value.update(control_schema_fingerprint="f" * 64),
            lambda value: value.update(tenant_schema_fingerprint="f" * 16),
            lambda value: value["target"]["database"].update(placements=[{"tenant_id": "system", "generation": True, "switch_token": "token"}]),
        )
        for change in changes:
            with self.subTest(change=changes.index(change)):
                value = self.raw("shared-control")
                change(value)
                with self.assertRaises(ValueError):
                    inventory.observation_from_inventory(self.backend, value, declared, self.scope, owners)
        with self.assertRaises(ValueError):
            inventory.observation_from_inventory(self.backend, self.raw("shared-control"), declared, self.scope, owners[:1])

    def test_existing_output_never_gets_overwritten(self):
        self.output.mkdir()
        marker = self.output / "marker.txt"
        marker.write_text("保留", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.capture()
        self.assertEqual(marker.read_text(encoding="utf-8"), "保留")
        self.assertEqual(self.calls, [])

    def test_unsupported_side_creates_only_failure_evidence(self):
        with self.assertRaises(inventory.InventoryCaptureError):
            self.capture("other")
        self.assertTrue((self.output / "failure.json").is_file())
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
