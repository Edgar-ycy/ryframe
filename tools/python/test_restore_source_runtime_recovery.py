"""过期授权快照与同代失败续验；未知写入只按完整前后像处理。"""

import contextlib
from concurrent.futures import ThreadPoolExecutor
import copy
import subprocess
from native_s3_client import _invoke as native_invoke, _read_response as native_response
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
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
        self.cleanup_calls = []
        self.drop_result = False
        self.omit_execution = False
        self.before_cleanup = None
        self.output = None
        self.added_rows = None

    def _redis(self, parts):
        if parts[0] == "TYPE" and parts[1] in self.expired | self.missing:
            return "none"
        return super()._redis(parts)

    def _cache_cleanup_native(self, observation, comparisons, output):
        attempt = self.cache_attempt(observation, output)
        self.cleanup_calls.append((observation, comparisons))
        if self.before_cleanup:
            self.before_cleanup()
            result = {"status": "watch_conflict", "deleted": None, "expired": [],
                      "message": "fixture: EXEC 已中止；保留现场人工核对，不重试"}
        else:
            self.deleted = True
            expired = [row["snapshot"]["key"] for row in observation["before"] if row["snapshot"]["type"] == "none"]
            result = {"status": "confirmed", "deleted": 33-len(expired), "expired": expired}
            if self.drop_result:
                result = {"status": "unknown_exec_result", "deleted": None, "expired": []}
        if not self.omit_execution:
            self.cache_execution(output, attempt, result)
        if self.drop_result:
            raise OSError("fixture: EXEC result lost")
        return result

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
        self.plan = native_response(*native_invoke(
            "__source-evidence", {
                "operation": 'cache-plan',
                "write": False,
                "arguments": {
                    'namespace': self.namespace,
                    'subjects': f.subjects,
                },
            }, subprocess.run, may_write=False,
        ))[0]["value"]

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
        with f.modules(), patch.object(runtime, "_resources", side_effect=self.resources_factory), \
                patch.object(sys.modules["devex_clone_seed_generation_images"], "capture_image", self.capture), \
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
        with self.context():
            runtime.execute_source_verification(self.fixture.backend, Path(self.fixture.start["path"]),
                                                self.output, popen=self.fixture.fake_popen)
        cleanup = read_json(self.output.parent / "cache-cleanup.json")
        self.assertEqual(cleanup["deleted"], 30)
        self.assertEqual(len(cleanup["expired_snapshot_keys"]), 3)
        runtime._verify_cache_receipt(self.fixture.backend, self.output.parent,
                                     binding(self.output.parent / "cache-cleanup.json"), self.fixture.subjects)
        self.assertEqual(len(self.resources.cleanup_calls), 1)
        self.resources.deleted = False
        self.resources.missing.add(self.plan[0]["keys"]["tenant_epoch"])
        with self.assertRaisesRegex(ValueError, "持久键缺失"):
            runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        self.assertEqual(len(self.resources.cleanup_calls), 1)

    def test_watch_conflict_cannot_reconcile_external_absence_or_retry(self):
        self.resources.before_cleanup = lambda: self.resources.expired.add(self.plan[0]["keys"]["snapshot"])
        with self.context(), self.assertRaisesRegex(ValueError, "EXEC 已中止"):
            runtime.execute_source_verification(self.fixture.backend, Path(self.fixture.start["path"]),
                                                self.output, popen=self.fixture.fake_popen)
        self.assertFalse(self.resources.deleted)
        self.resources.deleted = True
        with self.recovery_context(), self.assertRaisesRegex(ValueError, "明确未执行"):
            runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
        self.assertEqual(len(self.resources.cleanup_calls), 1)
        self.assertEqual(read_json(self.output.parent / "cache-cleanup-execution.json")["status"], "watch_conflict")

    def test_unknown_exec_same_values_and_mixed_state_fail_without_replay(self):
        self.failed_prefix()
        self.resources.drop_result = True
        with self.recovery_context():
            with self.assertRaises(OSError):
                runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
            self.resources.deleted = False
            with self.assertRaisesRegex(ValueError, "禁止重放"):
                runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
            self.resources.missing.add(self.plan[0]["keys"]["tenant_epoch"])
            with self.assertRaisesRegex(ValueError, "禁止重放"):
                runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
        self.assertEqual(len(self.resources.cleanup_calls), 1)

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
            self.assertEqual(receipt["remote_writes"]["authorization_cache"]["historical_cleanup"], "not_attempted")
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
        self.assertEqual(self.resources.cleanup_calls, [])
        self.assertFalse(self.output.exists())

    def test_lost_cleanup_response_resumes_same_intent_without_second_exec_or_login(self):
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
            self.assertEqual(producer.require_source_verifier_stopped(self.fixture.backend, self.fixture.start)["status"], "stopped")
            execution_path = self.output.parent / "cache-cleanup-execution.json"
            attempt_path = self.output.parent / "cache-cleanup-attempt.json"
            execution_bytes, attempt_bytes = execution_path.read_bytes(), attempt_path.read_bytes()
            execution_path.write_bytes(b"{}")
            with self.assertRaises(ValueError):
                producer.require_source_verifier_stopped(self.fixture.backend, self.fixture.start)
            execution_path.write_bytes(execution_bytes)
            attempt_path.unlink()
            with self.assertRaises(ValueError):
                producer.require_source_verifier_stopped(self.fixture.backend, self.fixture.start)
            attempt_path.write_bytes(attempt_bytes)
            result = runtime.execute_source_verification_recovery(
                self.fixture.backend, Path(self.fixture.start["path"]), self.output
            )
            self.assertEqual(result["status"], "source_runtime_verified")
            self.assertEqual(binding(intent_path), frozen)
            self.assertEqual(len(self.resources.cleanup_calls), 1)
            cleanup = read_json(self.output.parent / "cache-cleanup.json")
            self.assertIsNone(cleanup["deleted"])
            self.assertEqual(cleanup["execution"], "reconciled_unknown_exec_result")
            self.assertEqual(cleanup["attempt_phase"], "recovery")
            runtime.verify_source_runtime(self.fixture.backend, binding(self.output), live=False)

    def test_attempt_only_interruption_reconciles_absence_without_reexecuting(self):
        self.failed_prefix()
        self.resources.drop_result = True
        self.resources.omit_execution = True
        with self.recovery_context():
            with self.assertRaises(OSError):
                runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
            self.assertFalse((self.output.parent / "cache-cleanup-execution.json").exists())
            self.assertEqual(producer.require_source_verifier_stopped(self.fixture.backend, self.fixture.start)["status"], "stopped")
            runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
        cleanup = read_json(self.output.parent / "cache-cleanup.json")
        self.assertIsNone(cleanup["execution_record"])
        self.assertIsNone(cleanup["deleted"])
        self.assertEqual(len(self.resources.cleanup_calls), 1)

    def test_confirmed_result_with_lost_receipt_is_republished_as_confirmed(self):
        with self.context(), patch.object(runtime, "_publish_cache_cleanup", side_effect=OSError("receipt lost")):
            with self.assertRaisesRegex(OSError, "receipt lost"):
                runtime.execute_source_verification(self.fixture.backend, Path(self.fixture.start["path"]),
                                                    self.output, popen=self.fixture.fake_popen)
        attempt = binding(self.output.parent / "cache-cleanup-attempt.json")
        execution = binding(self.output.parent / "cache-cleanup-execution.json")
        with self.recovery_context():
            runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
        cleanup = read_json(self.output.parent / "cache-cleanup.json")
        self.assertEqual(cleanup["attempt"], attempt)
        self.assertEqual(cleanup["execution_record"], execution)
        self.assertEqual(cleanup["execution"], "confirmed_watch_exec")
        self.assertEqual(cleanup["deleted"], 33)
        recovery = read_json(self.output.parent / runtime.RECOVERY_RECEIPT)
        self.assertEqual(recovery["historical_cleanup"], "confirmed")
        self.assertEqual(recovery["confirmed_cleanup"], "republished_confirmed_watch_exec")
        self.assertEqual(len(self.resources.cleanup_calls), 1)

    def test_native_attempt_race_is_unique_and_pre_exec_failure_never_retries(self):
        self.failed_prefix()
        with self.recovery_context(), patch.object(runtime, "_execute_cache_cleanup", side_effect=ValueError("before attempt")):
            with self.assertRaisesRegex(ValueError, "before attempt"):
                runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
        observation, comparisons = runtime._cache_observation(self.resources, self.namespace, self.fixture.subjects)
        # 此独占夹具无 storage.redis.port，实际原生 Client 在连接前拒绝；不访问 Redis。
        request = read_json(self.fixture.copy_run / "target/request.json")
        arguments = {"backend": str(self.fixture.backend), "directory": str(self.output.parent),
                     "output": str(self.output.parent / "cache-cleanup.json"), "request": request,
                     "environment": {"APP_ENV": "test"}, "observation": observation, "comparisons": comparisons}

        def invoke():
            try:
                return native_response(*native_invoke("__source-evidence", {
                    "operation": "runtime-cache-cleanup", "write": True, "arguments": arguments,
                }, subprocess.run, may_write=True))[0]["value"]
            except ValueError:
                return None

        with ThreadPoolExecutor(max_workers=2) as workers:
            results = list(workers.map(lambda _: invoke(), range(2)))
        completed = [value for value in results if value is not None]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["status"], "pre_exec_io_failed")
        attempt = binding(self.output.parent / "cache-cleanup-attempt.json")
        execution = binding(self.output.parent / "cache-cleanup-execution.json")
        self.resources.deleted = True
        with self.recovery_context(), self.assertRaisesRegex(ValueError, "明确未执行"):
            runtime.execute_source_verification_recovery(self.fixture.backend, Path(self.fixture.start["path"]), self.output)
        self.assertEqual(binding(self.output.parent / "cache-cleanup-attempt.json"), attempt)
        self.assertEqual(binding(self.output.parent / "cache-cleanup-execution.json"), execution)
        self.assertEqual(self.resources.cleanup_calls, [])

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
        self.assertEqual(len(self.resources.cleanup_calls), 1)
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
        self.assertEqual(len(self.resources.cleanup_calls), 1)
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
            with self.assertRaisesRegex(ValueError, "缓存恢复授权与登记摘要不一致"):
                runtime.execute_source_verification_recovery(
                    self.fixture.backend, Path(self.fixture.start["path"]), self.output
                )
        self.assertEqual(len(self.resources.cleanup_calls), 1)
        self.assertFalse(self.output.exists())

    def test_ordinary_receipt_cannot_claim_unknown_cleanup_without_recovery_intent(self):
        f = self.fixture
        with self.context():
            runtime.execute_source_verification(f.backend, Path(f.start["path"]), self.output, popen=f.fake_popen)
            cleanup_path = self.output.parent / "cache-cleanup.json"
            cleanup = read_json(cleanup_path)
            cleanup.update(deleted=None, execution="reconciled_unknown_exec_result")
            cleanup_path.write_text(json.dumps(cleanup), encoding="utf-8")
            receipt = read_json(self.output)
            with self.assertRaises(ValueError):
                producer._runtime_policy("remote-writes", cleanup=cleanup, recovered=False)
            # 显式构造攻击输入，继续验证最终 validator，不能靠收据构造器提前拒绝替代。
            writes = producer._runtime_policy("remote-writes", cleanup=cleanup, recovered=True)
            writes["authorization_cache"].update(
                unknown_result_commands=0, phase="initial_verification",
                historical_cleanup="not_applicable",
            )
            receipt.update(cache_cleanup=binding(cleanup_path), remote_writes=writes)
            self.output.write_text(json.dumps(receipt), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "副作用|原单次|原尝试|原.*绑定"):
                runtime.verify_source_runtime(f.backend, binding(self.output), live=False)


if __name__ == "__main__":
    unittest.main()
