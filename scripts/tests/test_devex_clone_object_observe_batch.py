"""只读对象批次的计划顺序、失败关闭与父控制器账本边界；不访问服务。"""
import copy
from contextlib import contextmanager, nullcontext
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from devex_clone import verify_plan, write_plan
from devex_clone_resume import reconcile_steps, resume_steps
from devex_clone_factory import CloneSession
from process_environment import Environments
from devex_clone_export_verify import verify_export_bindings, verify_source_export
from devex_clone_source import export_source
from devex_clone_source_fixture import SourceFixture
from restore_build import file_digest
import test_devex_clone_transfer as fixtures


class ObjectReadBatchTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.TransferTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        value = self.case.fixture.value
        bucket = next(bucket for bucket in value["objects"] if bucket["entries"])
        template = bucket["entries"][0]
        bucket["entries"] += [{**copy.deepcopy(template), "key": f"clone-source/system/extra-{index}.txt"} for index in range(1, 9)]
        source = self.case.backend / ".local-tests/batch-input.json"
        source.write_text(json.dumps(value), encoding="utf-8")
        self.case.plan_file = self.case.backend / ".local-tests/batch-plan.json"
        write_plan(self.case.backend, str(source), str(self.case.plan_file))
        self.case.plan = verify_plan(self.case.backend, str(self.case.plan_file))
        self.case.guard = fixtures.OfflineGuard(self.case)
        self.owner = threading.get_ident()

    def intent(self, engine, index=0):
        step, item = list(engine.objects.items())[index]
        before, after = engine._images(step, item)
        self.case.ledger.intent(step, before, after, item["artifact"]["sha256"])
        return step, item

    @contextmanager
    def reconciler(self, intents=1):
        with self.case.open():
            engine = self.case.engine()
            for index in range(intents):
                step, item = self.intent(engine, index)
                if index < intents - 1:
                    self.stored(item)
                    self.case.ledger.confirm(step, engine._images(step, item)[1])
        with self.case.open(mode="reconcile", create=False):
            yield self.case.engine()

    def stored(self, item):
        body = (self.case.fixture.root / item["artifact"]["file"]).read_bytes()
        self.case.runner.objects[item["bucket"], item["target_key"]] = body, copy.deepcopy(item["metadata"])

    def test_reconcile_batches_mixed_known_and_pending_objects_in_plan_order_without_writes(self):
        with self.reconciler(intents=2) as engine:
            (first, item), (second, _) = list(engine.objects.items())[:2]
            self.stored(item)
            batches, checked, reconciled = [], [], []
            observe, check, reconcile = self.case.guard.observe_objects, engine._check, self.case.ledger.reconcile

            def reads(targets):
                self.assertEqual(threading.get_ident(), self.owner)
                snapshot = self.case.ledger.snapshot()
                batches.append(targets)
                result = observe(targets)
                self.assertEqual(self.case.ledger.snapshot(), snapshot)
                return result

            def checked_generation(*args, **kwargs):
                checked.append(threading.get_ident())
                return check(*args, **kwargs)

            def record(step, image):
                self.assertEqual(threading.get_ident(), self.owner)
                reconciled.append(step)
                return reconcile(step, image)

            with patch.object(self.case.guard, "observe_objects", reads), patch.object(engine, "_check", checked_generation), \
                    patch.object(self.case.ledger, "reconcile", record):
                result = engine.check_objects("reconcile", list(engine.objects))
            self.assertEqual(result, {first: "confirmed", second: "reconciled_before"})
            self.assertEqual(reconciled, [first, second])
            self.assertEqual([len(batch) for batch in batches], [4, 4, 1])
            self.assertEqual(sum(batches, []), [(item["bucket"], item["target_key"]) for item in engine.objects.values()])
            self.assertEqual(checked, [self.owner] * 6)
            self.assertEqual(self.case.writes(), [])

    def test_pending_mismatch_prevents_every_reconciliation_in_the_batch(self):
        with self.reconciler() as engine:
            first, item = next(iter(engine.objects.items()))
            self.stored(item)
            self.stored(list(engine.objects.values())[1])
            with patch.object(self.case.ledger, "reconcile", wraps=self.case.ledger.reconcile) as recorded:
                with self.assertRaisesRegex(ValueError, "未登记写入"):
                    engine.check_objects("reconcile", list(engine.objects))
            self.assertEqual(recorded.call_count, 0)
            self.assertEqual(self.case.ledger.snapshot()["steps"][first]["phase"], "unknown")
            self.assertTrue(engine.failed)
            self.assertEqual(self.case.writes(), [])

    def test_changed_generation_payload_or_ledger_cannot_register_batch_results(self):
        for change in ("generation", "payload", "ledger"):
            with self.subTest(change=change):
                case = ObjectReadBatchTests()
                case.setUp()
                try:
                    closing = self.assertRaisesRegex(ExceptionGroup, "账本未完成") if change == "ledger" else nullcontext()
                    with closing, case.reconciler() as engine:
                        first, item = next(iter(engine.objects.items()))
                        observe = case.case.guard.observe_objects

                        def changed(targets):
                            result = observe(targets)
                            if change == "generation":
                                case.case.guard.change_generation = True
                            elif change == "payload":
                                (case.case.fixture.root / item["artifact"]["file"]).write_bytes(b"changed")
                            else:
                                path = case.case.ledger.path("head.json")
                                path.write_bytes(path.read_bytes() + b" ")
                            return result

                        with patch.object(case.case.guard, "observe_objects", changed), \
                                patch.object(case.case.ledger, "reconcile", wraps=case.case.ledger.reconcile) as recorded:
                            with self.assertRaises(ValueError):
                                engine.check_objects("reconcile", list(engine.objects))
                        self.assertEqual(recorded.call_count, 0)
                        self.assertEqual(case.case.ledger.snapshot()["steps"][first]["phase"], "unknown")
                        self.assertTrue(engine.failed)
                        self.assertEqual(case.case.writes(), [])
                finally:
                    case.doCleanups()

    def test_missing_reordered_and_duplicate_observations_cannot_be_classified(self):
        for change in ("missing", "reordered", "duplicate"):
            with self.subTest(change=change):
                case = ObjectReadBatchTests()
                case.setUp()
                try:
                    with case.reconciler() as engine:
                        observe = case.case.guard.observe_objects

                        def invalid(targets):
                            result = observe(targets)
                            return result[:-1] if change == "missing" else list(reversed(result)) if change == "reordered" else [result[0]] * len(result)

                        with patch.object(case.case.guard, "observe_objects", invalid), patch.object(case.case.ledger, "reconcile") as record:
                            with self.assertRaises(ValueError):
                                engine.check_objects("reconcile", list(engine.objects))
                        self.assertEqual(record.call_count, 0)
                finally:
                    case.doCleanups()

    def test_finish_reobserves_every_object_in_batches_without_replaying_writes(self):
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            writes = len(self.case.writes())
            with patch.object(self.case.guard, "observe_objects", wraps=self.case.guard.observe_objects) as observe:
                engine.finish()
            self.assertEqual([len(call.args[0]) for call in observe.call_args_list], [4, 4, 1])
            self.assertEqual(len(self.case.writes()), writes)
        self.assertEqual(engine.result()["status"], "data_steps_verified")

    def test_selected_raw_proof_changed_at_post_generation_is_rejected_before_reconcile(self):
        source = SourceFixture(self)
        export_source(source.backend, source.request_path, source.output, source)
        binding = {"path": str(source.output / "export.json"), **file_digest(source.output / "export.json")}
        verified = verify_source_export(source.backend, binding)
        diagnostic = next(path for path in verified["proof_files"] if path.endswith("/get.diagnostic.json") or path.endswith("\\get.diagnostic.json"))
        with self.reconciler() as engine:
            first, item = next(iter(engine.objects.items()))
            session = CloneSession.__new__(CloneSession)
            session.backend, session.plan, session.source_verified = source.backend, engine.plan, verified
            session.environment = Environments(dict(source.environment), dict(source.environment))
            selection = item["bucket"], item["source_key"]
            verify_export_bindings(source.backend, verified, selection=selection)
            check, calls = engine._check, 0

            def changed(*args, **kwargs):
                nonlocal calls
                calls += 1
                if calls == 2:
                    path = Path(diagnostic)
                    path.write_bytes(path.read_bytes() + b" ")
                    verify_export_bindings(source.backend, verified, selection="global")
                return check(*args, **kwargs)

            with patch.object(engine, "_check", changed), \
                    patch.object(self.case.guard, "verify_object_bindings", session.verify_object_bindings), \
                    patch.object(self.case.ledger, "reconcile", wraps=self.case.ledger.reconcile) as recorded:
                with self.assertRaisesRegex(ValueError, "原始小型证据"):
                    engine.check_objects("reconcile", [first])
            self.assertEqual(calls, 2)
            self.assertEqual(recorded.call_count, 0)
            self.assertEqual(self.case.ledger.snapshot()["steps"][first]["phase"], "unknown")
            self.assertEqual(self.case.writes(), [])

    def test_resume_unknown_stops_before_batch_and_reconcile_includes_pending_objects(self):
        with self.reconciler() as engine:
            first = next(iter(engine.objects))
            with patch.object(self.case.guard, "observe_objects", wraps=self.case.guard.observe_objects) as observe:
                with self.assertRaisesRegex(ValueError, "未知"):
                    resume_steps(engine)
                self.assertEqual(observe.call_count, 0)
                result = reconcile_steps(engine)
            self.assertEqual(result["steps"], {first: "reconciled_before"})
            self.assertEqual(len(result["pending_steps"]), 10)
            self.assertEqual([len(call.args[0]) for call in observe.call_args_list], [4, 4, 1])
            self.assertEqual(self.case.writes(), [])


if __name__ == "__main__":
    unittest.main()
