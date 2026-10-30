"""绑定 A 产物与源 HEAD 的离线回归；只读网络操作由固定响应替身承担。"""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_export_verify as verify
import devex_clone_export as export
import devex_clone_capture as capture
import devex_clone_model as model
import devex_clone_rows as sql_rows
from devex_clone_source import export_source
from devex_clone_source_fixture import SourceFixture
from restore_build import file_digest
from restore_reference_io import COPY_METADATA_FIELDS, ExternalTools
from restore_reference_plan import BUCKETS, plan_hash


class ObjectRequestConfigurationTests(unittest.TestCase):
    def test_object_requests_use_only_verified_file_configuration_binding(self):
        from restore_reference_fixture import environment
        backend, plan = environment(self)
        work = backend / ".local-tests"
        configuration = plan["source"]["s3"]["configuration"]
        tools = ExternalTools(plan, work)
        with patch("restore_reference_io.native_s3_read", return_value=({"fixture": "response"}, {})) as native:
            self.assertEqual(tools.read_object("source", "uploads", "source/system/file", work / "object", max_bytes=4096), {"fixture": "response"})
        request = native.call_args.args[0]
        self.assertEqual(set(request), {"format_version", "kind", "scope_id", "configuration_binding", "bucket", "operation", "key", "if_match", "version", "destination", "max_bytes"})
        self.assertEqual(request["configuration_binding"], configuration)
        source = work / "body"
        source.write_bytes(b"data")
        metadata = {name: None for name in COPY_METADATA_FIELDS}
        metadata.update(ContentType="text/plain", Metadata={})
        with patch("restore_reference_io.native_put_if_absent", return_value=({"status": "created_unverified", "remote_write_may_have_occurred": True}, {"returncode": 0, "error_type": None})) as put:
            result = tools.create_object_if_absent("uploads", "target/system/file", source, metadata, file_digest(source))
        request = put.call_args.args[0]
        self.assertEqual(set(request), {"format_version", "kind", "scope_id", "configuration_binding", "bucket", "key", "source", "expected", "metadata"})
        self.assertEqual(request["configuration_binding"], configuration)
        self.assertNotIn("secret-fixture", Path(result["request_file"]).read_text(encoding="utf-8"))


