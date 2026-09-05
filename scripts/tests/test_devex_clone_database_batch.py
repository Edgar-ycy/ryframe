"""数据库只读批次使用内存远端替身及真实本地计划/账本；不连接服务。"""
import copy
from dataclasses import replace
import unittest
from unittest.mock import patch

import devex_clone_factory as factory
import devex_clone_resume as resume
from devex_clone_ledger import LedgerError
import test_devex_clone_transfer as fixtures
import test_devex_clone_factory as factory_fixtures


class DatabaseBatchTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.TransferTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)

    def pending(self, change=None):
        with self.case.open():
            engine = self.case.engine()
            original = self.case.guard.observe_databases
            def observe():
                result = original()
                if change:
                    change(engine, result)
                return result
            with patch.object(self.case.guard, "observe_databases", side_effect=observe) as vector, \
                    patch.object(self.case.guard, "observe_database", side_effect=AssertionError("不应逐库重复采集")):
                result = engine.check_databases("reconcile", list(engine.databases))
            return engine, result, vector.call_count

    def intents(self):
        with self.case.open():
            engine = self.case.engine()
            for index, (step, item) in enumerate(engine.databases.items()):
                before, after = engine._images(step, item)
                self.case.ledger.intent(step, before, after, item["artifact"]["sha256"])
                if index == 0:
                    self.change_rows(item["key"], source=True)
                    self.case.ledger.confirm(step, after)

    def change_rows(self, key, *, source=False):
        guard = self.case.guard
        tables = copy.deepcopy(guard.basis[key].source.tables if source else guard.current[key].tables)
        if not source:
            tables[next(iter(tables))]["sha256"] = "f" * 64
        guard.current[key] = replace(guard.current[key], tables=tables)

    def test_pending_all_databases_use_one_vector_and_no_writes(self):
        engine, result, count = self.pending()
        self.assertEqual((result, count), ({}, 1))
        self.assertEqual(len(self.case.guard.proof_selections), 3)  # 构造及批前后。
        self.assertEqual(self.case.writes(), [])
        self.assertFalse(engine.failed)

    def test_missing_database_rejected(self):
        with self.assertRaisesRegex(ValueError, "缺少或包含额外"):
            self.pending(lambda _engine, result: result.pop(next(iter(result))))

    def test_extra_database_rejected(self):
        with self.assertRaisesRegex(ValueError, "缺少或包含额外"):
            self.pending(lambda _engine, result: result.update(extra=next(iter(result.values()))))

    def test_wrong_database_identity_rejected(self):
        def change(_engine, result):
            keys = list(result)
            result[keys[0]] = result[keys[1]]
        with self.assertRaisesRegex(ValueError, "物理身份"):
            self.pending(change)

    def test_generation_change_after_observation_blocks_classification(self):
        with self.assertRaisesRegex(ValueError, "真实来源|初始化代次"):
            self.pending(lambda _engine, _result: setattr(self.case.guard, "change_generation", True))

    def test_plan_input_and_payload_changes_after_read_are_rejected(self):
        for field in ("plan", "input", "payload"):
            with self.subTest(field=field):
                case = fixtures.TransferTests()
                case.setUp()
                try:
                    with case.open():
                        engine = case.engine()
                        original = case.guard.observe_databases
                        def observe():
                            paths = {"plan": engine.plan_file, "input": engine.input_file,
                                     "payload": engine.root / next(iter(engine.databases.values()))["artifact"]["file"]}
                            path = paths[field]
                            path.write_bytes(path.read_bytes() + b"\n")
                            return original()
                        with patch.object(case.guard, "observe_databases", side_effect=observe), \
                                patch.object(case.ledger, "reconcile") as classify, self.assertRaises(ValueError):
                            engine.check_databases("reconcile", list(engine.databases))
                        classify.assert_not_called()
                        self.assertEqual(case.writes(), [])
                finally:
                    case.doCleanups()

    def test_raw_observation_changed_during_final_guard_is_rejected(self):
        with self.case.open():
            engine = self.case.engine()
            result = self.case.guard.observe_databases()
            original = self.case.guard.generation
            calls = 0
            def generation(*args, **kwargs):
                nonlocal calls
                calls += 1
                if calls == 2:
                    value = next(iter(result.values()))
                    value.tables[next(iter(value.tables))]["sha256"] = "f" * 64
                return original(*args, **kwargs)
            with patch.object(self.case.guard, "observe_databases", return_value=result), \
                    patch.object(self.case.guard, "generation", side_effect=generation), \
                    patch.object(self.case.ledger, "reconcile") as classify, \
                    self.assertRaisesRegex(ValueError, "观察或账本变化"):
                engine.check_databases("reconcile", list(engine.databases))
            classify.assert_not_called()

    def test_original_database_basis_change_after_read_rejected(self):
        def change(_engine, _result):
            key = next(iter(self.case.guard.basis))
            basis = self.case.guard.basis[key]
            source = copy.deepcopy(basis.source)
            source.tables[next(iter(source.tables))]["sha256"] = "f" * 64
            self.case.guard.basis[key] = replace(basis, source=source)
        with self.assertRaisesRegex(ValueError, "原像|前像"):
            self.pending(change)

    def test_current_ledger_bytes_change_after_read_rejected(self):
        with self.case.open():
            engine = self.case.engine()
            path = engine.ledger.path("head.json")
            before = path.read_bytes()
            original = self.case.guard.observe_databases
            def observe():
                path.write_bytes(before + b"\n")
                return original()
            try:
                with patch.object(self.case.guard, "observe_databases", side_effect=observe), \
                        patch.object(self.case.ledger, "reconcile") as classify, self.assertRaises(LedgerError):
                    engine.check_databases("reconcile", list(engine.databases))
                classify.assert_not_called()
            finally:
                path.write_bytes(before)  # 只还原本测试故意破坏的临时账本，以检查正常释放。

    def test_unknown_before_and_after_classified_without_replay(self):
        self.intents()
        first = self.case.plan["databases"][0]["key"]
        self.change_rows(first, source=True)
        with self.case.open(create=False, mode="reconcile"):
            engine = self.case.engine()
            result = engine.check_databases("reconcile", list(engine.databases))
        self.assertEqual(list(result.values()), ["confirmed", "reconciled_before"])
        self.assertEqual(self.case.writes(), [])

    def test_unknown_mismatch_is_preserved_and_never_adopted(self):
        self.intents()
        self.change_rows(self.case.plan["databases"][1]["key"])
        with self.case.open(create=False, mode="reconcile"):
            engine = self.case.engine()
            result = engine.check_databases("reconcile", list(engine.databases))
        self.assertEqual(list(result.values()), ["confirmed", "mismatch"])
        self.assertEqual(self.case.writes(), [])

    def test_unknown_after_image_is_confirmed_without_repeating_write(self):
        self.intents()
        self.change_rows(self.case.plan["databases"][1]["key"], source=True)
        with self.case.open(create=False, mode="reconcile"):
            engine = self.case.engine()
            result = engine.check_databases("reconcile", list(engine.databases))
        self.assertEqual(list(result.values()), ["confirmed", "confirmed"])
        self.assertEqual(self.case.writes(), [])

    def test_bad_unregistered_image_prevents_any_other_classification(self):
        with self.case.open():
            engine = self.case.engine()
            step, item = next(iter(engine.databases.items()))
            before, after = engine._images(step, item)
            self.case.ledger.intent(step, before, after, item["artifact"]["sha256"])
        self.change_rows(self.case.plan["databases"][1]["key"])
        with self.case.open(create=False, mode="reconcile"):
            engine = self.case.engine()
            with patch.object(self.case.ledger, "reconcile") as classify, self.assertRaisesRegex(ValueError, "未登记写入"):
                engine.check_databases("reconcile", list(engine.databases))
            classify.assert_not_called()

    def test_confirmed_subset_reads_full_vector_without_touching_other_intents(self):
        first = self.case.plan["databases"][0]
        with self.case.open():
            engine = self.case.engine()
            engine.apply_database(first["key"])
        with self.case.open(create=False, mode="resume"):
            engine = self.case.engine()
            before = self.case.ledger.snapshot()
            with patch.object(self.case.guard, "observe_databases", wraps=self.case.guard.observe_databases) as vector:
                self.assertEqual(engine.check_databases("confirmed", [engine.db_step(first)]), {})
            self.assertEqual(vector.call_count, 1)
            self.assertEqual(self.case.ledger.snapshot(), before)
        self.assertEqual(len(self.case.writes()), 1)

    def test_invalid_scope_and_empty_confirmed_never_observe(self):
        with self.case.open(), patch.object(self.case.guard, "observe_databases") as vector:
            for mode, selection in (("apply", []), ("reconcile", []), ("confirmed", ["outside"]),
                                    ("reconcile", ["duplicate", "duplicate"])):
                with self.subTest(mode=mode, selection=selection), self.assertRaises(ValueError):
                    self.case.engine().check_databases(mode, selection)
            self.assertEqual(self.case.engine().check_databases("confirmed", []), {})
            vector.assert_not_called()

    def test_database_writes_keep_independent_observations_before_final_read_batch(self):
        with self.case.open(), patch.object(self.case.guard, "observe_database", wraps=self.case.guard.observe_database) as observe:
            engine = self.case.engine()
            with patch.object(self.case.guard, "observe_databases", side_effect=AssertionError("写入不能使用批次快照")):
                self.case.apply_all(engine)
            with patch.object(self.case.guard, "observe_databases", wraps=self.case.guard.observe_databases) as vector:
                engine.finish()
                self.assertEqual(vector.call_count, 1)
        expected = [item["key"] for item in self.case.plan["databases"] for _ in range(2)]
        self.assertEqual([call.args[0] for call in observe.call_args_list], expected)
        self.assertEqual(len(self.case.writes()), 3)


