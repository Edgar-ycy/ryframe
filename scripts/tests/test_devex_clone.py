import copy
from datetime import datetime, timezone
from decimal import Decimal
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from devex_clone import read_json, verify_plan, write_plan
from devex_clone_model import create_plan, local_path, object_plan
from devex_clone_rows import EXCLUDED, literals, parse_row, reject_physical, schema_catalog, validate_state
from devex_clone_schedule import canonical_value, schedule_row_sha256
from restore_build import file_digest, verify_build
from restore_reference_io import COPY_METADATA_FIELDS, DATA_HEADER
from restore_reference_plan import BUCKETS

REPO = Path(__file__).resolve().parents[2]
UUID = "67cd8d4a-fe90-11ef-98fd-5847ca786499"


def sql_value(value):
    if value is None:
        return "NULL"
    if isinstance(value, (dict, list)):
        value = json.dumps(value, ensure_ascii=False)
    if isinstance(value, str):
        return "'" + value.replace("\\", "\\\\").replace("'", "\\'").replace("\n", "\\n") + "'"
    return str(value)


class CloneTests(unittest.TestCase):
    def setUp(self):
        location = REPO / ".local-tests/python-unit"
        location.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=location)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        for relative in ("sql/ryframe_config.sql", "crates/ryframe-tenant-db/src/generated/catalog.rs",
                         "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs"):
            destination = self.backend / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(REPO / relative, destination)
        self.root = self.backend / ".local-tests/export"
        self.root.mkdir(parents=True)
        self.control, self.tenant = schema_catalog(self.backend)
        self.data = {"shared-control": [], "dedicated-a": []}
        self.value = {"format_version": 1, "copy_id": "clone-case", "copy_stage": "source_to_seed", "target_schedule_actions": [],
                      "app_env": "test", "artifact_root": str(self.root),
                      "source_snapshot": {"head": "a" * 40, "clean": False, "worktree_sha256": "b" * 64},
                      "source": self.side("clone-source"), "target": {**self.side("clone-target"), "state": "new_initialized", "ever_started": False},
                      "evidence": {}, "databases": [], "objects": []}
        for role in ("source_stopped", "target_initialized", "target_stopped"):
            self.value["evidence"][role] = self.artifact(f"{role}.json", b'{"fixture":"offline evidence binding only"}')
        for bucket in sorted(BUCKETS):
            entries = []
            if bucket == "uploads":
                entries = [{"key": "clone-source/system/file.txt", "artifact": self.artifact("file.bin", b"data"),
                            "metadata": {**dict.fromkeys(COPY_METADATA_FIELDS), "ContentType": "text/plain", "Metadata": {}}}]
            self.value["objects"].append({"bucket": bucket, "entries": entries})
        for tenant, target in (("system", "shared-control"), ("clone-source-01", "dedicated-a")):
            self.add("shared-control", "sys_tenant", tenant_id=tenant, status="enabled")
            self.add("shared-control", "sys_tenant_data_placement", tenant_id=tenant, current_target_key=target,
                     placement_generation=1, switch_token="token", state="active")
            self.add(target, "biz_tenant_fence", tenant_id=tenant, target_key=target,
                     placement_generation=1, switch_token="token", state="active")
        self.add("shared-control", "biz_tenant_target_slot", slot_id=1)
        self.add("dedicated-a", "biz_tenant_target_slot", slot_id=1, tenant_id="clone-source-01", placement_generation=1, switch_token="token")
        self.add("shared-control", "sys_file", id=1, tenant_id="system", bucket="uploads", storage_path="system/file.txt",
                 file_url="uploads/system/file.txt", file_size=4, file_sha256=hashlib.sha256(b"data").hexdigest(),
                 upload_status="ready", del_flag="0")
        self.refresh()

    def side(self, scope):
        databases = []
        for key, kind, mode in (("shared-control", "combined", "shared"), ("dedicated-a", "tenant", "dedicated")):
            owners = ["control", "tenant-data"] if kind == "combined" else ["tenant-data"]
            databases.append({"key": key, "kind": kind, "mode": mode, "server_uuid": UUID,
                              "database": scope.replace("-", "_") + "_" + key.replace("-", "_"), "schema_sha256": "c" * 64,
                              "ownership": [{"resource_kind": item, "scope_id": scope, "marker": f"ryframe-owner:v1:{scope}:{item}"} for item in owners]})
        return {"scope_id": scope, "object_endpoint": "http://127.0.0.1:29200", "databases": databases}

    def artifact(self, filename, content):
        path = self.root / filename
        path.write_bytes(content)
        return {"file": filename, **file_digest(path)}

    def add(self, key, table, **values):
        catalog = self.control | self.tenant if key == "shared-control" else self.tenant
        row = dict.fromkeys(catalog[table])
        row.update(values)
        self.data[key].append((table, row))
        return row

    def refresh(self):
        self.value["databases"] = []
        for key, values in self.data.items():
            catalog = self.control | self.tenant if key == "shared-control" else self.tenant
            counts, lines = dict.fromkeys(catalog, 0), [DATA_HEADER]
            for table, row in values:
                counts[table] += 1
                columns = ", ".join(f"`{column}`" for column in row)
                literals_ = ",".join(sql_value(value) for value in row.values())
                lines.append(f"INSERT INTO `{table}` ({columns}) VALUES ({literals_});\n")
            self.value["databases"].append({"key": key, "tables": counts,
                                           "artifact": self.artifact(key + ".sql", "".join(lines).encode())})

    def test_dirty_offline_plan_preserves_logical_rows_maps_only_object_prefix_and_never_executes(self):
        before = {path.name: path.read_bytes() for path in self.root.iterdir()}
        result = create_plan(self.value, self.backend)
        self.assertFalse(result["source_snapshot"]["clean"])
        self.assertEqual(result["status"], "offline_verified")
        self.assertFalse(result["execution_authorized"])
        self.assertEqual(result["objects"][0]["target_key"], "clone-target/system/file.txt")
        self.assertEqual(result["objects"][0]["metadata"]["ContentType"], "text/plain")
        self.assertEqual(set(result["excluded_tables"]), EXCLUDED)
        self.assertIn("sys_tenant_data_backup_point", EXCLUDED)
        self.assertEqual(before, {path.name: path.read_bytes() for path in self.root.iterdir()})

    def test_object_metadata_is_complete_bound_to_plan_and_verified_readonly(self):
        original = create_plan(self.value, self.backend)
        item = self.value["objects"][-1]["entries"][0]
        value = copy.deepcopy(self.value)
        value["objects"][-1]["entries"][0].pop("metadata")
        with self.assertRaises(ValueError):
            create_plan(value, self.backend)
        for field in COPY_METADATA_FIELDS:
            value = copy.deepcopy(self.value)
            value["objects"][-1]["entries"][0]["metadata"].pop(field)
            with self.assertRaises(ValueError):
                create_plan(value, self.backend)
        item["metadata"].update(ContentType="text/plain; charset=utf-8", CacheControl="private, max-age=60",
                                ContentDisposition='attachment; filename="report.txt"', Metadata={"purpose": "fixture"})
        changed = create_plan(self.value, self.backend)
        self.assertEqual(changed["objects"][0]["metadata"], item["metadata"])
        self.assertNotEqual(changed["plan_sha256"], original["plan_sha256"])
        self.assertFalse(changed["execution_authorized"])
        filename, output = self.backend / ".local-tests/metadata-input.json", self.backend / ".local-tests/metadata-plan.json"
        filename.write_text(json.dumps(self.value), encoding="utf-8")
        write_plan(self.backend, str(filename), str(output))
        before = output.read_bytes()
        self.assertEqual(verify_plan(self.backend, str(output)), changed)
        self.assertEqual(output.read_bytes(), before)
        item["metadata"]["Metadata"]["purpose"] = "altered"
        filename.write_text(json.dumps(self.value), encoding="utf-8")
        with self.assertRaises(ValueError):
            verify_plan(self.backend, str(output))

    def test_object_mapping_checks_both_source_and_longer_target_utf8_key_limits(self):
        value = copy.deepcopy(self.value)
        item = value["objects"][-1]["entries"][0]
        prefix = value["source"]["scope_id"] + "/"
        item["key"] = prefix + "x" * (1024 - len(prefix.encode()))
        objects, _ = object_plan(value, self.root)
        self.assertEqual(len(objects[0]["target_key"].encode()), 1024)
        value["target"]["scope_id"] += "-longer"
        with self.assertRaises(ValueError):
            object_plan(value, self.root)
        item["key"] = prefix + "中" * 340
        with self.assertRaises(ValueError):
            object_plan(value, self.root)

    def test_formal_schema_complete_columns_counts_and_unknown_table_are_rejected(self):
        for mutate in (lambda value: value["databases"][0]["tables"].pop("sys_background_job"),
                       lambda value: value["databases"][0]["tables"].update(sys_unknown=0),
                       lambda value: value["databases"][0]["tables"].update(sys_tenant=3),
                       lambda value: value["databases"][0]["tables"].update(sys_backup_set=0)):
            value = copy.deepcopy(self.value); mutate(value)
            with self.assertRaises(ValueError):
                create_plan(value, self.backend)
        for line in ("INSERT INTO `sys_tenant` (`tenant_id`) VALUES ('system');",
                     "INSERT INTO `sys_tenant` (`tenant_id`, `tenant_id`) VALUES ('system','system');"):
            with self.assertRaises(ValueError):
                parse_row(line, self.control)

    def test_source_target_overlap_unknown_owner_and_preexisting_target_fail_closed(self):
        for mutate in (lambda value: value["target"].update(state="existing"),
                       lambda value: value["target"].update(ever_started=True),
                       lambda value: value["target"]["databases"][0].update(database=value["source"]["databases"][0]["database"]),
                       lambda value: value["target"]["databases"][0].update(ownership=value["source"]["databases"][0]["ownership"]),
                       lambda value: value["target"]["databases"][0].update(schema_sha256="d" * 64),
                       lambda value: value.update(app_env="production")):
            value = copy.deepcopy(self.value); mutate(value)
            with self.assertRaises(ValueError):
                create_plan(value, self.backend)

    def test_active_jobs_outbox_schedule_lease_import_and_workflows_are_rejected(self):
        for table, row in (("sys_background_job", {"status": "pending"}), ("sys_outbox_event", {"status": "running"}),
                           ("sys_user_import_job", {"status": "pending"}), ("sys_export_job", {"status": "queued"}),
                           ("sys_tenant_config_transfer", {"status": "apply_pending"}),
                           ("sys_tenant_config_bundle", {"status": "running"}),
                           ("sys_job_schedule", {"del_flag": "0", "enabled": 1}),
                           ("sys_tenant_operation_lease", {}), ("sys_tenant_data_migration", {"state": "finalized"}),
                           ("sys_file", {"upload_status": "cleanup"}), ("password_reset_requests", {"status": "pending"})):
            with self.subTest(table=table), self.assertRaises(ValueError):
                validate_state(table, row)

    def test_terminal_jobs_and_closed_lease_expired_attempt_preserve_unknown_finish(self):
        self.add("shared-control", "sys_background_job", id=7, status="succeeded", completed_at="2026-09-01 00:00:00",
                 payload={"tenant_id": "system", "target_key": "shared-control"})
        self.add("shared-control", "sys_background_job_attempt", job_id=7, sequence=1, outcome="lease_expired", closed_at="2026-09-01 00:00:00")
        self.refresh()
        self.assertEqual(create_plan(self.value, self.backend)["status"], "offline_verified")
        for row in ({"outcome": "running", "closed_at": None, "finished_at": None},
                    {"outcome": "lease_expired", "closed_at": "time", "finished_at": "invented"},
                    {"outcome": "failed", "closed_at": "time", "finished_at": None}):
            with self.assertRaises(ValueError):
                validate_state("sys_background_job_attempt", row)
        self.data["shared-control"] = [(table, row) for table, row in self.data["shared-control"] if table != "sys_background_job"]
        self.refresh()
        with self.assertRaisesRegex(ValueError, "尝试引用"):
            create_plan(self.value, self.backend)

    def test_decoded_json_hex_and_escaped_source_physical_references_are_rejected(self):
        for value in ({"nested": [{"destination": "clone-source/file"}]}, b"clone-source/file", "http://127.0.0.1:29200/file"):
            with self.assertRaises(ValueError):
                reject_physical(value, ["clone-source", "http://127.0.0.1:29200"])
        self.add("shared-control", "sys_background_job", id=7, status="dead", completed_at="2026-09-01 00:00:00",
                 payload={"destination": "clone-source/object"})
        self.refresh()
        with self.assertRaisesRegex(ValueError, "物理"):
            create_plan(self.value, self.backend)
        self.assertEqual(literals("'escaped\\'text',0x636c6f6e652d736f75726365,NULL,-12.5"), ["escaped'text", b"clone-source", None, -12.5])
        for value in ("SLEEP(1)", "1); DROP TABLE data", "'bad\\q'", "1,", "_binary"):
            with self.assertRaises(ValueError):
                literals(value)

    def test_declared_logical_tenant_survives_audit_json_and_logical_object_path(self):
        self.add("shared-control", "sys_oper_log", id=8, tenant_id="system",
                 oper_param=json.dumps({"tenant_id": "clone-source-01"}),
                 json_result=json.dumps({"data": {"tenant_id": "clone-source-01"}}))
        file = next(row for table, row in self.data["shared-control"] if table == "sys_file")
        file.update(tenant_id="clone-source-01", storage_path="clone-source-01/file.txt", file_url="uploads/clone-source-01/file.txt")
        self.value["objects"][-1]["entries"][0]["key"] = "clone-source/clone-source-01/file.txt"
        self.refresh()
        result = create_plan(self.value, self.backend)
        self.assertEqual(result["objects"][0]["target_key"], "clone-target/clone-source-01/file.txt")
        for value in ("clone-source-02", "clone-source/clone-source-01/file.txt", {"scope_id": "clone-source-01"},
                      {"endpoint": "http://127.0.0.1:29200/clone-source-01"}, "clone_source_shared_control"):
            with self.assertRaises(ValueError):
                reject_physical(value, ["clone-source", "http://127.0.0.1:29200", "clone_source_shared_control"], {"clone-source-01"})

    def test_shared_slot_and_unknown_tenant_baseline_are_checked(self):
        slot = next(row for table, row in self.data["shared-control"] if table == "biz_tenant_target_slot")
        slot.update(tenant_id="system", placement_generation=1, switch_token="token")
        self.refresh()
        with self.assertRaisesRegex(ValueError, "空 slot"):
            create_plan(self.value, self.backend)
        schema = self.backend / "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs"
        with schema.open("a", encoding="utf-8") as stream:
            stream.write("\nCREATE TABLE IF NOT EXISTS `biz_future` (\n    `id` BIGINT\n) ENGINE=InnoDB;\n")
        with self.assertRaisesRegex(ValueError, "租户基线含未审查"):
            create_plan(self.value, self.backend)

    def test_windows_reparse_point_is_rejected_even_within_evidence_root(self):
        filename = self.backend / ".local-tests/input.json"
        filename.write_text("{}", encoding="utf-8")
        original = Path.lstat
        def attributes(path):
            if path == filename:
                return type("Reparse", (), {"st_file_attributes": 0x400, "st_mode": original(path).st_mode})()
            return original(path)
        with patch.object(Path, "lstat", attributes), self.assertRaisesRegex(ValueError, "链接"):
            local_path(self.backend, str(filename))

    def test_placement_fence_slot_and_object_metadata_mismatch_rejected(self):
        for table, field, value in (("biz_tenant_fence", "state", "frozen"),
                                    ("biz_tenant_fence", "switch_token", "stale"),
                                    ("biz_tenant_target_slot", "tenant_id", "other")):
            original = copy.deepcopy(self.data)
            row = next(row for name_, row in self.data["dedicated-a"] if name_ == table)
            row[field] = value; self.refresh()
            with self.assertRaises(ValueError):
                create_plan(self.value, self.backend)
            self.data = original
        self.refresh()
        item = self.value["objects"][-1]["entries"][0]
        original = item["key"]
        for value in ("other/file", "clone-source/.ryframe-owner", "clone-source/../outside", "clone-source/other-file"):
            item["key"] = value
            with self.assertRaises(ValueError):
                create_plan(self.value, self.backend)
        item["key"] = original

    def test_plan_verify_readonly_tamper_and_no_overwrite(self):
        filename = self.backend / ".local-tests/input.json"
        filename.write_text(json.dumps(self.value), encoding="utf-8")
        output = self.backend / ".local-tests/plan.json"
        result = write_plan(self.backend, str(filename), str(output))
        before = file_digest(output)
        self.assertEqual(verify_plan(self.backend, str(output)), result)
        self.assertEqual(file_digest(output), before)
        with self.assertRaisesRegex(ValueError, "覆盖"):
            write_plan(self.backend, str(filename), str(output))
        with self.assertRaisesRegex(ValueError, "来源导出"):
            write_plan(self.backend, str(filename), str(self.root / "plan.json"))
        (self.root / "file.bin").write_bytes(b"tamp")
        with self.assertRaisesRegex(ValueError, "变化"):
            verify_plan(self.backend, str(output))

    def test_path_escape_duplicate_json_unknown_columns_and_generated_catalog_fail_closed(self):
        with self.assertRaises(ValueError):
            local_path(self.backend, str(self.backend / "outside.json"))
        filename = self.backend / ".local-tests/duplicate.json"
        filename.write_text('{"source":1,"source":2}')
        with self.assertRaisesRegex(ValueError, "重复"):
            read_json(filename)
        generated = self.backend / "crates/ryframe-tenant-db/src/generated/catalog.rs"
        generated.write_text("pub const GENERATED_TENANT_DATA_TABLES: &[TenantDataTableDescriptor] = &[DEVICE];")
        with self.assertRaisesRegex(ValueError, "非空生成"):
            create_plan(self.value, self.backend)

    def test_new_control_table_requires_explicit_policy_and_formal_restore_stays_clean(self):
        schema = self.backend / "sql/ryframe_config.sql"
        with schema.open("a", encoding="utf-8") as stream:
            stream.write("\nCREATE TABLE IF NOT EXISTS `sys_future_queue` (\n    `id` BIGINT\n) ENGINE=InnoDB;\n")
        with self.assertRaisesRegex(ValueError, "白名单不同"):
            create_plan(self.value, self.backend)
        with self.assertRaisesRegex(ValueError, "精确干净"):
            verify_build(self.backend, {"format_version": 1, "kind": "restore-backend-build",
                                        "source": self.value["source_snapshot"]}, "a" * 40)

    def test_cli_requires_write_and_verify_does_not_change_evidence(self):
        filename = self.backend / ".local-tests/input.json"
        filename.write_text(json.dumps(self.value), encoding="utf-8")
        output = self.backend / ".local-tests/plan.json"
        command = [sys.executable, "-X", "utf8", str(REPO / "scripts/devex_clone.py")]
        arguments = ["plan", "--backend-dir", str(self.backend), "--input", str(filename), "--output", str(output)]
        denied = subprocess.run([*command, *arguments], capture_output=True, text=True, encoding="utf-8", timeout=15)
        self.assertEqual(denied.returncode, 2)
        self.assertFalse(output.exists())
        prepared = subprocess.run([*command, *arguments, "--write"], capture_output=True, text=True, encoding="utf-8", timeout=15)
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        self.assertFalse(json.loads(prepared.stdout)["execution_authorized"])
        before = {path: file_digest(path) for path in self.backend.rglob("*") if path.is_file()}
        verified = subprocess.run([*command, "verify", "--backend-dir", str(self.backend), "--plan", str(output)],
                                  capture_output=True, text=True, encoding="utf-8", timeout=15)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertEqual(before, {path: file_digest(path) for path in self.backend.rglob("*") if path.is_file()})
        self.assertEqual(json.loads(prepared.stdout), json.loads(verified.stdout))

    def schedule(self):
        row = self.add("shared-control", "sys_job_schedule", id=3, tenant_id="system", name="数据保留清理",
                       handler_key="system.data_retention_cleanup", cron_expression="0 30 3 * * * *", timezone="UTC",
                       enabled=1, next_run_at="2026-09-04 03:30:00", version=1, del_flag="0",
                       created_at="2026-09-04 00:00:00", updated_at="2026-09-04 00:00:00")
        self.refresh()
        action = {"action": "disable_schedule_via_api", "tenant_id": "system", "schedule_id": "3",
                  "handler_key": row["handler_key"], "expected_version": 1, "source_row_sha256": schedule_row_sha256(row)}
        self.value["target_schedule_actions"] = [action]
        return row, action

    def test_exact_schedule_disposition_is_pending_and_does_not_write_source(self):
        row, action = self.schedule()
        before = {path.name: path.read_bytes() for path in self.root.iterdir()}
        result = create_plan(self.value, self.backend)
        self.assertEqual(result["status"], "offline_verified_pending_target_actions")
        self.assertFalse(result["target_ready"])
        self.assertFalse(result["execution_authorized"])
        self.assertTrue(result["worker_must_remain_stopped"])
        pending = result["pending_target_actions"][0]
        self.assertEqual(pending["body"], {"version": 1, "enabled": False})
        self.assertEqual(pending["required_permissions"], ["monitor:schedule:list", "monitor:schedule:edit"])
        self.assertFalse(pending["api_called"])
        self.assertEqual(before, {path.name: path.read_bytes() for path in self.root.iterdir()})
        for field, wrong in (("source_row_sha256", "d" * 64), ("expected_version", 2),
                             ("handler_key", "system.other"), ("schedule_id", "4"), ("tenant_id", "clone-source-01")):
            value = copy.deepcopy(self.value); value["target_schedule_actions"][0][field] = wrong
            with self.subTest(field=field), self.assertRaises(ValueError):
                create_plan(value, self.backend)
        for actions in ([], [action, action]):
            value = copy.deepcopy(self.value); value["target_schedule_actions"] = actions
            with self.assertRaises(ValueError):
                create_plan(value, self.backend)
        row["name"] = "源记录已修改"; self.refresh()
        with self.assertRaisesRegex(ValueError, "完整源行摘要"):
            create_plan(self.value, self.backend)

    def test_seed_to_arm_requires_completed_disposition_and_never_allows_implicit_other_tenant(self):
        row, _ = self.schedule()
        self.value["copy_stage"] = "seed_to_arm"
        with self.assertRaisesRegex(ValueError, "只有来源到 seed"):
            create_plan(self.value, self.backend)
        # 独立单测数据模拟目标 API 已完成后的新 seed 导出；并非真实资源操作。
        row.update(enabled=0, next_run_at=None, version=2, updated_at="2026-09-04 00:01:00")
        self.value["target_schedule_actions"] = []; self.refresh()
        result = create_plan(self.value, self.backend)
        self.assertEqual(result["status"], "offline_verified")
        self.assertEqual(result["pending_target_actions"], [])
        row.update(enabled=1, tenant_id="clone-source-01"); self.refresh()
        with self.assertRaisesRegex(ValueError, "精确匹配"):
            create_plan(self.value, self.backend)

    def test_schedule_row_digest_has_explicit_lossless_types_and_dates(self):
        self.assertEqual(canonical_value(Decimal("123456789012345678901234567890.000")),
                         ["decimal", "123456789012345678901234567890"])
        self.assertEqual(canonical_value(1), canonical_value(Decimal("1.000")))
        self.assertNotEqual(canonical_value(b"data"), canonical_value("data"))
        self.assertNotEqual(canonical_value(None), canonical_value("null"))
        self.assertNotEqual(canonical_value(True), canonical_value(1))
        self.assertEqual(schedule_row_sha256({"updated_at": "2026-09-04 00:00:00", "payload": {"n": Decimal("1.00")}}),
                         schedule_row_sha256({"payload": {"n": 1}, "updated_at": datetime(2026, 9, 4)}))
        for value in (1.5, Decimal("NaN"), Decimal("Infinity"), {1: "ambiguous"}, datetime.now(timezone.utc)):
            with self.assertRaises(ValueError):
                canonical_value(value)
        for value in ("2026-09-04T00:00:00Z", "2026-09-04 00:00:00.1234567", "2026-02-31 00:00:00"):
            with self.assertRaises(ValueError):
                schedule_row_sha256({"updated_at": value})
        row = self.add("shared-control", "sys_message", id=9, tenant_id="system")
        for value in ('{"id":1,"id":2}', '{"value":NaN}'):
            row["payload_json"] = value; self.refresh()
            with self.assertRaises(ValueError):
                create_plan(self.value, self.backend)

    def test_schedule_execution_references_exact_same_tenant_terminal_job(self):
        job = self.add("shared-control", "sys_background_job", id=7, tenant_id="system", status="succeeded",
                       completed_at="2026-09-04 00:00:00")
        execution = self.add("shared-control", "sys_job_schedule_execution", id=8, tenant_id="system", schedule_id=3,
                             outcome="enqueued", background_job_id=7)
        self.refresh()
        self.assertEqual(create_plan(self.value, self.backend)["status"], "offline_verified")
        for outcome in ("skipped_misfire", "skipped_concurrency", "target_unavailable", "invalid_configuration"):
            execution.update(outcome=outcome, background_job_id=None); self.refresh()
            self.assertEqual(create_plan(self.value, self.backend)["status"], "offline_verified")
        for outcome, parent in (("pending", None), ("enqueued", None), ("skipped_misfire", 7)):
            execution.update(outcome=outcome, background_job_id=parent); self.refresh()
            with self.assertRaises(ValueError):
                create_plan(self.value, self.backend)
        execution.update(outcome="enqueued", background_job_id=99); self.refresh()
        with self.assertRaisesRegex(ValueError, "缺少父任务证据"):
            create_plan(self.value, self.backend)
        execution["background_job_id"] = 7; job["tenant_id"] = "clone-source-01"; self.refresh()
        with self.assertRaisesRegex(ValueError, "同一租户"):
            create_plan(self.value, self.backend)
        job.update(tenant_id="system", status="pending"); self.refresh()
        with self.assertRaisesRegex(ValueError, "可执行任务"):
            create_plan(self.value, self.backend)

    def test_provision_request_is_retained_only_for_completed_tenant_and_active_placement(self):
        request = self.add("shared-control", "sys_tenant_provision_request", tenant_id="clone-source-01",
                           request_token="a" * 64, admin_password_hash="unit-test-only-hash")
        tenant = next(row for table, row in self.data["shared-control"] if table == "sys_tenant" and row["tenant_id"] != "system")
        for status in ("enabled", "disabled"):
            tenant["status"] = status; self.refresh()
            self.assertEqual(create_plan(self.value, self.backend)["status"], "offline_verified")
        for status in ("provisioning", "provisioning_failed"):
            tenant["status"] = status; self.refresh()
            with self.assertRaisesRegex(ValueError, "开通"):
                create_plan(self.value, self.backend)
        tenant["status"] = "enabled"; request["tenant_id"] = "missing-tenant"; self.refresh()
        with self.assertRaisesRegex(ValueError, "未登记的逻辑租户"):
            create_plan(self.value, self.backend)


if __name__ == "__main__":
    unittest.main()
