"""完整源导出离线回归；真实协议与真实数据不由这些模型测试替代。"""
import copy
import errno
import json
from pathlib import Path
import re
import socket
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_source as source
import devex_clone_source_proof as proof
from devex_clone_export import schema_snapshot
from devex_clone_model import schema_fingerprints
from devex_clone_source_fixture import SourceFixture
from devex_clone_jobs_fixture import add_cleanup
from restore_reference_io import ExternalTools


class SourceExportTests(unittest.TestCase):
    def setUp(self):
        self.fixture = SourceFixture(self)

    def export(self):
        item = self.fixture
        item.inventory_count = 0
        return source.export_source(item.backend, item.request_path, item.output, item)

    def failed(self, stage=None):
        with self.assertRaises(Exception):
            self.export()
        result = json.loads((self.fixture.output / "failure.json").read_text(encoding="utf-8"))
        self.assertFalse((self.fixture.output / "export.json").exists())
        self.assertFalse(result["clone_verified"])
        self.assertFalse(result["restore_qualified"])
        if stage:
            self.assertEqual(result["stage"], stage)
        return result

    def test_complete_dirty_source_export_preserves_empty_tables_and_never_writes_remote(self):
        result = self.export()
        self.assertEqual(result["status"], "source_export_captured")
        self.assertFalse(result["source_snapshot"]["clean"])
        self.assertFalse(result["dump_semantic_digest_verified"])
        self.assertFalse(result["clone_verified"])
        self.assertFalse(result["restore_qualified"])
        self.assertTrue(result["requires_offline_plan"])
        self.assertTrue(result["operator_declared_producers_only"])
        self.assertEqual(result["remote_writes"], 0)
        self.assertEqual(len(result["objects"]), 5)
        self.assertEqual(len(result["databases"]), 2)
        control = next(item for item in result["databases"] if item["key"] == "shared-control")
        self.assertEqual(control["tables"]["sys_background_job"], 0)
        self.assertNotIn("sys_backup_set", control["tables"])
        self.assertNotIn("sys_tenant_data_backup_point", control["tables"])
        self.assertEqual(result["enabled_system_schedule_rows"], [])
        self.assertEqual(self.fixture.inventory_count, 2)
        calls = self.fixture.calls
        self.assertEqual(sum("verify" in command for command in calls), 6)
        self.assertTrue(all("up" not in command and "--all" not in command and "backup-register" not in command for command in calls))
        self.assertTrue(all(Path(command[0]).stem != "ryframe-reset" for command in calls))
        uploads = next(bucket for bucket in result["objects"] if bucket["bucket"] == "uploads")
        self.assertTrue(uploads["entries"][0]["get_header_consistent"])
        self.assertEqual((self.fixture.output / uploads["entries"][0]["artifact"]["file"]).read_bytes(), b"data")
        self.assertNotEqual(control["artifact"]["sha256"], result["logical_inventory_sha256"])

    def test_schema_snapshot_combined_shared_and_dedicated_use_actual_ledgers(self):
        item = self.fixture
        shared = copy.deepcopy(item.request["source"]["databases"][-1])
        shared.update(key="shared-empty", mode="shared", database="clone_source_shared_empty")
        item.request["source"]["databases"].append(shared)
        item.output.mkdir()
        tools = ExternalTools({"source": item.request["source"], "tools": item.request["tools"]}, item.output, item)
        result = schema_snapshot(tools, item.models, "ledger-kinds")
        for database in result:
            actual = {table for table in database["columns"] if table.startswith("seaql_")}
            expected = {"seaql_tenant_data_migrations"} | ({"seaql_migrations"} if database["key"] == "shared-control" else set())
            self.assertEqual(actual, expected)
        self.assertEqual(len(result), 3)

    def test_missing_wrong_and_unknown_ledger_reject_before_source_dump(self):
        cases = [
            ("shared-control", {"seaql_migrations"}),
            ("shared-control", {"seaql_tenant_data_migrations"}),
            ("shared-control", {"seaql_migrations", "seaql_tenant_data_migrations", "seaql_unknown"}),
            ("dedicated-a", set()),
            ("dedicated-a", {"seaql_migrations"}),
            ("dedicated-a", {"seaql_migrations", "seaql_tenant_data_migrations"}),
        ]
        for index, (key, ledgers) in enumerate(cases):
            with self.subTest(key=key, ledgers=ledgers):
                self.fixture.output = self.fixture.local / f"ledger-failure-{index}"
                self.fixture.ledger_sets = {key: ledgers}
                self.failed("source_identity_and_schema")
                self.assertFalse(any("--no-create-info" in command for command in self.fixture.calls))

    def test_ledger_names_are_bound_to_current_rust_migrators(self):
        root = Path(__file__).resolve().parents[2]
        for relative, constant, expected in (
            ("crates/ryframe-db/src/migration/mod.rs", "CONTROL_MIGRATION_LEDGER", "seaql_migrations"),
            ("crates/ryframe-tenant-db/src/migration/status.rs", "TENANT_DATA_MIGRATION_LEDGER", "seaql_tenant_data_migrations"),
        ):
            content = (root / relative).read_text(encoding="utf-8")
            matched = re.search(rf'pub const {constant}: &str = "([^"]+)";', content)
            self.assertIsNotNone(matched)
            self.assertEqual(matched[1], expected)

    def test_wrong_uuid_or_owner_refuses_source_before_dump(self):
        for field in ("uuid_failure", "owner_failure"):
            with self.subTest(failure=field):
                self.fixture.output = self.fixture.local / ("changed-" + field)
                setattr(self.fixture, field, True)
                self.failed("source_identity_and_schema")
                self.assertFalse(any("--no-create-info" in command for command in self.fixture.calls))
                setattr(self.fixture, field, False)

    def test_unknown_table_missing_target_and_omitted_zero_row_table_fail_closed(self):
        for field, stage in (("unknown_table", "source_identity_and_schema"), ("unknown_column", "source_identity_and_schema"),
                             ("missing_target", "inventory_before"),
                             ("omit_empty_table", "inventory_before")):
            with self.subTest(failure=field):
                self.fixture.output = self.fixture.local / field
                setattr(self.fixture, field, True)
                self.failed(stage)
                setattr(self.fixture, field, False)

    def test_before_after_logical_digest_difference_refuses_whole_export(self):
        self.fixture.inventory_change = True
        self.failed("inventory_after")
        self.assertTrue((self.fixture.output / "databases/shared-control.sql").exists())
        self.assertTrue((self.fixture.output / "inventory-after.json").exists())

    def test_invalid_sql_relations_fail_before_any_business_object_capture(self):
        _, job, _ = add_cleanup(self.fixture.case)
        job["job_type"] = "unreviewed.cleanup"
        self.fixture.case.refresh()
        with patch.object(source, "capture_objects", wraps=source.capture_objects) as capture:
            self.failed("source_row_state_before_objects")
            capture.assert_not_called()
        self.assertFalse((self.fixture.output / "objects").exists())
        self.assertFalse(any("get-object" in command and command[command.index("--key") + 1].endswith("file.txt")
                             for command in self.fixture.calls if "s3api" in command))

    def test_inventory_objects_are_only_early_evidence_and_captured_index_is_rechecked(self):
        calls, check, capture = [], source.validate_dump_state, source.capture_objects

        def validate(*args):
            calls.append("state")
            return check(*args)

        def capture_changed(*args):
            calls.append("capture")
            objects, index = capture(*args)
            index["uploads", "system/file.txt"]["sha256"] = "f" * 64
            return objects, index

        with patch.object(source, "validate_dump_state", side_effect=validate), patch.object(source, "capture_objects", side_effect=capture_changed):
            self.failed("source_row_state")
        self.assertEqual(calls, ["state", "capture", "state"])
        self.assertEqual(self.fixture.inventory_count, 1)

    def test_real_system_cleanup_sql_is_checked_before_and_after_objects(self):
        for index in range(3):
            add_cleanup(self.fixture.case, index)
        self.fixture.case.refresh()
        with patch.object(source, "validate_dump_state", wraps=source.validate_dump_state) as check:
            result = self.export()
        self.assertEqual(check.call_count, 2)
        self.assertEqual(len(result["enabled_system_schedule_rows"]), 3)
        self.assertFalse(result["clone_verified"])

    def test_changed_schedule_after_objects_is_rejected_before_final_inventory(self):
        schedule, _, _ = add_cleanup(self.fixture.case)
        self.fixture.case.refresh()
        original = source.capture_objects

        def changed(*args):
            result = original(*args)
            schedule["version"] += 1
            self.fixture.case.refresh()
            path = self.fixture.output / "databases/shared-control.sql"
            path.write_bytes((self.fixture.case.root / "shared-control.sql").read_bytes())
            return result

        with patch.object(source, "capture_objects", side_effect=changed):
            self.failed("source_row_state")
        # 复验先重核完整 SQL 文件绑定，已变化的行不会进入最终 inventory。
        self.assertEqual(self.fixture.inventory_count, 1)

    def test_enabled_system_schedule_is_only_a_full_row_fact_not_an_action(self):
        self.fixture.case.add("shared-control", "sys_job_schedule", id=3, tenant_id="system", enabled=1, del_flag="0",
                              handler_key="system.data_retention", version=1)
        self.fixture.case.refresh()
        result = self.export()
        schedule = result["enabled_system_schedule_rows"][0]
        self.assertEqual(schedule["schedule_id"], "3")
        self.assertEqual(len(schedule["source_row_sha256"]), 64)
        self.assertNotIn("action", schedule)
        self.assertNotIn("target", result)

    def test_enabled_other_tenant_and_unclosed_jobs_are_rejected(self):
        row = self.fixture.case.add("shared-control", "sys_job_schedule", id=3, tenant_id="clone-source-01", enabled=1,
                                     del_flag="0", handler_key="system.data_retention", version=1)
        self.fixture.case.refresh()
        self.failed("source_row_state_before_objects")
        row["enabled"] = 0
        self.fixture.case.add("shared-control", "sys_background_job", id=4, tenant_id="system", status="running")
        self.fixture.case.refresh()
        self.fixture.output = self.fixture.local / "unclosed-job"
        self.failed("source_row_state_before_objects")

    def test_dirty_source_generation_or_maintenance_snapshot_mismatch_fails_before_remote_calls(self):
        self.fixture.fingerprint = "sha256:" + "f" * 64
        self.failed("source_generation_before")
        self.assertEqual(self.fixture.calls, [])
        self.fixture.fingerprint = self.fixture.request["worktree_fingerprint"]
        self.fixture.maintenance["source"]["snapshot"] = {**self.fixture.snapshot, "head": "f" * 40}
        self.fixture.output = self.fixture.local / "maintenance-mismatch"
        self.failed("source_generation_before")

    def test_actual_binary_config_and_runtime_binding_changes_fail_closed(self):
        for field in ("binary", "config", "runtime"):
            with self.subTest(field=field):
                self.fixture.output = self.fixture.local / ("changed-" + field)
                if field == "binary":
                    path = Path(self.fixture.build["artifacts"]["api"]["executable"])
                elif field == "config":
                    path = self.fixture.backend / "config/app.toml"
                else:
                    path = self.fixture.runtime_dir / "runtime.json"
                original = path.read_bytes()
                path.write_bytes(original.replace(b"port=18210", b"port=18211") if field == "config" else b"changed")
                self.failed("source_generation_before")
                self.assertEqual(self.fixture.calls, [])
                path.write_bytes(original)

    def test_malformed_current_identity_and_missing_producer_registry_fail_before_resources(self):
        self.fixture.mocks[-2].return_value = {"pid": 100, "started": "other", "executable": "unknown"}
        self.failed("source_generation_before")
        self.fixture.mocks[-2].return_value = None
        self.fixture.registry["processes"] = self.fixture.registry["processes"][:2]
        path = Path(self.fixture.request["producers_registry"]["path"])
        self.fixture.request["producers_registry"] = self.fixture.bind(path, self.fixture.registry)
        self.fixture.save_request()
        self.fixture.output = self.fixture.local / "missing-producer"
        self.failed("source_generation_before")
        self.assertEqual(self.fixture.calls, [])

    def test_newer_pid_generations_preserve_original_source_generation(self):
        item = self.fixture
        identities = {row["identity"]["pid"]: row["identity"] for row in item.registry["processes"]}
        original = proof.verify_generation(item.backend, item.request, item)
        item.mocks[-2].side_effect = lambda pid: {**identities[pid], "started": str(int(identities[pid]["started"]) + 1)}
        self.assertEqual(proof.verify_generation(item.backend, item.request, item), original)

    def test_extra_registered_producer_is_checked_even_when_api_and_worker_are_stopped(self):
        identity = self.fixture.registry["processes"][-1]["identity"]
        self.fixture.mocks[-2].side_effect = lambda pid: identity if pid == identity["pid"] else None
        self.failed("source_generation_before")
        self.assertEqual(self.fixture.calls, [])
        self.assertEqual([call.args[0] for call in self.fixture.mocks[-2].call_args_list], [100, 101, 102])

    def test_registered_empty_target_cannot_be_omitted_from_inventory(self):
        value = copy.deepcopy(self.fixture.request["source"]["databases"][-1])
        value.update(key="shared-empty", mode="shared", database="clone_source_shared_empty")
        self.fixture.request["source"]["databases"].append(value)
        self.fixture.case.data["shared-empty"] = []
        self.fixture.case.add("shared-empty", "biz_tenant_target_slot", slot_id=1)
        self.fixture.case.refresh()
        self.fixture.write_config(self.fixture.request["source"])
        self.fixture.save_request()
        self.fixture.missing_target = True
        self.failed("inventory_before")
        self.assertFalse(any("--no-create-info" in command for command in self.fixture.calls))

    def test_source_changed_after_dump_preserves_partial_files_and_rejects_export(self):
        original = source.dump_databases

        def dump(*args):
            result = original(*args)
            self.fixture.fingerprint = "sha256:" + "c" * 64
            return result

        with patch.object(source, "dump_databases", side_effect=dump):
            self.failed("dump_databases")
        self.assertTrue((self.fixture.output / "databases/shared-control.sql").exists())

    def test_reusing_output_is_rejected_before_any_additional_command(self):
        self.export()
        calls = len(self.fixture.calls)
        with self.assertRaises(ValueError):
            self.export()
        self.assertEqual(len(self.fixture.calls), calls)

    def test_exact_tool_file_change_or_bad_database_overlap_is_rejected(self):
        tool = Path(self.fixture.request["tools"]["aws"]["path"])
        old = tool.read_bytes()
        tool.write_bytes(b"changed")
        self.failed("source_generation_before")
        tool.write_bytes(old)
        first, second = self.fixture.request["source"]["databases"]
        second["database"] = first["database"]
        self.fixture.save_request()
        self.fixture.output = self.fixture.local / "physical-overlap"
        self.failed("source_generation_before")

    def test_old_non_app_password_binding_and_wrong_api_port_are_rejected(self):
        path = self.fixture.backend / "config/app.toml"
        original = path.read_bytes()
        path.write_bytes(original.replace(b"APP_SOURCE_PASSWORD", b"SOURCE_PASSWORD"))
        self.failed("source_generation_before")
        path.write_bytes(original)
        self.fixture.request["source"]["api_url"] = "http://127.0.0.1:18211"
        self.fixture.save_request()
        self.fixture.output = self.fixture.local / "wrong-api-port"
        self.failed("source_generation_before")

    def test_mutated_local_dump_or_partial_cli_failure_never_publishes_success(self):
        original = source.capture_objects

        def capture(*args):
            result = original(*args)
            path = self.fixture.output / "databases/shared-control.sql"
            path.write_bytes(path.read_bytes() + b"unexpected")
            return result

        with patch.object(source, "capture_objects", side_effect=capture):
            self.failed("source_row_state")