class DatabaseFactoryTests(unittest.TestCase):
    def test_complete_source_and_target_vector_are_recaptured_each_time(self):
        session = factory.CloneSession.__new__(factory.CloneSession)
        session.initialized = {"a": {}, "b": {}}
        source, target = {"a": {}, "b": {}}, {"a": {"value": 1}, "b": {"value": 2}}
        with patch.object(session, "_source_inventory", return_value=source) as left, \
                patch.object(session, "_inventory", return_value=target) as right:
            first = session.observe_databases()
            target["a"]["value"] = 3
            second = session.observe_databases()
        self.assertEqual((left.call_count, right.call_count), (2, 2))
        self.assertEqual([call.args for call in right.call_args_list], [("target",), ("target",)])
        self.assertEqual((first["a"]["value"], second["a"]["value"]), (1, 3))

    def test_factory_rejects_source_or_target_topology_drift(self):
        session = factory.CloneSession.__new__(factory.CloneSession)
        session.initialized = {"a": {}, "b": {}}
        for side in ("source", "target"):
            for value in ({"a": {}}, {"a": {}, "b": {}, "extra": {}}):
                with self.subTest(side=side, value=value), \
                        patch.object(session, "_source_inventory", return_value=value if side == "source" else session.initialized), \
                        patch.object(session, "_inventory", return_value=value if side == "target" else session.initialized), \
                        self.assertRaisesRegex(ValueError, "完整覆盖"):
                    session.observe_databases()

    def test_reconcile_actual_factory_consumes_one_batch_after_hydration(self):
        case = factory_fixtures.FactoryTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        with patch.object(factory, "TransferSteps", side_effect=RuntimeError("fixture prewrite")), self.assertRaises(RuntimeError):
            case.execute()
        inventory = case.mocks[5]
        inventory.reset_mock()
        result = resume.continue_copy(case.backend, case.output, case.environments["source"],
                                      case.environments["target"], mode="reconcile", run=case.case.runner)
        self.assertEqual([call.args[1] for call in inventory.call_args_list], ["source", "source", "target"])
        self.assertEqual(result["remote_writes"], 0)
        self.assertEqual(case.case.writes(), [])


if __name__ == "__main__":
    unittest.main()