class ExportVerificationTests(unittest.TestCase):
    def setUp(self):
        self.fixture = SourceFixture(self)
        self.fixture.relocate_configuration()
        item = self.fixture
        self.export = export_source(item.backend, item.request_path, item.output, item)
        self.binding = {"path": str(item.output / "export.json"), **file_digest(item.output / "export.json")}
        self.environment = dict(item.environment)
        self.calls, self.effect = [], None
        self.missing = self.extra = self.duplicate = self.cycle = False
        self.paginated = self.wrong_owner = self.head_changed = False
        self.outcome = None
        self.observation = item.local / "source-observation"
        self.tools = ExternalTools({"source": copy.deepcopy(item.request["source"]), "tools": item.request["tools"]}, item.local, self.run_read)

    def load(self):
        return verify.verify_source_export(self.fixture.backend, self.binding)

    def test_raw_target_inventory_is_bound_before_and_after_loading(self):
        loaded = self.load()
        path = self.fixture.output / "inventory-before-target-dedicated-a.json"
        self.assertIn(str(path), loaded["proof_files"])
        path.write_text(path.read_text(encoding="utf-8") + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "证据"):
            self.load()
        with self.assertRaisesRegex(ValueError, "证据"):
            verify.verify_export_bindings(self.fixture.backend, loaded, selection="global")

    def save_export(self):
        path = Path(self.binding["path"])
        path.write_text(json.dumps(self.export), encoding="utf-8")
        self.binding = {"path": str(path), **file_digest(path)}

    def alter_evidence(self, key, change):
        binding = self.export["evidence"][key]
        path = self.fixture.output / binding["file"]
        value = json.loads(path.read_text(encoding="utf-8"))
        change(value)
        path.write_text(json.dumps(value), encoding="utf-8")
        self.export["evidence"][key] = {"file": binding["file"], **file_digest(path)}
        self.save_export()

    def run_read(self, command, **kwargs):
        self.calls.append((command, kwargs))
        self.assertFalse(any(name.startswith("AWS_") for name in kwargs["env"]))
        self.assertFalse(kwargs["check"])
        self.assertIn(command[-1], ("__s3-read", "__s3-inspect"))
        request = json.loads(kwargs["input"])
        self.assertEqual(request["configuration_binding"], self.fixture.object_configuration)
        operation = request["operation"]
        self.assertIn(operation, ("list-objects-v2", "head-object", "get-object"))
        bucket = request["bucket"]
        self.assertIn(bucket, BUCKETS)
        scope = self.fixture.scope + "/"
        if operation == "list-objects-v2":
            self.assertEqual(command[-1], "__s3-inspect")
            self.assertEqual(request["prefix"], scope)
            keys = [scope + ".ryframe-owner"] + ([scope + "system/file.txt"] if bucket == "uploads" and not self.missing else [])
            if self.extra and bucket == "uploads":
                keys.append(scope + "system/unregistered.txt")
            if self.duplicate:
                keys.append(keys[0])
            page = {"IsTruncated": False, "Contents": [{"Key": key} for key in keys]}
            if self.paginated and bucket == "uploads":
                continued = request["continuation"] is not None
                page["Contents"] = [{"Key": keys[-1] if continued else keys[0]}]
                if not continued or self.cycle:
                    page.update(IsTruncated=True, NextContinuationToken="next-page")
            return subprocess.CompletedProcess(command, 0, json.dumps({"ok": True, "result": page}).encode(), b"")
        key = request["key"]
        self.assertTrue(key.startswith(scope))
        if operation == "get-object":
            self.assertTrue(key.endswith("/.ryframe-owner"), "只读复验不能下载任何源业务 payload")
        else:
            self.assertEqual(operation, "head-object")
            self.assertEqual(request["if_match"], '"fixture-etag"')
            self.assertIsNone(request["version"])
            if self.effect:
                self.effect()
            if self.outcome:
                if isinstance(self.outcome, BaseException):
                    raise self.outcome
                return subprocess.CompletedProcess(command, 0 if self.outcome["ok"] else 1,
                                                   json.dumps(self.outcome).encode(), b"private stderr")
        result = self.fixture.native_read(command, kwargs)
        if self.wrong_owner and operation == "get-object":
            Path(request["destination"]).write_bytes(b"wrong-owner")
        if self.head_changed and operation == "head-object":
            value = json.loads(result.stdout)
            value["result"]["Metadata"] = {"changed": "value"}
            result.stdout = json.dumps(value).encode()
        return result

    def observe(self, verified=None, *, single=False):
        verified = self.load() if verified is None else verified
        arguments = (self.fixture.backend, self.tools, verified)
        if single:
            return verify.observe_source_object(*arguments, "uploads", self.fixture.scope + "/system/file.txt", self.observation, environment=self.environment)
        return verify.observe_source_objects(*arguments, self.observation, environment=self.environment)

    def failed_observation(self, verified=None, *, single=False):
        with self.assertRaises(verify.SourceObjectObservationError):
            self.observe(verified, single=single)
        self.assertTrue((self.observation / "failure.json").is_file())
        self.assertFalse((self.observation / "observation.json").exists())

    def test_complete_export_is_verified_without_any_external_calls(self):
        prior = len(self.fixture.calls)
        result = self.load()
        self.assertEqual(len(self.fixture.calls), prior)
        self.assertEqual(result["export"], self.export)
        self.assertFalse(result["export"]["source_snapshot"]["clean"])
        self.assertFalse(result["export"]["dump_semantic_digest_verified"])
        self.assertEqual(set(result["captures"]), BUCKETS)
        self.assertTrue(result["proof_files"])
        self.assertFalse(any(Path(name).suffix == ".sql" or Path(name).name == "object.bin" for name in result["proof_files"]))

    def test_full_export_parses_each_dump_once_and_keeps_narrow_tenant_preread(self):
        with patch.object(export, "rows", wraps=export.rows) as full, \
                patch.object(model, "rows", wraps=model.rows) as narrow:
            self.load()
        self.assertEqual(full.call_count, len(self.export["databases"]))
        self.assertEqual({call.args[0].name for call in full.call_args_list},
                         {Path(dump["artifact"]["file"]).name for dump in self.export["databases"]})
        self.assertTrue(all(not call.kwargs for call in full.call_args_list))
        self.assertEqual(narrow.call_count, 1)
        self.assertEqual(narrow.call_args.kwargs, {"only_table": "sys_tenant"})
        self.assertEqual(set(narrow.call_args.args[1]), set(self.fixture.case.control) | set(self.fixture.case.tenant))

    def test_full_export_keeps_real_object_relationship_validation_after_counting(self):
        capture = verify.verify_captures
        def wrong_relation(*args):
            captures, index, proofs = capture(*args)
            index = copy.deepcopy(index)
            index["uploads", "system/file.txt"]["sha256"] = "f" * 64
            return captures, index, proofs
        with patch.object(verify, "verify_captures", side_effect=wrong_relation), \
                patch.object(export, "validate_relations", wraps=export.validate_relations) as relations, \
                self.assertRaisesRegex(ValueError, "有效文件"):
            self.load()
        relations.assert_called_once()

    def test_dedicated_dump_change_after_semantic_scan_is_rejected(self):
        original = export.rows
        changed = []
        def change_after(path, catalog):
            yield from original(path, catalog)
            if path.name == "dedicated-a.sql":
                path.write_bytes(path.read_bytes() + b"-- modified after scanning\n")
                changed.append(path.name)
        with patch.object(export, "rows", side_effect=change_after), self.assertRaisesRegex(ValueError, "证据已变化"):
            self.load()
        self.assertEqual(changed, ["dedicated-a.sql"])

    def test_dedicated_dump_change_between_declaration_and_state_scan_is_rejected(self):
        original = verify.verify_captures
        def change_before(*args):
            result = original(*args)
            path = self.fixture.output / "databases/dedicated-a.sql"
            self.assertTrue(path.is_file())
            path.write_bytes(b"-- changed before state scan\n")
            return result
        with patch.object(verify, "verify_captures", side_effect=change_before), \
                self.assertRaisesRegex(ValueError, "证据已变化"):
            self.load()

    def test_unrelated_new_target_proof_does_not_mutate_bound_source(self):
        loaded = self.load()
        (self.fixture.output / "target-proof.json").write_text('{"purpose":"independent"}', encoding="utf-8")
        verify.verify_export_bindings(self.fixture.backend, loaded)
        self.assertEqual(self.load()["export"], loaded["export"])

    def test_external_binding_is_required_even_when_json_remains_valid(self):
        path = Path(self.binding["path"])
        path.write_text(path.read_text(encoding="utf-8") + "\n", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.load()

    def test_failure_marker_rejects_residual_export(self):
        (self.fixture.output / "failure.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.load()

    def test_missing_original_evidence_cannot_be_replaced_with_empty_map(self):
        self.export["evidence"] = {}
        self.save_export()
        with self.assertRaises(ValueError):
            self.load()

    def test_generation_disagreement_rejects_even_when_new_file_hash_is_bound(self):
        self.alter_evidence("generation-after", lambda value: value["source"].update(head="f" * 40))
        with self.assertRaises(ValueError):
            self.load()

    def test_inventory_disagreement_rejects(self):
        self.alter_evidence("inventory-after", lambda value: value["databases"][0]["tables"][0].update(sha256="e" * 64))
        with self.assertRaises(ValueError):
            self.load()

    def test_unknown_schema_table_rejects_even_with_updated_schema_hash(self):
        def alter(value):
            entry = value["databases"][0]
            entry["columns"]["unknown_table"] = {"id": "BIGINT"}
            entry["sha256"] = plan_hash(entry["columns"])
        self.alter_evidence("schema-before", alter)
        with self.assertRaises(ValueError):
            self.load()

    def test_missing_empty_dump_table_rejects(self):
        next(item for item in self.export["databases"] if item["key"] == "shared-control")["tables"].pop("sys_background_job")
        self.save_export()
        with self.assertRaises(ValueError):
            self.load()

    def test_changed_sql_payload_is_rejected(self):
        path = self.fixture.output / self.export["databases"][0]["artifact"]["file"]
        path.write_text(path.read_text(encoding="utf-8") + "-- changed\n", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.load()

    def test_changed_object_payload_and_original_head_are_rejected(self):
        entry = next(bucket for bucket in self.export["objects"] if bucket["bucket"] == "uploads")["entries"][0]
        body = self.fixture.output / entry["artifact"]["file"]
        body.write_bytes(b"changed")
        with self.assertRaises(ValueError):
            self.load()
        body.write_bytes(b"data")
        head = body.parent / "head-before.json"
        head.write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.load()

    def test_lightweight_binding_check_still_rejects_original_diagnostic_changes(self):
        loaded = self.load()
        entry = next(bucket for bucket in self.export["objects"] if bucket["bucket"] == "uploads")["entries"][0]
        (self.fixture.output / entry["capture"]["file"]).parent.joinpath("get.diagnostic.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            verify.verify_export_bindings(self.fixture.backend, loaded)

    def test_lightweight_cached_evidence_map_cannot_be_emptied(self):
        loaded = self.load()
        loaded["proof_files"] = {}
        with self.assertRaises(ValueError):
            verify.verify_export_bindings(self.fixture.backend, loaded)

    def test_linked_original_evidence_and_dangling_failure_marker_are_rejected(self):
        loaded = self.load()
        original = Path.is_symlink
        for target in (self.fixture.output / "generation-before.json", self.fixture.output / "failure.json"):
            with self.subTest(target=target.name), patch.object(Path, "is_symlink", lambda path: path == target or original(path)):
                with self.assertRaises(ValueError):
                    verify.verify_export_bindings(self.fixture.backend, loaded)

    def test_original_small_proof_change_during_head_is_detected(self):
        loaded = self.load()
        self.effect = lambda: (self.fixture.output / "schema-before.json").write_text("{}", encoding="utf-8")
        self.failed_observation(loaded, single=True)

    def test_missing_complete_bucket_and_wrong_capture_metadata_are_rejected(self):
        original = copy.deepcopy(self.export)
        self.export["objects"].pop()
        self.save_export()
        with self.assertRaises(ValueError):
            self.load()
        self.export = original
        next(bucket for bucket in self.export["objects"] if bucket["bucket"] == "uploads")["entries"][0]["metadata"]["ContentType"] = "application/json"
        self.save_export()
        with self.assertRaises(ValueError):
            self.load()

    def test_catalog_change_is_rejected_before_observation(self):
        loaded = self.load()
        original = verify.schema_catalog
        def changed(path):
            result = list(original(path))
            result[0] = {**result[0], "unknown_table": {"id": "BIGINT"}}
            return tuple(result)
        with patch.object(verify, "schema_catalog", side_effect=changed):
            with self.assertRaises(ValueError):
                verify.verify_export_bindings(self.fixture.backend, loaded)

    def test_binding_uses_only_required_catalog_models(self):
        loaded = self.load()
        for selection in ("all", "global", ("uploads", self.fixture.scope + "/system/file.txt")):
            with self.subTest(selection=selection), patch.object(verify, "schema_models", side_effect=AssertionError("unused full models")), \
                    patch.object(sql_rows, "_native", wraps=sql_rows._native) as catalog, \
                    patch.object(export, "rows", side_effect=AssertionError("binding must not read SQL")), \
                    patch.object(model, "rows", side_effect=AssertionError("binding must not preread tenants")):
                verify.verify_export_bindings(self.fixture.backend, loaded, selection=selection)
                catalog.assert_called_once_with("copy-schema-catalog", {
                    "backend": str(self.fixture.backend), "include_full": False})

    def test_binding_rejects_actual_control_ddl_column_type_drift(self):
        loaded = self.load()
        filename = self.fixture.backend / "sql/ryframe_config.sql"
        content = filename.read_text(encoding="utf-8")
        start = content.index("CREATE TABLE IF NOT EXISTS `sys_user`")
        end = content.index("ENGINE=", start)
        table = content[start:end]
        self.assertIn("BIGINT", table)
        changed = table.replace("BIGINT", "INT", 1)
        filename.write_text(content[:start] + changed + content[end:], encoding="utf-8")
        with self.assertRaises(ValueError):
            verify.verify_export_bindings(self.fixture.backend, loaded)

    def test_full_model_callers_retain_complete_models(self):
        with patch.object(sql_rows, "_native", side_effect=AssertionError("unused filtered models")), \
                patch.object(export, "_native", wraps=export._native) as full:
            models = export.schema_models(self.fixture.backend)
        full.assert_called_once_with("copy-schema-catalog", {
            "backend": str(self.fixture.backend), "include_full": True})
        self.assertEqual(len(models), 4)
        self.assertEqual(models[:2], verify.schema_catalog(self.fixture.backend))
        self.assertIn("ryframe_resource_ownership", models[2])
        self.assertIn("ryframe_resource_ownership", models[3])
        with patch.object(verify, "schema_models", wraps=verify.schema_models) as complete:
            self.load()
        self.assertEqual(complete.call_count, 1)

    def test_saved_build_source_must_match_recorded_generation(self):
        def changed(value):
            value["runtime"]["scope_id"] = "unexpected-scope"
        self.alter_evidence("generation-before", changed)
        self.alter_evidence("generation-after", changed)
        with self.assertRaises(ValueError):
            self.load()

    def test_whole_listing_and_heads_use_no_business_get_or_ambient_credentials(self):
        self.assertFalse((self.fixture.backend / "config").exists())
        with patch.dict(os.environ, {"TEST_SECRET": "ambient-wrong", "AWS_SECRET_ACCESS_KEY": "implicit-wrong"}):
            result = self.observe()
        self.assertTrue(result["whole_source"])
        self.assertEqual(result["business_get_requests"], 0)
        self.assertEqual(result["business_bytes_downloaded"], 0)
        self.assertFalse(result["source_body_sha_recomputed"])
        operations = [json.loads(options["input"])["operation"] for _, options in self.calls]
        self.assertEqual(operations.count("get-object"), 10)
        self.assertEqual(operations.count("head-object"), 1)
        self.assertEqual(operations.count("list-objects-v2"), 10)

    def test_original_runtime_and_both_registered_configurations_stay_bound_during_head(self):
        loaded = self.load()
        paths = [self.fixture.runtime_dir / "runtime.json",
                 Path(self.fixture.runtime["api_config_dir"]) / "app.test.toml",
                 Path(self.fixture.runtime["worker_config_dir"]) / "app.test.toml"]
        for index, path in enumerate(paths):
            with self.subTest(path=path.name, role=index):
                self.observation = self.fixture.local / f"runtime-drift-{index}"
                self.calls = []
                original = path.read_bytes()
                self.effect = lambda: path.write_bytes(original + b"\n")
                try:
                    self.failed_observation(loaded, single=True)
                    operations = [json.loads(options["input"])["operation"] for _, options in self.calls]
                    self.assertEqual(operations.count("head-object"), 1)
                    self.assertEqual(operations.count("list-objects-v2"), 0)
                finally:
                    path.write_bytes(original)
        self.effect = None
        loaded["request"]["runtime"]["sha256"] = "0" * 64
        self.observation = self.fixture.local / "forged-runtime-descriptor"
        self.calls = []
        self.failed_observation(loaded, single=True)
        self.assertEqual(self.calls, [])

    def test_single_head_only_does_not_download_or_list_whole_dataset(self):
        result = self.observe(single=True)
        self.assertFalse(result["whole_source"])
        operations = [json.loads(options["input"])["operation"] for _, options in self.calls]
        self.assertEqual(operations.count("head-object"), 1)
        self.assertNotIn("list-objects-v2", operations)
        self.assertEqual(operations.count("get-object"), 2)
        owners = [json.loads(options["input"]) for _, options in self.calls
                  if json.loads(options["input"]).get("key", "").endswith("/.ryframe-owner")]
        self.assertEqual([request["bucket"] for request in owners], ["uploads", "uploads"])

    def test_list_diagnostics_open_with_extended_path_conversion(self):
        recorded = []
        original = capture.filesystem_path

        def wrap(path):
            recorded.append(Path(path))
            return original(path)

        with patch.object(capture, "filesystem_path", wrap):
            self.observe()
        expected = {f"list-{phase}-{bucket}-0.diagnostic.json"
                    for phase in ("before", "after") for bucket in BUCKETS}
        self.assertTrue(expected.issubset({path.name for path in recorded}))

    def test_real_pagination_is_completed_without_truncating_key_set(self):
        self.paginated = True
        self.observe()
        self.assertEqual(sum(json.loads(options["input"]).get("continuation") is not None
                             for _, options in self.calls), 2)

    def test_missing_extra_duplicate_and_looping_source_lists_fail(self):
        loaded = self.load()
        for index, flag in enumerate(("missing", "extra", "duplicate", "cycle")):
            with self.subTest(flag=flag):
                self.observation = self.fixture.local / f"list-failure-{index}"
                self.paginated = flag == "cycle"
                setattr(self, flag, True)
                self.failed_observation(loaded)
                setattr(self, flag, False)

    def test_wrong_owner_fails_before_any_business_head(self):
        self.wrong_owner = True
        self.failed_observation()
        self.assertFalse(any(json.loads(options["input"])["operation"] == "head-object"
                             for _, options in self.calls))

    def test_changed_head_metadata_is_never_adopted(self):
        self.head_changed = True
        self.failed_observation(single=True)

    def test_head_404_403_and_timeout_fail_without_retry_and_hide_credentials(self):
        loaded = self.load()
        for index, code in enumerate(("404", "403", "timeout")):
            with self.subTest(code=code):
                self.observation = self.fixture.local / f"head-failure-{index}"
                self.calls = []
                if code == "timeout":
                    self.outcome = subprocess.TimeoutExpired(["xtask", "__s3-read"], 1, stderr=b"secret-fixture")
                elif code == "404":
                    self.outcome = {"ok": True, "result": {"status": "not_found", "http_status": 404,
                                    "remote_write_may_have_occurred": False}}
                else:
                    self.outcome = {"ok": False, "error": {"category": "NativeS3ReadRejected",
                                    "message": "403 access-fixture secret-fixture",
                                    "remote_write_may_have_occurred": False,
                                    "automatic_retry_permitted": False}}
                self.failed_observation(loaded, single=True)
                self.assertEqual(sum(json.loads(options["input"])["operation"] == "head-object"
                                     for _, options in self.calls), 1)
                text = "\n".join(path.read_text(encoding="utf-8") for path in self.observation.glob("*.json"))
                self.assertNotIn("secret-fixture", text)
                self.assertNotIn("access-fixture", text)

    def test_environment_and_plan_change_stop_before_next_request(self):
        loaded = self.load()
        self.effect = lambda: self.environment.update(APP_ENV="changed")
        self.failed_observation(loaded, single=True)
        self.assertEqual(sum(json.loads(options["input"])["operation"] == "head-object"
                             for _, options in self.calls), 1)
        self.environment["APP_ENV"] = "test"
        self.observation = self.fixture.local / "changed-plan"
        self.calls = []
        self.effect = lambda: self.tools.plan["source"]["s3"].update(region="changed-region")
        self.failed_observation(loaded, single=True)
        self.assertEqual(sum(json.loads(options["input"])["operation"] == "head-object"
                             for _, options in self.calls), 1)

    def test_unknown_single_key_rejected_before_remote_calls_or_new_directory(self):
        loaded = self.load()
        with self.assertRaises(ValueError):
            verify.observe_source_object(self.fixture.backend, self.tools, loaded, "uploads", "other/system/file.txt", self.observation, environment=self.environment)
        self.assertFalse(self.observation.exists())
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
