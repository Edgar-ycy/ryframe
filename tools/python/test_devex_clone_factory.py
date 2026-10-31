"""实际组合入口与真实本地账本的离线回归；远端库存和协议由明确替身提供。"""
import copy
from dataclasses import asdict, replace
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_factory as factory
import test_devex_clone_transfer as fixtures
from devex_clone_ledger import CloneLedger
from devex_clone_capture import capture_object
from devex_clone_transfer import ObjectObservation
from restore_build import file_digest
from restore_reference_plan import plan_hash
from native_s3_client import configuration as object_configuration


class FactoryTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.TransferTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        defaults = Path(self.case.tools.plan["source"]["databases"][0]["defaults_file"])
        defaults.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=file-only-auth\nssl-mode=DISABLED\n", encoding="utf-8")
        for side in ("source", "target"):
            for database in self.case.tools.plan[side]["databases"]:
                database["defaults_sha256"] = file_digest(defaults)["sha256"]
        self.backend, self.local = self.case.backend, self.case.backend / ".local-tests"
        self.output = self.local / "live-copy"
        self.target = self.local / "fresh-target"
        self.target.mkdir()
        self.schema = {"control_schema_fingerprint": "c" * 16, "tenant_schema_fingerprint": "d" * 64}
        for key, basis in list(self.case.guard.basis.items()):
            schema = {**self.schema, **({"control_schema_fingerprint": None} if key != "shared-control" else {})}
            source = replace(basis.source, schema_sha256=plan_hash(schema))
            initial = replace(basis.initialized, schema_sha256=plan_hash(schema))
            self.case.guard.basis[key] = replace(basis, source=source, initialized=initial)
            self.case.guard.current[key] = initial
        value = self.case.fixture.value
        self.exported = {key: copy.deepcopy(value[key]) for key in ("artifact_root", "databases", "objects")}
        self.exported.update(source={**self.case.tools.plan["source"], "s3": {key: object_configuration(self.case.tools.plan["source"]["s3"]["configuration"], self.backend)[key] for key in ("endpoint", "region")}}, source_snapshot={"head": "a" * 40, "clean": False, "patch_sha256": "f" * 64, "files": []},
                             worktree_fingerprint="sha256:" + "b" * 64, enabled_system_schedule_rows=[],
                             evidence={"generation-after": value["evidence"]["source_stopped"]})
        self.export_binding = self.bind(self.local / "source-export.json", {"fixture": "verified by isolated source adapter"})
        maintenance = self.bind(self.local / "maintenance.json", {
            "fixture": "maintenance", "backend_root": str(self.backend)})
        self.source_request = {"maintenance_build": maintenance, "source": self.case.tools.plan["source"],
                               "tools": {name: copy.deepcopy(self.case.tools.plan["tools"]["mysql"])
                                         for name in ("mysql", "mysqldump", "node")}}
        self.target_request = {"maintenance_build": maintenance, "tools": {"mysql": self.case.tools.plan["tools"]["mysql"]}, "target": {**self.case.tools.plan["target"], "s3": object_configuration(self.case.tools.plan["target"]["s3"]["configuration"], self.backend)}, "config_dir": self.case.tools.plan["target"]["s3"]["configuration"]["directory"]}
        self.initial = {"redis": {"fixture": "owner only"}, "generation": {"selected": {"runtime_dir": str(self.local / "never-started"), "api_url": "http://127.0.0.1:18400"}},
                        "inventory": {"observations": {basis.initialized.resource["database"]: json.loads(json.dumps(asdict(basis.initialized)))
                                                        for basis in self.case.guard.basis.values()}}}
        self.initialized = self.bind(self.target / "initialized.json", self.initial)
        inventory = {**self.schema, "databases": [{"key": key, "kind": "combined" if key == "shared-control" else "tenant",
                     "tables": [{"table": name, **data} for name, data in basis.source.tables.items()]}
                    for key, basis in self.case.guard.basis.items()]}
        self.verified = {"export": self.exported, "generation": {"fixture": "stopped"}, "request": self.source_request, "inventory": inventory}
        self.source_changed = False
        self.prepared_target_changed = False
        self.extra_object = False
        self.mutate_final_object = None
        self.source_object_passes = 0
        self.environments = {"source": dict(os.environ, CLONE_SIDE="source"), "target": dict(os.environ, CLONE_SIDE="target")}
        temporary = self.local / "target-private-tmp"
        temporary.mkdir()
        self.environments["target"].update(TEMP=str(temporary), TMP=str(temporary))
        self.patches = [patch.object(factory, "verify_source_export", return_value=self.verified),
                        patch.object(factory, "verify_export_bindings"),
                        patch.object(factory, "verify_generation", side_effect=self.generation),
                        patch.object(factory, "target_history", side_effect=self.history),
                        patch.object(factory, "target_binding", side_effect=self.preflight),
                        patch.object(factory, "capture_side_inventory", side_effect=self.inventory),
                        patch.object(factory, "observe_source_objects", side_effect=self.source_objects),
                        patch.object(factory, "observe_source_object", side_effect=self.source_object),
                        patch.object(factory, "observe_target_object", side_effect=self.target_object),
                        patch.object(factory, "CloneLedger", side_effect=self.ledger),
                        patch.object(factory, "initialization_history", side_effect=lambda *_: (self.initial, self.target_request))]
        self.mocks = [patcher.start() for patcher in self.patches]
        self.addCleanup(lambda: [patcher.stop() for patcher in reversed(self.patches)])

    def bind(self, path, value):
        path.write_text(json.dumps(value), encoding="utf-8")
        return {"path": str(path), **file_digest(path)}

    def generation(self, *_):
        self.assertEqual(os.environ["CLONE_SIDE"], "source")
        return {"fixture": "changed" if self.source_changed else "stopped"}

    def history(self, backend, binding, output, run, *, owned_lock_identity, storage_run=None):
        self.assertEqual(os.environ["CLONE_SIDE"], "target")
        lock = self.target / "initialize.lock"
        self.assertEqual(owned_lock_identity, lock.stat().st_ino if lock.exists() else None)
        resources = SimpleNamespace(tools=self.case.tools, object_keys=self.object_keys,
                                    redis_state=lambda **_: self.initial["redis"])
        return copy.deepcopy(self.initial), copy.deepcopy(self.target_request), resources

    def object_keys(self, bucket):
        keys = {"clone-target/.ryframe-owner"} | {key for stored_bucket, key in self.case.runner.objects if stored_bucket == bucket}
        if self.extra_object:
            keys.add("clone-target/system/unknown")
        return keys

    def preflight(self, operation, backend, **arguments):
        self.assertEqual(os.environ["CLONE_SIDE"], "target")
        self.assertEqual(operation, "fresh-verify-target")
        self.assertEqual(backend, self.backend)
        self.assertEqual(set(arguments), {"initialized", "observation", "storage_run", "temporary_directory", "app_env", "write"})
        self.assertEqual(arguments["temporary_directory"], self.environments["target"]["TEMP"])
        self.assertEqual(arguments["initialized"], self.initialized)
        self.assertEqual(arguments["app_env"], "test")
        self.assertIs(arguments["write"], True)
        if self.prepared_target_changed:
            raise ValueError("actual initial inventory changed")
        return {"initialized": self.initialized}

    def inventory(self, backend, side, tools, receipt, output, *, environment, evidence_root):
        self.assertEqual(os.environ["CLONE_SIDE"], side)
        self.assertEqual(environment["CLONE_SIDE"], side)
        values = [basis.source for basis in self.case.guard.basis.values()] if side == "source" else list(self.case.guard.current.values())
        return SimpleNamespace(observations=tuple(copy.deepcopy(values)))

    def source_objects(self, *_, environment):
        self.assertEqual(environment["CLONE_SIDE"], "source")
        self.assertEqual(os.environ["CLONE_SIDE"], "source")
        self.source_object_passes += 1
        if self.source_object_passes == 2 and self.mutate_final_object:
            key = ("uploads", "clone-target/system/file.txt")
            body, metadata = self.case.runner.objects[key]
            self.case.runner.objects[key] = ((b"changed", metadata) if self.mutate_final_object == "body"
                                             else (body, {**metadata, "ContentLanguage": "en"}))
        return {"fixture": "all source objects observed"}

    def source_object(self, backend, tools, verified, bucket, key, output, *, environment):
        self.assertEqual(environment["CLONE_SIDE"], "source")
        self.assertTrue(key.startswith("clone-source/"))

    def target_object(self, backend, tools, bucket, key, output, **_):
        self.assertEqual(os.environ["CLONE_SIDE"], "target")
        resource = {"kind": "object", "scope_id": tools.plan["target"]["scope_id"],
                    "endpoint": object_configuration(tools.plan["target"]["s3"]["configuration"], backend)["endpoint"], "bucket": bucket, "key": key}
        if (bucket, key) not in self.case.runner.objects:
            return ObjectObservation(resource, None, "d" * 64)
        capture_object(tools, "target", bucket, key, output, expected=_["expected"], max_bytes=_["max_bytes"], owner_buckets=(bucket,))
        return ObjectObservation(resource, output, None)

    def ledger(self, *args, **kwargs):
        self.case.ledger = CloneLedger(*args, **kwargs)
        return self.case.ledger

    def execute(self, *, source_storage_run=None, target_storage_run=None):
        return factory.copy_to_fresh_target(self.backend, self.export_binding, self.initialized,
                    self.environments["source"], self.environments["target"], self.output,
                    copy_id="factory-case", stage="source_to_seed", run=self.case.runner,
                    source_storage_run=source_storage_run, target_storage_run=target_storage_run)

    def test_complete_transfer_uses_source_and_target_environments_and_retains_pending_state(self):
        original = dict(os.environ)
        result = self.execute()
        self.assertEqual(result["status"], "data_steps_verified")
        self.assertEqual(result["written_steps"], 3)
        self.assertTrue(any("__s3-put-if-absent" in args for args, _ in self.case.writes()))
        self.assertFalse(result["target_ready"])
        self.assertFalse(result["restore_success"])
        self.assertEqual(self.mocks[6].call_count, 2)
        self.assertEqual(dict(os.environ), original)
        self.assertFalse((self.target / "initialize.lock").exists())
        self.assertEqual(json.loads((self.output / "result.json").read_text(encoding="utf-8")), result)

    def test_initial_and_final_full_proofs_surround_per_object_generation_selection(self):
        self.execute()
        selected = [call.kwargs["selection"] for call in self.mocks[1].call_args_list]
        item = self.case.plan["objects"][0]
        source = (item["bucket"], item["source_key"])
        self.assertEqual(selected[0], "all")
        self.assertEqual(selected[-2:], ["all", "global"])
        self.assertIn("global", selected)
        self.assertEqual(selected.count(source), 7)
        self.assertGreaterEqual(selected.count("all"), 3)

    def test_current_object_global_adapter_failure_prevents_its_put(self):
        item = self.case.plan["objects"][0]
        selected = (item["bucket"], item["source_key"])
        def reject(_backend, _verified, *, selection):
            if selection == selected:
                raise ValueError("当前对象原始证明损坏")
        self.mocks[1].side_effect = reject
        with self.assertRaisesRegex(ValueError, "对象原始证明"):
            self.execute()
        self.assertEqual(len(self.case.writes()), len(self.exported["databases"]))
        self.assertFalse(any("__s3-put-if-absent" in args for args, _ in self.case.writes()))

    def test_registered_source_storage_failure_prevents_copy_writes(self):
        directory = self.local / "run"
        with patch("devex_clone_storage.current_storage_binding", side_effect=ValueError("source storage stopped")) as storage:
            with self.assertRaisesRegex(ValueError, "storage stopped"):
                self.execute(source_storage_run=directory)
        storage.assert_called_once_with(self.backend, directory, "source")
        self.mocks[1].assert_not_called()
        self.assertEqual(self.case.writes(), [])
        self.assertFalse((self.output / "result.json").exists())

    def test_source_storage_stop_after_commit_leaves_unknown_write_for_reconciliation(self):
        directory = self.local / "run"
        stopped = False
        def stop(_):
            nonlocal stopped
            stopped = True
        def current(backend, run_directory, side):
            self.assertEqual((backend, run_directory, side), (self.backend, directory, "source"))
            if stopped:
                raise ValueError("source storage latest stop")
            return {"fixture": "registered running storage"}
        self.case.runner.after_commit = stop
        with patch("devex_clone_storage.current_storage_binding", side_effect=current):
            with self.assertRaisesRegex(ValueError, "latest stop"):
                self.execute(source_storage_run=directory)
        self.assertEqual(len(self.case.writes()), 1)
        self.assertEqual(self.case.ledger.inspect()["status"], "needs_reconciliation")
        self.assertFalse((self.output / "result.json").exists())
        self.assertTrue((self.output / "failure.json").exists())

    def test_source_without_storage_registration_preserves_history_and_copy_contract(self):
        directory = self.local / "run"
        paths = [Path(item["path"]) for item in (self.export_binding, self.initialized)]
        original = {path: path.read_bytes() for path in paths}
        with patch("devex_clone_storage.current_storage_binding", return_value=None) as storage:
            result = self.execute(source_storage_run=directory)
        self.assertEqual(result["status"], "data_steps_verified")
        self.assertEqual(result["written_steps"], 3)
        self.assertGreater(storage.call_count, 2)
        self.assertTrue(all(item.args == (self.backend, directory, "source") for item in storage.call_args_list))
        self.assertEqual({path: path.read_bytes() for path in paths}, original)

    def test_changed_source_generation_after_commit_keeps_unknown_ledger_and_stops(self):
        self.case.runner.after_commit = lambda _: setattr(self, "source_changed", True)
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(len(self.case.writes()), 1)
        self.assertFalse((self.output / "result.json").exists())
        self.assertEqual(self.case.ledger.inspect()["status"], "needs_reconciliation")
        self.assertTrue((self.output / "failure.json").exists())

    def test_changed_target_initial_snapshot_never_writes(self):
        self.prepared_target_changed = True
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.case.writes(), [])
        self.assertTrue((self.output / "failure.json").exists())

    def test_missing_target_temporary_directory_never_invokes_native_verification_or_copy_writes(self):
        self.environments["target"].pop("TEMP")
        with patch.object(factory, "target_binding") as verify:
            with self.assertRaisesRegex(ValueError, "TEMP/TMP"):
                self.execute()
            verify.assert_not_called()
        self.assertEqual(self.case.writes(), [])

    def test_different_mysql_tool_rejected_before_any_write(self):
        self.target_request["tools"] = copy.deepcopy(self.target_request["tools"])
        self.target_request["tools"]["mysql"]["sha256"] = "f" * 64
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.case.writes(), [])

    def test_unexpected_source_row_digest_prevents_copy(self):
        self.verified["inventory"]["databases"][0]["tables"][0]["sha256"] = "f" * 64
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.case.writes(), [])

    def test_failed_initial_source_validation_has_failure_evidence(self):
        secret = "private-copy-password-must-not-be-recorded"
        for label, errno, winerror in (("unsafe-attributes", secret, True), ("integer-attributes", 13, 5)):
            with self.subTest(label=label):
                self.output = self.local / ("prepare-failure-" + label)
                error = ValueError(secret)
                error.errno, error.winerror = errno, winerror
                def reject_export(*_args):
                    raise error
                self.mocks[0].side_effect = reject_export
                with self.assertRaises(ValueError) as failed:
                    self.execute()
                self.assertIs(failed.exception, error)
                raw = (self.output / "failure.json").read_text(encoding="utf-8")
                failure = json.loads(raw)
                self.assertEqual(set(failure), {"format_version", "kind", "phase", "status", "error_type", "frames",
                                               "errno", "winerror", "automatic_retry", "automatic_resource_cleanup",
                                               "target_ready", "restore_success"})
                self.assertEqual((failure["format_version"], failure["kind"], failure["phase"]),
                                 (1, "devex-copy-failure", "prepare"))
                self.assertEqual((failure["status"], failure["error_type"]), ("needs_reconciliation", "ValueError"))
                self.assertEqual((failure["errno"], failure["winerror"]),
                                 (13, 5) if label == "integer-attributes" else (None, None))
                functions = [frame["function"] for frame in failure["frames"]]
                self.assertIn("copy_to_fresh_target", functions)
                self.assertIn("prepare", functions)
                self.assertEqual(failure["frames"][-1], {"file": Path(__file__).name, "function": "reject_export",
                                                       "line": reject_export.__code__.co_firstlineno + 1})
                for frame in failure["frames"]:
                    self.assertEqual(set(frame), {"file", "function", "line"})
                    self.assertFalse(Path(frame["file"]).is_absolute())
                    self.assertIs(type(frame["line"]), int)
                    self.assertGreater(frame["line"], 0)
                for field in ("automatic_retry", "automatic_resource_cleanup", "target_ready", "restore_success"):
                    self.assertIs(failure[field], False)
                self.assertNotIn(secret, raw)
                for name in ("plan.json", "session.json", "ledger", "result.json"):
                    self.assertFalse((self.output / name).exists())
        self.assertEqual(self.case.writes(), [])

    def test_prepare_failure_record_error_does_not_replace_original_exception(self):
        error = ValueError("private-copy-password-must-not-be-recorded")
        self.mocks[0].side_effect = error
        with patch.object(factory, "write_json", side_effect=OSError("private-diagnostic-password")), \
                self.assertRaises(ValueError) as failed:
            self.execute()
        self.assertIs(failed.exception, error)
        self.assertEqual(error.__notes__, ["复制失败位置证据保存失败：OSError"])
        self.assertFalse((self.output / "result.json").exists())
        self.assertEqual(self.case.writes(), [])

    def test_command_failure_saves_redacted_diagnostic_without_sql_stdin(self):
        self.case.runner.failure = "commit-response-lost"
        with self.assertRaises(subprocess.TimeoutExpired):
            self.execute()
        logs = [json.loads(path.read_text(encoding="utf-8")) for path in self.output.glob("command-*.json")]
        self.assertTrue(any(log["error_type"] == "TimeoutExpired" for log in logs))
        self.assertTrue(all("input" not in log for log in logs))
        self.assertFalse((self.output / "result.json").exists())

    def test_extra_target_object_blocks_completion_without_deleting_it(self):
        self.extra_object = True
        with self.assertRaisesRegex(ValueError, "完整对象范围"):
            self.execute()
        self.assertEqual(len(self.case.writes()), 3)
        self.assertFalse((self.output / "result.json").exists())
        self.assertTrue(self.extra_object)

    def test_failure_log_redacts_actual_credentials_and_omits_sql_body(self):
        def denied(command, **kwargs):
            raise subprocess.CalledProcessError(254, command, stderr=b"fixture-secret fixture-access file-only-auth")

        session = factory.CloneSession(self.backend, self.export_binding, self.initialized,
            self.environments["source"], self.environments["target"], self.output, denied)
        session.exported, session.target_request, session.source_request = self.exported, self.target_request, self.source_request
        session.bind_credentials()
        with session.environment.use("target"), self.assertRaises(subprocess.CalledProcessError):
            session.command(["fixture"], input=b"private SQL body")
        raw = next(self.output.glob("command-*.json")).read_text(encoding="utf-8")
        self.assertNotIn("fixture-secret", raw)
        self.assertNotIn("fixture-access", raw)
        self.assertNotIn("file-only-auth", raw)
        self.assertNotIn("private SQL body", raw)
        self.assertIn("[REDACTED]", raw)

    def test_same_key_target_body_replacement_during_final_source_check_blocks_completion(self):
        self.mutate_final_object = "body"
        with self.assertRaisesRegex(ValueError, "同 key"):
            self.execute()
        self.assertFalse((self.output / "result.json").exists())
        self.assertEqual(self.case.runner.objects["uploads", "clone-target/system/file.txt"][0], b"changed")

    def test_same_key_target_metadata_replacement_during_final_source_check_blocks_completion(self):
        self.mutate_final_object = "metadata"
        with self.assertRaisesRegex(ValueError, "同 key"):
            self.execute()
        self.assertFalse((self.output / "result.json").exists())

    def test_process_command_probe_never_records_unknown_raw_output(self):
        def unknown_process(command, **kwargs):
            raise subprocess.CalledProcessError(7, command, output=b'"unexpected-process --private=unknown-credential"',
                                                stderr=b"unknown-process-private-diagnostic")

        session = factory.CloneSession(self.backend, self.export_binding, self.initialized,
            self.environments["source"], self.environments["target"], self.output, unknown_process)
        session.exported, session.target_request, session.source_request = self.exported, self.target_request, self.source_request
        session.bind_credentials()
        with self.assertRaises(subprocess.CalledProcessError):
            session.command(["D:/WindowsPowerShell/powershell.exe", "-Command", "Get-CimInstance Win32_Process"])
        raw = next(self.output.glob("command-*.json")).read_text(encoding="utf-8")
        self.assertNotIn("unknown-credential", raw)
        self.assertNotIn("unknown-process-private-diagnostic", raw)
        record = json.loads(raw)
        self.assertEqual(record["output_policy"], "digest_only")
        self.assertEqual(record["returncode"], 7)
        self.assertEqual(record["error_type"], "CalledProcessError")

    def test_existing_output_cannot_be_reused_or_changed(self):
        self.output.mkdir()
        marker = self.output / "original.txt"
        marker.write_text("keep", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(marker.read_text(encoding="utf-8"), "keep")
        self.assertEqual(list(self.output.iterdir()), [marker])


if __name__ == "__main__":
    unittest.main()
