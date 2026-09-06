import hashlib
import json
import os
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from devex_clone_ledger import CloneLedger, LedgerError, encoded, image_pair, resource_digest

ROOT = Path(__file__).resolve().parents[2]


def sha(value):
    return hashlib.sha256(value.encode()).hexdigest()


class CloneLedgerTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        (self.backend / ".local-tests").mkdir()
        self.root = self.backend / ".local-tests/clone-ledger"
        self.plan, self.generation = sha("plan"), sha("generation")
        self.database = {"kind": "database", "scope_id": "perf-seed-fixture",
                         "server_uuid": "67cd8d4a-fe90-11ef-98fd-5847ca786499", "database": "fixture_control"}
        self.before = {"kind": "database", "resource_sha256": resource_digest(self.generation, self.database),
                       "schema_sha256": sha("schema"), "tables_sha256": sha("initial rows"), "preserved_sha256": sha("owner and excluded")}
        self.after = {**self.before, "tables_sha256": sha("complete copied rows")}
        self.object_resource = {"kind": "object", "scope_id": "perf-seed-fixture", "endpoint": "http://127.0.0.1:29000",
                                "bucket": "uploads", "key": "perf-seed-fixture/system/exact.bin"}
        self.absent = {"kind": "object", "resource_sha256": resource_digest(self.generation, self.object_resource),
                       "exists": False, "bytes": None, "sha256": None, "metadata_sha256": None}
        self.present = {**self.absent, "exists": True, "bytes": 42, "sha256": sha("object bytes"), "metadata_sha256": sha("full headers")}

    def ledger(self, **options):
        return CloneLedger(self.backend, self.root, self.plan, self.generation, **options)

    def interrupted(self, before=None, after=None):
        before, after = before or self.before, after or self.after
        with self.assertRaises(ConnectionError):
            with self.ledger(create=True) as ledger:
                ledger.intent("copy-one", before, after, sha("artifact"))
                raise ConnectionError("离线模拟响应丢失；不是实际复制")
        return ledger

    def test_intent_is_durable_before_caller_can_write_and_finish_is_evidence_only(self):
        with self.ledger(create=True) as ledger:
            ledger.intent("copy-one", self.before, self.after, sha("artifact"))
            self.assertEqual(self.ledger().inspect()["steps"]["copy-one"]["phase"], "unknown")
            ledger.confirm("copy-one", self.after)
            ledger.finish()
            self.assertIsNone(ledger.receipt)
            self.assertTrue((self.root / "clone.lock").exists())
        self.assertEqual(ledger.receipt["status"], "ledger_evidence_complete")
        for field in ("target_ready", "restore_success", "runtime_performance_passed"):
            self.assertFalse(ledger.receipt[field])
        first = self.ledger().inspect()
        self.assertEqual(first["completed_at"], self.ledger().inspect()["completed_at"])

    def test_commit_response_lost_requires_explicit_complete_postimage_reconciliation(self):
        self.interrupted()
        with self.assertRaisesRegex(LedgerError, "未知结果"):
            with self.ledger():
                self.fail("未知提交不允许普通执行重放")
        with self.ledger(mode="reconcile") as ledger:
            self.assertEqual(ledger.reconcile("copy-one", self.after), "confirmed")
            ledger.finish()
        self.assertEqual(self.ledger().inspect()["status"], "ledger_evidence_complete")

    def test_object_response_lost_needs_all_bytes_and_metadata_not_only_existing_key(self):
        self.interrupted(self.absent, self.present)
        with self.ledger(mode="reconcile") as ledger:
            wrong = {**self.present, "metadata_sha256": sha("changed header")}
            self.assertEqual(ledger.reconcile("copy-one", wrong), "mismatch")
            with self.assertRaisesRegex(LedgerError, "未确认"):
                ledger.finish()
        self.assertEqual(self.ledger().inspect()["status"], "needs_reconciliation")
        with self.ledger(mode="reconcile") as ledger:
            self.assertEqual(ledger.reconcile("copy-one", self.present), "confirmed")
            ledger.finish()

    def test_before_image_is_not_automatic_retry_and_resume_needs_a_new_observation(self):
        self.interrupted()
        with self.ledger(mode="reconcile") as ledger:
            self.assertEqual(ledger.reconcile("copy-one", self.before), "reconciled_before")
        self.assertEqual(self.ledger().inspect()["steps"]["copy-one"]["attempt"], 1)
        with self.ledger(mode="resume") as ledger:
            ledger.resume("copy-one", self.before)
            self.assertEqual(ledger.snapshot()["steps"]["copy-one"]["attempt"], 2)
            ledger.confirm("copy-one", self.after)
            ledger.finish()

    def test_resume_cannot_skip_reconciliation_or_use_wrong_image(self):
        self.interrupted()
        with self.assertRaises(LedgerError):
            with self.ledger(mode="resume"):
                self.fail("未核对不能续跑")
        with self.ledger(mode="reconcile") as ledger:
            ledger.reconcile("copy-one", self.before)
        with self.assertRaisesRegex(LedgerError, "完整前像"):
            with self.ledger(mode="resume") as ledger:
                ledger.resume("copy-one", self.after)

    def test_partial_completion_keeps_confirmed_steps_without_repeating_them(self):
        with self.assertRaises(ConnectionError):
            with self.ledger(create=True) as ledger:
                ledger.intent("copy-db", self.before, self.after, sha("sql"))
                ledger.confirm("copy-db", self.after)
                ledger.intent("copy-object", self.absent, self.present, sha("file"))
                raise ConnectionError("离线模拟第二个步骤响应丢失")
        state = self.ledger().inspect()
        self.assertEqual(state["steps"]["copy-db"]["phase"], "confirmed")
        self.assertEqual(state["steps"]["copy-object"]["phase"], "unknown")
        with self.ledger(mode="reconcile") as ledger:
            ledger.reconcile("copy-object", self.present)
            with self.assertRaisesRegex(LedgerError, "全部"):
                ledger.finish()
            ledger.reconcile("copy-db", self.after)
            ledger.finish()

    def test_lock_is_exclusive_and_failed_contender_does_not_release_it(self):
        with self.ledger(create=True):
            lock = self.root / "clone.lock"
            identity = lock.stat().st_ino
            with self.assertRaisesRegex(LedgerError, "锁定"):
                with self.ledger(mode="reconcile"):
                    self.fail("不能并发取得锁")
            self.assertEqual(lock.stat().st_ino, identity)

    def test_lock_release_failure_prevents_completion_and_keeps_the_lock(self):
        ledger = self.ledger(create=True)
        with self.assertRaises(ExceptionGroup) as caught:
            with ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.confirm("copy-one", self.after)
                ledger.finish()
                self.release = patch.object(ledger, "_release", side_effect=OSError("lock release fixture"))
                self.release.start()
        self.release.stop()
        self.assertTrue((self.root / "clone.lock").exists())
        self.assertIsNone(ledger.receipt)
        self.assertFalse(list(self.root.glob("complete-*.json")))
        self.assertIn("lock release fixture", str(caught.exception.exceptions[0]))
        self.assertEqual(self.ledger().inspect()["status"], "needs_reconciliation")
        ledger._release()

    def test_original_operation_error_is_retained_alongside_release_failure(self):
        ledger = self.ledger(create=True)
        with self.assertRaises(ExceptionGroup) as caught:
            with ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                self.release = patch.object(ledger, "_release", side_effect=OSError("release fixture"))
                self.release.start()
                raise ConnectionError("commit fixture")
        self.release.stop()
        self.assertIsInstance(caught.exception.exceptions[0], ConnectionError)
        self.assertIsInstance(caught.exception.exceptions[1], OSError)
        ledger._release()

    def test_complete_receipt_write_failure_leaves_unknown_and_no_success_receipt(self):
        ledger = self.ledger(create=True)
        original = ledger._write
        def fail_receipt(path, value, **options):
            if path.name.startswith("complete-"):
                raise OSError("receipt disk fixture")
            original(path, value, **options)
        with patch.object(ledger, "_write", side_effect=fail_receipt), self.assertRaises(ExceptionGroup):
            with ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.confirm("copy-one", self.after)
                ledger.finish()
        self.assertFalse(list(self.root.glob("complete-*.json")))
        self.assertEqual(self.ledger().inspect()["status"], "needs_reconciliation")

    def test_confirm_head_write_failure_is_not_automatically_repaired(self):
        ledger = self.ledger(create=True)
        with self.assertRaises(OSError):
            with ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                with patch.object(ledger, "_write", side_effect=OSError("head disk fixture")):
                    ledger.confirm("copy-one", self.after)
        original = {path.name: path.read_bytes() for path in self.root.iterdir() if path.is_file()}
        with self.assertRaisesRegex(LedgerError, "head 不匹配"):
            with self.ledger(mode="reconcile"):
                self.fail("不能自动截断或补写")
        self.assertEqual(original, {path.name: path.read_bytes() for path in self.root.iterdir() if path.is_file()})

    def test_intent_fsync_failure_never_reaches_the_callers_resource_write(self):
        called = False
        with self.assertRaises(OSError):
            with self.ledger(create=True) as ledger:
                with patch("devex_clone_ledger.os.fsync", side_effect=OSError("fsync fixture")):
                    ledger.intent("copy-one", self.before, self.after, sha("sql"))
                called = True
        self.assertFalse(called)
        self.assertFalse(list(self.root.glob("complete-*.json")))
        with self.assertRaisesRegex(LedgerError, "head 不匹配"):
            self.ledger().inspect()

    def test_receipt_fsync_failure_leaves_only_unpublished_temp_and_unknown(self):
        ledger = self.ledger(create=True)
        original = ledger._write
        def fail_fsync(path, value, **options):
            if path.name.startswith("complete-"):
                with patch("devex_clone_ledger.os.fsync", side_effect=OSError("receipt fsync fixture")):
                    original(path, value, **options)
            else:
                original(path, value, **options)
        with patch.object(ledger, "_write", side_effect=fail_fsync), self.assertRaises(ExceptionGroup):
            with ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.confirm("copy-one", self.after)
                ledger.finish()
        self.assertFalse(list(self.root.glob("complete-*.json")))
        self.assertTrue(list(self.root.glob("complete-*.tmp")))
        self.assertEqual(self.ledger().inspect()["status"], "needs_reconciliation")

    def test_unconfirmed_step_blocks_the_next_write_intent(self):
        with self.assertRaisesRegex(LedgerError, "下一资源写入"):
            with self.ledger(create=True) as ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.intent("copy-two", self.absent, self.present, sha("object"))
        self.assertEqual(set(self.ledger().inspect()["steps"]), {"copy-one"})

    def test_append_before_head_crash_and_truncation_are_rejected_without_rewrite(self):
        self.interrupted()
        path = self.root / "events.ndjson"
        original = path.read_bytes()
        variants = [original[:-1], b"\n".join(original.splitlines()[:-1]) + b"\n", original.replace(b'"unknown"', b'"changed"')]
        # 有效字段字节被替换后摘要链仍应拒绝。
        variants[-1] = original.replace(self.after["tables_sha256"].encode(), sha("tampered").encode())
        for raw in variants:
            with self.subTest():
                path.write_bytes(raw)
                with self.assertRaises(LedgerError):
                    self.ledger().inspect()
                self.assertEqual(path.read_bytes(), raw)
        path.write_bytes(original)

    def test_wrong_plan_generation_or_forged_completion_flags_are_rejected(self):
        with self.ledger(create=True) as ledger:
            ledger.intent("copy-one", self.before, self.after, sha("sql"))
            ledger.confirm("copy-one", self.after)
            ledger.finish()
        for plan, generation in ((sha("other-plan"), self.generation), (self.plan, sha("other-generation"))):
            with self.assertRaisesRegex(LedgerError, "代次"):
                CloneLedger(self.backend, self.root, plan, generation).inspect()
        receipt = next(self.root.glob("complete-*.json"))
        value = json.loads(receipt.read_bytes())
        value["target_ready"] = True
        receipt.write_bytes(encoded(value))
        with self.assertRaisesRegex(LedgerError, "完成收据"):
            self.ledger().inspect()

    def test_unknown_or_duplicate_events_cannot_be_ignored(self):
        for event in ({"type": "unknown-future-event"}, {"type": "confirmed", "step_id": "unowned-object", "observed": self.present}):
            target = self.root.with_name(self.root.name + sha(json.dumps(event))[:8])
            with self.assertRaises(ValueError):
                with CloneLedger(self.backend, target, self.plan, self.generation, create=True) as ledger:
                    ledger._append(event)
        with self.assertRaisesRegex(LedgerError, "重复步骤"):
            with self.ledger(create=True) as ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.intent("copy-one", self.before, self.after, sha("sql"))

    def test_repeated_confirm_or_reconcile_requires_a_new_explicit_session(self):
        with self.assertRaisesRegex(LedgerError, "确认必须"):
            with self.ledger(create=True) as ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.confirm("copy-one", self.after)
                ledger.confirm("copy-one", self.after)
        with self.assertRaisesRegex(LedgerError, "重复消费"):
            with self.ledger(mode="reconcile") as ledger:
                ledger.reconcile("copy-one", self.after)
                ledger.reconcile("copy-one", self.after)

    def test_resource_digest_binds_generation_scope_uuid_and_exact_object_key(self):
        baseline = resource_digest(self.generation, self.database)
        self.assertNotEqual(baseline, resource_digest(sha("next-generation"), self.database))
        for field, value in (("scope_id", "another-scope"), ("database", "another_schema"),
                             ("server_uuid", "77cd8d4a-fe90-11ef-98fd-5847ca786499")):
            self.assertNotEqual(baseline, resource_digest(self.generation, {**self.database, field: value}))
        with self.assertRaises(LedgerError):
            resource_digest(self.generation, {**self.object_resource, "key": "reference-source/exact.bin"})
        with self.assertRaises(LedgerError):
            resource_digest(self.generation, {**self.object_resource, "key": "perf-seed-fixture/.ryframe-owner"})

    def test_preserved_tables_schema_and_ambiguous_equal_images_cannot_be_changed(self):
        for after in (self.before, {**self.after, "schema_sha256": sha("other schema")},
                      {**self.after, "preserved_sha256": sha("owner changed")}, self.absent):
            with self.assertRaises(ValueError):
                image_pair(self.before, after)
        with self.assertRaises(LedgerError):
            image_pair(self.present, {**self.present, "sha256": sha("overwrite")})

    def test_path_outside_ignored_workspace_is_rejected(self):
        with self.assertRaises(ValueError):
            CloneLedger(self.backend, self.backend / "outside", self.plan, self.generation, create=True)

    def test_event_capacity_is_checked_before_the_intent_is_written(self):
        with self.assertRaisesRegex(LedgerError, "账本上限"):
            with self.ledger(create=True) as ledger:
                original = (self.root / "events.ndjson").read_bytes()
                with patch("devex_clone_ledger.MAX_BYTES", len(original) + 1):
                    ledger.intent("copy-one", self.before, self.after, sha("sql"))
        self.assertEqual((self.root / "events.ndjson").read_bytes(), original)

    def two_reconciled_before_steps(self):
        with self.ledger(create=True) as ledger:
            ledger.intent("copy-db", self.before, self.after, sha("sql"))
            ledger.confirm("copy-db", self.after)
            ledger.intent("copy-object", self.absent, self.present, sha("object"))
            ledger.confirm("copy-object", self.present)
        with self.ledger(mode="reconcile") as ledger:
            ledger.reconcile("copy-db", self.before)
            ledger.reconcile("copy-object", self.absent)

    def test_review_resume_cannot_start_the_next_step_while_first_is_unknown(self):
        self.two_reconciled_before_steps()
        with self.assertRaisesRegex(LedgerError, "其他步骤"):
            with self.ledger(mode="resume") as ledger:
                ledger.resume("copy-db", self.before)
                ledger.resume("copy-object", self.absent)
        state = self.ledger().inspect()["steps"]
        self.assertEqual(state["copy-db"]["phase"], "unknown")
        self.assertEqual(state["copy-object"]["phase"], "reconciled_before")
        self.assertEqual(state["copy-object"]["attempt"], 1)

    def test_review_resume_can_process_before_images_in_order_after_confirmation(self):
        self.two_reconciled_before_steps()
        with self.ledger(mode="resume") as ledger:
            ledger.resume("copy-db", self.before)
            ledger.confirm("copy-db", self.after)
            ledger.resume("copy-object", self.absent)
            ledger.confirm("copy-object", self.present)
            ledger.finish()
        self.assertEqual(self.ledger().inspect()["status"], "ledger_evidence_complete")

    def test_review_resource_key_uses_the_same_utf8_limit_as_the_clone_model(self):
        prefix = self.object_resource["scope_id"] + "/"
        exact_key = prefix + "x" * (1024 - len(prefix.encode()))
        self.assertEqual(len(resource_digest(self.generation, {**self.object_resource, "key": exact_key})), 64)
        for key in (exact_key + "x", prefix + "汉" * 340):
            with self.subTest(bytes=len(key.encode())), self.assertRaises(LedgerError):
                resource_digest(self.generation, {**self.object_resource, "key": key})

    def test_review_exclusive_publication_never_overwrites_a_racing_receipt(self):
        ledger = self.ledger(create=True)
        original_link = os.link
        def racing_link(source, destination, **options):
            if Path(destination).name.startswith("complete-"):
                Path(destination).write_bytes(b"previous unrelated receipt")
            return original_link(source, destination, **options)
        with patch("devex_clone_ledger.os.link", side_effect=racing_link), self.assertRaises(ExceptionGroup):
            with ledger:
                ledger.intent("copy-one", self.before, self.after, sha("sql"))
                ledger.confirm("copy-one", self.after)
                ledger.finish()
        receipt = next(self.root.glob("complete-*.json"))
        self.assertEqual(receipt.read_bytes(), b"previous unrelated receipt")
        self.assertIsNone(ledger.receipt)
        self.assertEqual(self.ledger().inspect()["status"], "needs_reconciliation")


if __name__ == "__main__":
    unittest.main()
