"""内部复制编排的离线模型；不执行真实数据库、S3 或 fresh-target 创建。"""
import copy
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import unittest
from unittest.mock import patch
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import test_devex_clone as fixtures
from devex_clone import verify_plan, write_plan
from devex_clone_capture import capture_object
from devex_clone_ledger import CloneLedger, LedgerError
from devex_clone_transfer import DatabaseBasis, DatabaseObservation, GenerationObservation, ObjectObservation, TransferSteps
from restore_build import file_digest
from restore_reference_io import ExternalTools, ObjectCreateError
from restore_reference_plan import plan_hash

GENERATION = "e" * 64


class OfflineGuard:
    """显式离线替身：字段不能用作真实执行许可。"""
    def __init__(self, case):
        self.case, self.basis, self.current = case, {}, {}
        self.change_generation = False
        self.source_changed = False
        self.source_checks = 0
        self.proof_selections = []
        for item in case.plan["databases"]:
            source = self.database("source", item, "source")
            initial = self.database("target", item, "initial")
            self.basis[item["key"]] = DatabaseBasis(source, initial, item["artifact"]["sha256"])
            self.current[item["key"]] = initial

    def database(self, side, item, phase):
        config = self.case.fixture.value[side]
        declared = next(db for db in config["databases"] if db["key"] == item["key"])
        tables = {name: {"rows": count if phase == "source" else 0, "sha256": plan_hash({"phase": phase, "table": name})}
                  for name, count in item["tables"].items()}
        preserved = {"ryframe_resource_ownership": {"rows": len(declared["ownership"]), "sha256": plan_hash(declared["ownership"])}}
        resource = {"kind": "database", "scope_id": config["scope_id"], "server_uuid": declared["server_uuid"], "database": declared["database"]}
        return DatabaseObservation(resource, declared["schema_sha256"], tables, preserved, tuple(tables) + tuple(preserved), tuple(declared["ownership"]))

    def generation(self, plan, declaration, tools, *, source_object=None):
        self.proof_selections.append(source_object)
        return GenerationObservation(plan["plan_sha256"], "f" * 64 if self.change_generation else GENERATION,
                                     plan["input_sha256"], plan_hash(tools.plan), "a" * 64, "b" * 64, True, True, True)

    def database_basis(self, key):
        return copy.deepcopy(self.basis[key])

    def verify_complete_snapshot(self):
        self.source_checks += 1
        if self.source_changed:
            raise ValueError("源完整清单在复制期间发生变化")

    def observe_databases(self):
        return copy.deepcopy(self.current)

    def observe_database(self, key):
        return copy.deepcopy(self.current[key])

    def observe_object(self, bucket, key):
        resource = {"kind": "object", "scope_id": "clone-target", "endpoint": self.case.tools.plan["target"]["s3"]["endpoint"], "bucket": bucket, "key": key}
        stored = self.case.runner.objects.get((bucket, key))
        if stored is None:
            return ObjectObservation(resource, None, "d" * 64)
        body, _ = stored
        expected = {"bytes": len(body), "sha256": hashlib.sha256(body).hexdigest()}
        directory = self.case.work / ("observer-" + uuid.uuid4().hex)
        capture_object(self.case.tools, "target", bucket, key, directory, expected=expected, max_bytes=len(body), owner_buckets=(bucket,))
        return ObjectObservation(resource, directory, None)

    def observe_objects(self, targets):
        return [self.observe_object(bucket, key) for bucket, key in targets]

    def verify_object_bindings(self, targets):
        planned = {(item["bucket"], item["target_key"]) for item in self.case.plan["objects"]}
        if not set(targets) <= planned:
            raise ValueError("本批源对象不属于当前计划")