class SourcePortProofTests(unittest.TestCase):
    def test_compiled_schema_fingerprints_keep_their_distinct_real_formats(self):
        value = {"control_schema_fingerprint": "9de18acc314ca0bf", "tenant_schema_fingerprint": "d" * 64}
        self.assertEqual(schema_fingerprints(value), value)
        for field, bad in (("control_schema_fingerprint", "c" * 64),
                           ("control_schema_fingerprint", "c" * 15),
                           ("control_schema_fingerprint", "G" * 16),
                           ("tenant_schema_fingerprint", "d" * 16),
                           ("tenant_schema_fingerprint", None)):
            with self.subTest(field=field, bad=bad), self.assertRaises(ValueError):
                schema_fingerprints({**value, field: bad})

    def test_only_explicit_connection_refused_proves_closed_port(self):
        original = proof.require_closed_port
        for code, succeeds in ((errno.ECONNREFUSED, True), (10061, True), (0, False), (errno.ETIMEDOUT, False), (errno.EACCES, False), (10035, False), (10060, False), (10013, False)):
            with self.subTest(code=code):
                connection = Mock()
                connection.__enter__ = Mock(return_value=connection)
                connection.__exit__ = Mock(return_value=False)
                connection.connect_ex.return_value = code
                with patch.object(socket, "socket", return_value=connection), patch.object(proof.os, "name", "posix"):
                    if succeeds:
                        original("http://127.0.0.1:18210")
                    else:
                        with self.assertRaises(ValueError):
                            original("http://127.0.0.1:18210")
                connection.connect_ex.assert_called_once_with(("127.0.0.1", 18210))
                connection.settimeout.assert_called_once_with(5)

    def test_permission_or_observation_exception_cannot_be_treated_as_stopped(self):
        for error in (PermissionError("fixture denied"), TimeoutError("fixture unknown")):
            with self.subTest(error=type(error).__name__):
                connection = Mock()
                connection.__enter__ = Mock(return_value=connection)
                connection.__exit__ = Mock(return_value=False)
                connection.connect_ex.side_effect = error
                with patch.object(socket, "socket", return_value=connection), patch.object(proof.os, "name", "posix"), self.assertRaises(type(error)):
                    proof.require_closed_port("http://127.0.0.1:18210")

    def test_windows_observation_errors_never_fall_back_to_timeout_probe(self):
        with patch.object(proof.os, "name", "nt"), patch.object(proof, "verify_windows_port_idle", side_effect=PermissionError), patch.object(socket, "socket") as probe:
            with self.assertRaises(PermissionError):
                proof.require_closed_port("http://127.0.0.1:18210")
            probe.assert_not_called()


if __name__ == "__main__":
    unittest.main()
