"""过期授权快照与同代失败续验；未知写入只按完整前后像处理。"""

import contextlib
import copy
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
import restore_source_runtime as runtime
import restore_source_runtime_producer as producer
import source_fingerprints
import test_restore_source as source_fixture
import test_restore_source_runtime as fixture


class CacheResources(fixture.FakeResources):
    def __init__(self, lineage, subjects):
        super().__init__(lineage, subjects)
        self.expired = set()
        self.missing = set()
        self.eval_calls = []
        self.drop_result = False
        self.before_eval = None
        self.output = None
        self.added_rows = None

    def _redis(self, parts):
        if parts[0] == "TYPE" and parts[1] in self.expired | self.missing:
            return "none"
        if parts[0] == "EVAL":
            self.eval_calls.append(parts)
            if self.before_eval:
                self.before_eval()
            result = super()._redis(parts)
            if self.drop_result:
                raise OSError("fixture: Lua result lost")
            return result
        return super()._redis(parts)

    def _mysql(self, database, sql):
        raw = super()._mysql(database, sql)
        if "WHERE `id` >" in sql:
            if self.added_rows is None:
                self.added_rows = raw
            raw = self.added_rows
        if self.output:
            write_json(self.output / f"mysql-shared-control-{uuid.uuid4().hex}.command.json", {
                "command": [sys.executable, "mysql-fixture"], "returncode": 0,
                "error_type": None, "stdout": raw, "stderr": "",
            })
        return raw


class SourceRuntimeRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixture.SourceRuntimeTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        f = self.fixture
        self.resources = CacheResources(f.lineage_value, f.subjects)
        self.namespace = self.resources.selected["redis"]["namespace"]
        self.output = f.generation / "verification/source-runtime.json"
        self.plan = runtime.authorization_cache_plan(self.namespace, f.subjects)

    def resources_factory(self, _backend, _facts, output, _run):
        self.resources.output = output
        return self.resources

    def capture(self, *args, **kwargs):
        descriptor = self.fixture.capture(*args, **kwargs)
        source_fixture.capture_dynamic_evidence(Path(descriptor["path"]).parent)
        return descriptor

    @contextlib.contextmanager
    def context(self):
        f = self.fixture
        tools = SimpleNamespace(command=lambda _name: [sys.executable])
        with f.modules(), patch.object(runtime, "_resources", side_effect=self.resources_factory), \
                patch.object(sys.modules["devex_clone_seed_generation_images"], "capture_image", self.capture), \
                patch.object(runtime, "ExternalTools", return_value=tools), \
                patch("restore_reference_io.ExternalTools", return_value=tools), \
                patch.object(producer, "process_identity", side_effect=f.identity):
            yield

    def failed_prefix(self):
        f = self.fixture
        with self.context(), patch.object(runtime, "_clean_authorization_cache", side_effect=ValueError("expired")):
            with self.assertRaisesRegex(ValueError, "expired"):
                runtime.execute_source_verification(f.backend, Path(f.start["path"]), self.output, popen=f.fake_popen)
        return runtime._manifest(self.output.parent)

    @contextlib.contextmanager
    def recovery_context(self):
        f = self.fixture
        successor = copy.deepcopy(f.coordinator_source)
        successor["fingerprints"]["test_tools"]["sha256"] = "f" * 64

        @contextlib.contextmanager
        def registered(_backend, descriptor):
            self.assertEqual(descriptor, f.start)
            source_fingerprints.require_current_execution_source(f.backend, successor)
            yield lambda: f.facts, successor

        generation = SimpleNamespace(
            verify_running_source=lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("ordinary verifier")),
            verify_historical_running_source=lambda *_args, **_kwargs: f.facts,
            registered_historical_running_source=registered,
        )
        with self.context(), patch.dict(sys.modules, {"devex_clone_seed_generation": generation}), \
                patch.object(source_fingerprints, "current_execution_source", return_value=successor), \
                patch.object(runtime, "run_source_producer", side_effect=AssertionError("must not replay Node")):
            yield successor

    def test_ttl_expired_snapshots_are_accounted_and_persistent_keys_remain_required(self):
        self.resources.expired = {row["keys"]["snapshot"] for row in self.plan[:3]}
        self.output.parent.mkdir()
        cleanup = runtime._clean_authorization_cache(
            self.resources, self.namespace, self.fixture.subjects, self.output.parent / "cache-cleanup.json"
        )
        self.assertEqual(cleanup["deleted"], 30)
        self.assertEqual(len(cleanup["expired_snapshot_keys"]), 3)
        runtime._verify_cache_receipt(self.fixture.backend, self.output.parent,
                                     binding(self.output.parent / "cache-cleanup.json"), self.fixture.subjects)
        self.assertEqual(len(self.resources.eval_calls), 1)
        self.resources.deleted = False
        self.resources.missing.add(self.plan[0]["keys"]["tenant_epoch"])
        with self.assertRaisesRegex(ValueError, "持久键缺失"):
            runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        self.assertEqual(len(self.resources.eval_calls), 1)

    def test_receipt_rejects_live_hash_counted_as_expired(self):
        observation, _ = runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        with self.assertRaisesRegex(ValueError, "键或版本"):
            runtime._validate_cache_before(observation["before"], self.plan, [self.plan[0]["keys"]["snapshot"]])

    def test_cas_rechecks_state_after_observation_and_never_cleans_changed_keys(self):
        observation, comparisons = runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        self.resources.before_eval = lambda: self.resources.expired.add(self.plan[0]["keys"]["snapshot"])
        with self.assertRaises(AssertionError):
            runtime._execute_cache_cleanup(self.resources, observation, comparisons, self.output)
        self.assertFalse(self.resources.deleted)
        self.assertFalse(self.output.exists())

    def test_unknown_lua_result_uses_persisted_order_and_reconciles_without_inventing_deletions(self):
        observation, _ = runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        intent = json.loads(json.dumps({"authorization_cache": observation}, sort_keys=True))
        self.resources.drop_result = True
        self.output.parent.mkdir()
        cleanup_path = self.output.parent / "cache-cleanup.json"
        with self.assertRaisesRegex(OSError, "result lost"):
            runtime._resume_or_execute_cache_cleanup(self.resources, intent, cleanup_path)
        self.assertFalse(cleanup_path.exists())
        cleanup = runtime._resume_or_execute_cache_cleanup(self.resources, intent, cleanup_path)
        self.assertEqual(len(self.resources.eval_calls), 1)
        self.assertIsNone(cleanup["deleted"])
        self.assertEqual(cleanup["execution"], "reconciled_unknown_lua_result")
        writes = runtime._remote_write_receipt(cleanup, recovered=True)
        self.assertEqual(writes["authorization_cache"]["historical_cleanup"], "unknown")
        self.assertEqual(writes["authorization_cache"]["confirmed_commands"], 0)
        runtime._verify_cache_receipt(self.fixture.backend, self.output.parent,
                                     binding(cleanup_path), self.fixture.subjects)

    def test_unknown_lua_mixed_state_fails_without_replay(self):
        observation, _ = runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        self.resources.missing.add(self.plan[0]["keys"]["tenant_epoch"])
        with self.assertRaisesRegex(ValueError, "持久键缺失|混合状态"):
            runtime._resume_or_execute_cache_cleanup(self.resources, {"authorization_cache": observation}, self.output)
        self.assertEqual(self.resources.eval_calls, [])

    def test_failed_prefix_recovers_in_place_with_original_audit_and_no_node_replay(self):
        original = self.failed_prefix()
        self.resources.expired = {row["keys"]["snapshot"] for row in self.plan}
        with self.recovery_context():
            result = runtime.execute_source_verification_recovery(
                self.fixture.backend, Path(self.fixture.start["path"]), self.output
            )
            self.assertEqual(result["status"], "source_runtime_verified")
            verified = runtime.verify_source_runtime(self.fixture.backend, binding(self.output), live=True)
            self.assertEqual(verified["after"]["image"], self.fixture.after_image)
            receipt = verified["receipt"]
            self.assertEqual(receipt["remote_writes"]["authorization_cache"]["deleted_keys"], 22)
            self.assertEqual(receipt["remote_writes"]["authorization_cache"]["historical_cleanup"], "unknown")
            intent = read_json(self.output.parent / runtime.RECOVERY_INTENT)
            self.assertEqual(intent["pre_recovery_manifest"], original)
            with patch.object(source_fingerprints, "current_execution_source", return_value=self.fixture.coordinator_source):
                with self.assertRaisesRegex(ValueError, "test_tools"):
                    runtime.verify_source_runtime(self.fixture.backend, binding(self.output), live=True)
                runtime.verify_source_runtime(self.fixture.backend, binding(self.output), live=False)

    def test_pause_does_not_allow_same_count_login_row_change(self):
        self.failed_prefix()
        self.resources.old = self.resources.old.replace("6F6C642D75736572".upper(), "6F74686572".upper())
        with self.recovery_context(), self.assertRaisesRegex(ValueError, "未知写入"):
            runtime.execute_source_verification_recovery(
                self.fixture.backend, Path(self.fixture.start["path"]), self.output
            )
        self.assertEqual(self.resources.eval_calls, [])
        self.assertFalse(self.output.exists())

    def test_lost_cleanup_response_resumes_same_intent_without_second_eval_or_login(self):
        original = self.failed_prefix()
        self.resources.expired = {row["keys"]["snapshot"] for row in self.plan}
        self.resources.drop_result = True
        with self.recovery_context():
            with self.assertRaisesRegex(OSError, "result lost"):
                runtime.execute_source_verification_recovery(
                    self.fixture.backend, Path(self.fixture.start["path"]), self.output
                )
            intent_path = self.output.parent / runtime.RECOVERY_INTENT
            frozen = binding(intent_path)
            self.assertEqual(read_json(intent_path)["pre_recovery_manifest"], original)
            result = runtime.execute_source_verification_recovery(
                self.fixture.backend, Path(self.fixture.start["path"]), self.output
            )
            self.assertEqual(result["status"], "source_runtime_verified")
            self.assertEqual(binding(intent_path), frozen)
            self.assertEqual(len(self.resources.eval_calls), 1)
            cleanup = read_json(self.output.parent / "cache-cleanup.json")
            self.assertIsNone(cleanup["deleted"])
            runtime.verify_source_runtime(self.fixture.backend, binding(self.output), live=False)

    def test_capture_does_not_hide_new_login_row_content_change(self):
        self.failed_prefix()
        capture = self.fixture.capture

        def changed_capture(*args, **kwargs):
            result = capture(*args, **kwargs)
            self.resources.added_rows = self.resources.added_rows.replace("6F776E6572".upper(), "6F74686572".upper())
            return result

        with self.recovery_context(), \
                patch.object(sys.modules["devex_clone_seed_generation_images"], "capture_image", changed_capture), \
                self.assertRaisesRegex(ValueError, "未知写入"):
            runtime.execute_source_verification_recovery(
                self.fixture.backend, Path(self.fixture.start["path"]), self.output
            )
        self.assertEqual(len(self.resources.eval_calls), 1)
        self.assertFalse(self.output.exists())

    def test_pending_intent_rejects_changed_original_evidence_without_cache_replay(self):
        self.failed_prefix()
        self.resources.drop_result = True
        with self.recovery_context():
            with self.assertRaises(OSError):
                runtime.execute_source_verification_recovery(
                    self.fixture.backend, Path(self.fixture.start["path"]), self.output
                )
            audit_path = self.output.parent / "audit/login-after-new.tsv"
            audit_path.write_bytes(audit_path.read_bytes() + b"unknown")
            with self.assertRaisesRegex(ValueError, "证据已变化|原始行证据"):
                runtime.execute_source_verification_recovery(
                    self.fixture.backend, Path(self.fixture.start["path"]), self.output
                )
        self.assertEqual(len(self.resources.eval_calls), 1)
        self.assertFalse(self.output.exists())

    def test_all_absent_does_not_bypass_pending_cache_plan_validation(self):
        self.failed_prefix()
        self.resources.drop_result = True
        with self.recovery_context():
            with self.assertRaises(OSError):
                runtime.execute_source_verification_recovery(
                    self.fixture.backend, Path(self.fixture.start["path"]), self.output
                )
            intent_path = self.output.parent / runtime.RECOVERY_INTENT
            intent = read_json(intent_path)
            intent["authorization_cache"]["plan"][0]["keys"]["tenant_epoch"] += ":unknown"
            intent_path.write_text(json.dumps(intent), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "缓存计划不同"):
                runtime.execute_source_verification_recovery(
                    self.fixture.backend, Path(self.fixture.start["path"]), self.output
                )
        self.assertEqual(len(self.resources.eval_calls), 1)
        self.assertFalse(self.output.exists())

    def test_ordinary_receipt_cannot_claim_unknown_cleanup_without_recovery_intent(self):
        f = self.fixture
        with self.context():
            runtime.execute_source_verification(f.backend, Path(f.start["path"]), self.output, popen=f.fake_popen)
            cleanup_path = self.output.parent / "cache-cleanup.json"
            cleanup = read_json(cleanup_path)
            cleanup.update(deleted=None, execution="reconciled_unknown_lua_result")
            cleanup_path.write_text(json.dumps(cleanup), encoding="utf-8")
            receipt = read_json(self.output)
            receipt.update(cache_cleanup=binding(cleanup_path), remote_writes=runtime._remote_write_receipt(cleanup, recovered=False))
            self.output.write_text(json.dumps(receipt), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "副作用"):
                runtime.verify_source_runtime(f.backend, binding(self.output), live=False)


if __name__ == "__main__":
    unittest.main()