class OfflineTools:
    def __init__(self, case):
        self.case, self.calls, self.objects = case, [], {}
        self.failure, self.after_commit, self.after_put = None, None, None

    def __call__(self, command, **kwargs):
        self.calls.append((command, kwargs))
        if any(part.startswith("--database=") for part in command):
            return self.mysql(command, kwargs)
        operation = command[command.index("s3api") + 1]
        bucket, key = command[command.index("--bucket") + 1], command[command.index("--key") + 1]
        if operation == "put-object":
            return self.put(command, bucket, key)
        if operation not in {"head-object", "get-object"}:
            raise AssertionError("出现非计划写入或清理命令")
        if key.endswith("/.ryframe-owner"):
            Path(command[-1]).write_bytes(f"ryframe-owner:v1:{key.split('/')[0]}:object-storage:{bucket}".encode())
            return subprocess.CompletedProcess(command, 0, stdout=b"{}")
        body, metadata = self.objects[bucket, key]
        response = {**metadata, "ContentLength": len(body), "ETag": '"fixture"', "LastModified": "2026-09-04T00:00:00+00:00"}
        if operation == "get-object":
            Path(command[-1]).write_bytes(body)
            if self.failure == "get-header-loss":
                response.pop("ContentLanguage", None)
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(response).encode())

    def mysql(self, command, kwargs):
        actual = next(part.split("=", 1)[1] for part in command if part.startswith("--database="))
        item = next(db for db in self.case.tools.plan["target"]["databases"] if db["database"] == actual)
        self.case.assertNotIn("--execute", command)
        sql = kwargs["input"].decode()
        self.case.assertTrue(sql.endswith("COMMIT;\n"))
        self.case.assertEqual(sql.count("START TRANSACTION"), 1)
        self.case.assertEqual(set(re.findall(r"DELETE FROM `([^`]+)`", sql)), set(self.case.guard.basis[item["key"]].source.tables))
        self.case.assertNotIn("DELETE FROM `ryframe_resource_ownership`", sql)
        step = next(step for step in self.case.ledger.snapshot()["steps"].values() if step["before"]["kind"] == "database" and step["phase"] == "unknown")
        self.case.assertEqual(step["artifact_sha256"], self.case.guard.basis[item["key"]].artifact_sha256)
        if self.failure != "before-commit":
            basis = self.case.guard.basis[item["key"]]
            self.case.guard.current[item["key"]] = replace(basis.initialized, tables=copy.deepcopy(basis.source.tables))
        if self.after_commit:
            self.after_commit(item["key"])
        if self.failure in {"before-commit", "commit-response-lost"}:
            raise subprocess.TimeoutExpired(command, 1800)
        return subprocess.CompletedProcess(command, 0, stdout=b"")

    def put(self, command, bucket, key):
        self.case.assertEqual(command[command.index("--if-none-match") + 1], "*")
        self.case.assertTrue(key.startswith("clone-target/"))
        self.case.assertTrue(any(step["phase"] == "unknown" for step in self.case.ledger.snapshot()["steps"].values()))
        if self.failure in {"409", "412"}:
            raise subprocess.CalledProcessError(255, command, stderr=f"An error occurred ({self.failure}) when calling the PutObject operation: fixture".encode())
        path = Path(command[command.index("--body") + 1])
        metadata = json.loads(Path(command[command.index("--cli-input-json") + 1].removeprefix("file://")).read_text())
        self.objects[bucket, key] = path.read_bytes(), metadata
        if self.after_put:
            self.after_put()
        if self.failure == "put-response-lost":
            raise subprocess.TimeoutExpired(command, 1800)
        return subprocess.CompletedProcess(command, 0, stdout=b'{"ETag":"fixture"}')


class TransferTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.CloneTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.backend = self.fixture.backend
        self.plan_file = self.backend / ".local-tests/plan.json"
        source = self.backend / ".local-tests/input.json"
        source.write_text(json.dumps(self.fixture.value), encoding="utf-8")
        write_plan(self.backend, str(source), str(self.plan_file))
        self.plan = verify_plan(self.backend, str(self.plan_file))
        self.work = self.backend / ".local-tests/transfer"
        self.work.mkdir()
        executable = self.work / "fixture-tool.exe"
        executable.write_bytes(b"offline fixture tool")
        defaults = self.work / "fixture.cnf"
        defaults.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture\n")
        tools_plan = {"tools": {name: {"path": str(executable), "sha256": file_digest(executable)["sha256"]} for name in ("mysql", "aws")}}
        for side in ("source", "target"):
            declared = self.fixture.value[side]
            tools_plan[side] = {"scope_id": declared["scope_id"], "s3": {"endpoint": declared["object_endpoint"], "region": "us-east-1", "access_key_env": "FIXTURE_ACCESS", "secret_key_env": "FIXTURE_SECRET"},
                                "databases": [{**item, "defaults_file": str(defaults), "defaults_sha256": file_digest(defaults)["sha256"]} for item in declared["databases"]]}
        self.runner = OfflineTools(self)
        self.tools = ExternalTools(tools_plan, self.work, self.runner)
        self.guard = OfflineGuard(self)
        self.env = patch.dict("os.environ", {"FIXTURE_ACCESS": "fixture-access", "FIXTURE_SECRET": "fixture-secret"})
        self.env.start(); self.addCleanup(self.env.stop)
        self.ledger_dir = self.work / "ledger"

    def open(self, *, mode="apply", create=True):
        self.ledger = CloneLedger(self.backend, self.ledger_dir, self.plan["plan_sha256"], GENERATION, create=create, mode=mode)
        return self.ledger

    def engine(self, guard=True):
        return TransferSteps(self.backend, self.plan_file, self.tools, self.ledger, self.guard if guard else None)

    def writes(self):
        return [(args, kwargs) for args, kwargs in self.runner.calls if "put-object" in args or any(part.startswith("--database=") for part in args)]

    def apply_all(self, engine):
        for item in self.plan["databases"]:
            engine.apply_database(item["key"])
        for item in self.plan["objects"]:
            engine.apply_object(item["bucket"], item["target_key"])

    def test_full_plan_uses_exact_target_tables_files_and_preserves_metadata(self):
        with self.open():
            engine = self.engine()
            self.apply_all(engine)
            engine.finish()
            with self.assertRaises(ValueError):
                engine.result()
        result = engine.result()
        self.assertEqual(result["status"], "data_steps_verified")
        self.assertEqual(result["written_steps"], 3)
        self.assertFalse(result["target_ready"])
        self.assertTrue(result["requires_final_live_verification"])
        self.assertEqual(len(self.writes()), 3)
        self.assertEqual(self.guard.source_checks, 1)
        for item, (_, kwargs) in zip(self.plan["databases"], self.writes()):
            self.assertIn((self.fixture.root / item["artifact"]["file"]).read_bytes(), kwargs["input"])

    def test_object_checkpoints_bind_source_key_for_pending_apply_and_existing(self):
        item = self.plan["objects"][0]
        expected = (item["bucket"], item["source_key"])
        with self.open():
            engine = self.engine()
            self.assertEqual(self.guard.proof_selections, [None])
            step = engine.object_step(item)
            self.guard.proof_selections.clear()
            engine.verify_pending(step)
            self.assertEqual(self.guard.proof_selections, [expected, expected])
            self.guard.proof_selections.clear()
            engine.apply_object(item["bucket"], item["target_key"])
            self.assertEqual(self.guard.proof_selections, [expected] * 3)
            self.guard.proof_selections.clear()
            engine.verify_existing(step)
            self.assertEqual(self.guard.proof_selections, [expected, expected])

    def test_object_write_uses_one_put_and_current_bucket_before_and_after_capture(self):
        item = self.plan["objects"][0]
        with self.open():
            engine = self.engine()
            engine.apply_object(item["bucket"], item["target_key"])
        commands = [command for command, _ in self.runner.calls]
        self.assertEqual([command[command.index("s3api") + 1] for command in commands],
                         ["put-object", "get-object", "head-object", "get-object", "head-object", "get-object"])
        owners = [command for command in commands if command[command.index("--key") + 1].endswith("/.ryframe-owner")]
        self.assertEqual(len(owners), 2)
        self.assertEqual([command[command.index("--bucket") + 1] for command in owners], [item["bucket"]] * 2)

    def test_object_proof_failure_at_prewrite_checkpoint_keeps_unknown_intent_without_put(self):
        item = self.plan["objects"][0]
        expected = (item["bucket"], item["source_key"])
        original, calls = self.guard.generation, []
        def generation(plan, declaration, tools, *, source_object=None):
            calls.append(source_object)
            if calls == [expected, expected]:
                raise ValueError("当前对象证明在写入前损坏")
            return original(plan, declaration, tools, source_object=source_object)
        with self.assertRaisesRegex(ValueError, "对象证明"), self.open():
            engine = self.engine()
            with patch.object(self.guard, "generation", side_effect=generation):
                engine.apply_object(item["bucket"], item["target_key"])
        self.assertEqual(self.writes(), [])
        self.assertEqual(next(iter(self.ledger.inspect()["steps"].values()))["phase"], "unknown")

    def test_object_reconcile_and_resume_each_bind_the_same_source_proof(self):
        item = self.plan["objects"][0]
        expected = (item["bucket"], item["source_key"])
        self.runner.failure = "409"
        with self.assertRaises(ObjectCreateError), self.open():
            self.engine().apply_object(item["bucket"], item["target_key"])
        self.runner.failure = None
        with self.open(create=False, mode="reconcile"):
            engine = self.engine()
            self.guard.proof_selections.clear()
            step = engine.object_step(item)
            self.assertEqual(engine.reconcile(step), "reconciled_before")
            self.assertEqual(self.guard.proof_selections, [expected, expected])
        with self.open(create=False, mode="resume"):
            engine = self.engine()
            self.guard.proof_selections.clear()
            engine.resume(step)
            self.assertEqual(self.guard.proof_selections, [expected] * 3)

    def test_sql_and_constructor_use_global_proof_and_unknown_step_never_reaches_guard(self):
        with self.open():
            engine = self.engine()
            engine.apply_database(self.plan["databases"][0]["key"])
            self.assertTrue(all(value is None for value in self.guard.proof_selections))
            count = len(self.guard.proof_selections)
            with self.assertRaisesRegex(ValueError, "当前完整计划"):
                engine._check("not-in-plan")
            self.assertEqual(len(self.guard.proof_selections), count)

    def test_missing_fresh_guard_or_ready_dictionary_never_writes(self):
        with self.open():
            with self.assertRaises(ValueError):
                self.engine(False)
            with patch.object(self.guard, "generation", return_value={"target_ready": True}), self.assertRaises(ValueError):
                self.engine()
        self.assertEqual(self.writes(), [])

    def test_source_changes_at_final_inventory_prevent_completion_receipt(self):
        with self.assertRaises(ValueError), self.open():
            engine = self.engine()
            self.apply_all(engine)
            self.guard.source_changed = True
            engine.finish()
        self.assertEqual(self.guard.source_checks, 1)
        self.assertNotEqual(self.ledger.inspect()["status"], "ledger_evidence_complete")
        with self.assertRaises(ValueError):
            engine.result()

    def test_wrong_generation_and_physical_tool_overlap_never_write(self):
        with self.open():
            self.guard.change_generation = True
            with self.assertRaises(ValueError):
                self.engine()
            self.guard.change_generation = False
            self.tools.plan["target"]["databases"][0]["database"] = self.tools.plan["source"]["databases"][0]["database"]
            with self.assertRaises(ValueError):
                self.engine()
        self.assertEqual(self.writes(), [])

    def test_injected_table_or_changed_artifact_never_writes(self):
        with self.open():
            engine = self.engine()
            engine.databases[next(iter(engine.databases))]["tables"]["ryframe_resource_ownership"] = 1
            with self.assertRaises(ValueError):
                engine.apply_database("shared-control")
        self.assertEqual(self.writes(), [])

    def test_changed_artifact_after_engine_creation_is_rejected(self):
        with self.open():
            engine = self.engine()
            (self.fixture.root / "shared-control.sql").write_text("changed")
            with self.assertRaises(ValueError):
                engine.apply_database("shared-control")
        self.assertEqual(self.writes(), [])

    def test_unknown_names_and_source_object_key_cannot_expand_plan(self):
        with self.open():
            engine = self.engine()
            with self.assertRaises(ValueError):
                engine.apply_database("not-in-plan")
            with self.assertRaises(ValueError):
                engine.apply_object("uploads", "clone-source/system/file.txt")
        self.assertEqual(self.writes(), [])

    def test_commit_response_lost_requires_reconcile_and_does_not_replay(self):
        self.runner.failure = "commit-response-lost"
        with self.assertRaises(subprocess.TimeoutExpired), self.open():
            self.engine().apply_database("shared-control")
        with self.assertRaises(LedgerError), self.open(create=False):
            pass
        self.runner.failure = None
        with self.open(create=False, mode="reconcile"):
            engine = self.engine()
            step = next(iter(self.ledger.snapshot()["steps"]))
            self.assertEqual(engine.reconcile(step), "confirmed")
        with self.open(create=False):
            engine = self.engine()
            with self.assertRaises(ValueError):
                engine.apply_database("shared-control")
        self.assertEqual(len(self.writes()), 1)

    def test_before_commit_failure_allows_only_explicit_reconcile_then_resume(self):
        self.runner.failure = "before-commit"
        with self.assertRaises(subprocess.TimeoutExpired), self.open():
            self.engine().apply_database("shared-control")
        self.runner.failure = None
        with self.open(create=False, mode="reconcile"):
            engine = self.engine()
            step = next(iter(self.ledger.snapshot()["steps"]))
            self.assertEqual(engine.reconcile(step), "reconciled_before")
        with self.open(create=False, mode="resume"):
            self.assertEqual(self.engine().resume(step), "confirmed")
        self.assertEqual(len(self.writes()), 2)

    def test_post_image_or_preserved_owner_change_is_unknown_and_blocks_next_write(self):
        def damage(key):
            current = self.guard.current[key]
            self.guard.current[key] = replace(current, preserved={"ryframe_resource_ownership": {"rows": 2, "sha256": "0" * 64}})
        self.runner.after_commit = damage
        with self.assertRaises(LedgerError), self.open():
            engine = self.engine()
            engine.apply_database("shared-control")
        self.assertEqual(len(self.writes()), 1)
        self.assertEqual(next(iter(self.ledger.snapshot()["steps"].values()))["phase"], "unknown")

    def test_put_response_lost_is_readonly_reconciled_and_not_repeated(self):
        self.runner.failure = "put-response-lost"
        with self.assertRaises(ObjectCreateError), self.open():
            self.engine().apply_object("uploads", "clone-target/system/file.txt")
        self.runner.failure = None
        with self.open(create=False, mode="reconcile"):
            engine = self.engine()
            step = next(iter(self.ledger.snapshot()["steps"]))
            self.assertEqual(engine.reconcile(step), "confirmed")
        self.assertEqual(len(self.writes()), 1)

    def test_409_and_412_are_not_retried_or_adopted(self):
        for status in ("409", "412"):
            with self.subTest(status=status):
                self.ledger_dir = self.work / ("ledger-" + status)
                self.runner.failure = status
                before = len(self.writes())
                with self.assertRaises(ObjectCreateError), self.open():
                    self.engine().apply_object("uploads", "clone-target/system/file.txt")
                self.assertEqual(len(self.writes()), before + 1)
                self.assertEqual(next(iter(self.ledger.snapshot()["steps"].values()))["phase"], "unknown")

    def test_preexisting_object_is_not_adopted_or_overwritten(self):
        self.runner.objects["uploads", "clone-target/system/file.txt"] = b"data", {"ContentType": "text/plain", "Metadata": {}}
        with self.assertRaises(ValueError), self.open():
            self.engine().apply_object("uploads", "clone-target/system/file.txt")
        self.assertEqual(self.writes(), [])

    def test_metadata_difference_fails_confirmation_without_erasing_fields(self):
        def damage():
            body, metadata = self.runner.objects["uploads", "clone-target/system/file.txt"]
            self.runner.objects["uploads", "clone-target/system/file.txt"] = body, {**metadata, "ContentLanguage": "en"}
        self.runner.after_put = damage
        with self.assertRaises(LedgerError), self.open():
            self.engine().apply_object("uploads", "clone-target/system/file.txt")
        self.assertEqual(len(self.writes()), 1)
        captures = list(self.work.glob("transfer-capture-*/capture.json"))
        self.assertEqual(json.loads(captures[0].read_text(encoding="utf-8"))["metadata"]["ContentLanguage"], "en")

    def test_partial_work_never_finishes_and_confirmed_database_not_replayed(self):
        with self.open():
            engine = self.engine()
            engine.apply_database("shared-control")
        with self.open(create=False):
            engine = self.engine()
            with self.assertRaises(ValueError):
                engine.finish()
        self.assertEqual(len(self.writes()), 1)

    def test_complete_unchanged_database_is_readonly_and_bound_in_result(self):
        key = "dedicated-a"
        basis = self.guard.basis[key]
        initial = replace(basis.initialized, tables=basis.source.tables)
        self.guard.basis[key] = replace(basis, initialized=initial)
        self.guard.current[key] = initial
        with self.open():
            engine = self.engine()
            self.apply_all(engine)
            engine.finish()
        self.assertEqual(engine.result()["unchanged_databases"], 1)
        self.assertEqual(len(self.writes()), 2)

    def test_confirmation_write_failure_keeps_unknown_resource_and_no_success(self):
        with self.assertRaises(OSError), self.open():
            engine = self.engine()
            with patch.object(self.ledger, "confirm", side_effect=OSError("fixture evidence failure")):
                engine.apply_database("shared-control")
        with self.assertRaises(ValueError):
            engine.result()
        self.assertEqual(len(self.writes()), 1)

    def test_finished_ledger_receipt_failure_cannot_publish_data_success(self):
        with self.assertRaises(ExceptionGroup), self.open():
            engine = self.engine()
            self.apply_all(engine)
            engine.finish()
            self.ledger._publish = lambda: (_ for _ in ()).throw(OSError("fixture receipt failure"))
        with self.assertRaises(ValueError):
            engine.result()

    def test_target_started_or_source_running_denies_write(self):
        original = self.guard.generation
        with self.open():
            for changes in ({"target_stopped": False}, {"source_stopped": False}, {"target_never_started": False}):
                with patch.object(self.guard, "generation", side_effect=lambda *args, **kwargs: replace(original(*args, **kwargs), **changes)), self.assertRaises(ValueError):
                    self.engine()
        self.assertEqual(self.writes(), [])

    def test_database_reconcile_records_mismatch_and_resume_never_writes(self):
        self.runner.failure = "before-commit"
        with self.assertRaises(subprocess.TimeoutExpired), self.open():
            self.engine().apply_database("shared-control")
        current = self.guard.current["shared-control"]
        changed = copy.deepcopy(current.tables)
        changed["sys_user"]["sha256"] = "9" * 64
        self.guard.current["shared-control"] = replace(current, tables=changed)
        with self.open(create=False, mode="reconcile"):
            engine = self.engine()
            step = next(iter(self.ledger.snapshot()["steps"]))
            self.assertEqual(engine.reconcile(step), "mismatch")
        with self.assertRaises(LedgerError), self.open(create=False, mode="resume"):
            pass
        self.assertEqual(len(self.writes()), 1)

    def test_reconcile_cannot_accept_intent_for_different_logical_image(self):
        with self.open():
            engine = self.engine()
            item = self.plan["databases"][0]
            step = engine.db_step(item)
            before, after = engine._database_basis(item)
            self.ledger.intent(step, before, {**after, "tables_sha256": "7" * 64}, item["artifact"]["sha256"])
        with self.assertRaises(ValueError), self.open(create=False, mode="reconcile"):
            self.engine().reconcile(step)
        self.assertEqual(self.writes(), [])

    def test_get_header_loss_is_preserved_and_not_confirmed(self):
        def damage():
            body, metadata = self.runner.objects["uploads", "clone-target/system/file.txt"]
            self.runner.objects["uploads", "clone-target/system/file.txt"] = body, {**metadata, "ContentLanguage": "en"}
        self.runner.after_put = damage
        self.runner.failure = "get-header-loss"
        with self.assertRaises(LedgerError), self.open():
            self.engine().apply_object("uploads", "clone-target/system/file.txt")
        result = json.loads(next(self.work.glob("transfer-capture-*/capture.json")).read_text(encoding="utf-8"))
        self.assertFalse(result["get_header_consistent"])
        self.assertEqual(result["get_header_differences"][0]["field"], "ContentLanguage")

    def test_success_then_live_data_changed_cannot_finish(self):
        with self.open():
            engine = self.engine()
            self.apply_all(engine)
            current = self.guard.current["shared-control"]
            tables = copy.deepcopy(current.tables)
            tables["sys_user"]["sha256"] = "8" * 64
            self.guard.current["shared-control"] = replace(current, tables=tables)
            with self.assertRaises(ValueError):
                engine.finish()
        self.assertFalse(list(self.ledger_dir.glob("complete-*.json")))

    def test_absent_or_extra_table_in_observer_cannot_be_silently_ignored(self):
        initial = self.guard.current["shared-control"]
        self.guard.current["shared-control"] = replace(initial, all_tables=initial.all_tables + ("unknown_table",))
        with self.assertRaises(ValueError), self.open():
            self.engine().apply_database("shared-control")
        self.assertEqual(self.writes(), [])

    def test_lock_release_failure_prevents_data_success(self):
        with self.assertRaises(ExceptionGroup), self.open():
            engine = self.engine()
            self.apply_all(engine)
            engine.finish()
            self.ledger._release = lambda: (_ for _ in ()).throw(OSError("fixture lock failure"))
        with self.assertRaises(ValueError):
            engine.result()

    def test_excluded_tenant_migration_snapshot_is_preserved(self):
        key = "dedicated-a"
        basis = self.guard.basis[key]
        def migration(value):
            return replace(value, preserved={**value.preserved, "seaql_tenant_data_migrations": {"rows": 1, "sha256": "2" * 64}},
                           all_tables=value.all_tables + ("seaql_tenant_data_migrations",))
        self.guard.basis[key] = replace(basis, source=migration(basis.source), initialized=migration(basis.initialized))
        self.guard.current[key] = self.guard.basis[key].initialized
        with self.open():
            self.engine().apply_database(key)
        self.assertNotIn(b"DELETE FROM `seaql_tenant_data_migrations`", self.writes()[0][1]["input"])


if __name__ == "__main__":
    unittest.main()
