"""最后只读数据库批次的离线回归；保留实际本地账本，不连接真实资源。"""
import copy
from dataclasses import replace
import unittest
from unittest.mock import patch

from devex_clone_ledger import LedgerError
import test_devex_clone_transfer as fixtures


class DatabaseFinishTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.TransferTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)

    def noop(self, key):
        basis = self.case.guard.basis[key]
        initial = replace(basis.initialized, tables=copy.deepcopy(basis.source.tables))
        self.case.guard.basis[key] = replace(basis, initialized=initial)
        self.case.guard.current[key] = initial

    def test_noop_and_confirmed_database_finish_together_without_rewrite(self):
        self.noop("dedicated-a")
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            expected_noops = copy.deepcopy(engine.noops)
            with patch.object(self.case.guard, "observe_database", side_effect=AssertionError("收尾不可逐库重采")), \
                    patch.object(self.case.guard, "observe_databases", wraps=self.case.guard.observe_databases) as vector:
                engine.finish()
            self.assertEqual(vector.call_count, 1)
        result = engine.result()
        self.assertEqual(result["unchanged_database_images"], expected_noops)
        self.assertEqual((result["written_steps"], result["unchanged_databases"]), (2, 1))
        self.assertEqual(len(self.case.writes()), 2)

    def test_all_databases_noop_keep_their_original_images(self):
        for key in self.case.guard.basis:
            self.noop(key)
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            before = copy.deepcopy(engine.noops)
            engine.finish()
        self.assertEqual(engine.result()["unchanged_database_images"], before)
        self.assertEqual(engine.result()["unchanged_databases"], 2)
        self.assertEqual(len(self.case.writes()), 1)  # 仅既有对象写入。

    def test_finish_requires_all_databases_in_plan_order(self):
        with self.case.open(), patch.object(self.case.guard, "observe_databases") as observe:
            engine = self.case.engine()
            keys = list(engine.databases)
            for selection in ([], keys[:1], keys[::-1], keys + keys[:1]):
                with self.subTest(selection=selection), self.assertRaisesRegex(ValueError, "按计划顺序"):
                    self.case.engine().check_databases("finish", selection)
            observe.assert_not_called()

    def test_missing_noop_mapping_is_not_inferred_from_equal_data(self):
        self.noop("dedicated-a")
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            engine.noops.clear()
            with patch.object(self.case.guard, "observe_databases") as observe, self.assertRaisesRegex(ValueError, "no-op"):
                engine.check_databases("finish", list(engine.databases))
            observe.assert_not_called()

    def test_noop_must_not_overlap_confirmed_intent(self):
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            step, item = next(iter(engine.databases.items()))
            engine.noops[step] = engine._images(step, item)[0]
            with patch.object(self.case.guard, "observe_databases") as observe, self.assertRaisesRegex(ValueError, "no-op"):
                engine.finish()
            observe.assert_not_called()

    def test_noop_image_must_match_the_original_equal_before_and_after(self):
        self.noop("dedicated-a")
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            engine.noops[next(iter(engine.noops))]["tables_sha256"] = "f" * 64
            with self.assertRaisesRegex(ValueError, "初始/来源像"):
                engine.finish()

    def test_matching_before_image_alone_cannot_fabricate_noop(self):
        with self.case.open():
            engine = self.case.engine()
            for step, item in engine.databases.items():
                before, after = engine._images(step, item)
                self.assertNotEqual(before, after)
                engine.noops[step] = before
            with patch.object(self.case.guard, "observe_databases") as observe, self.assertRaisesRegex(ValueError, "初始/来源像"):
                engine.check_databases("finish", list(engine.databases))
            observe.assert_not_called()

    def unknown_database(self, engine):
        items = list(engine.databases.items())
        engine.apply_database(items[0][1]["key"])
        for item in engine.objects.values():
            engine.apply_object(item["bucket"], item["target_key"])
        step, item = items[1]
        before, after = engine._images(step, item)
        self.case.ledger.intent(step, before, after, item["artifact"]["sha256"])
        return step, item

    def test_unknown_intent_is_not_confirmed_by_finish_even_when_data_matches(self):
        with self.case.open():
            engine = self.case.engine()
            step, item = self.unknown_database(engine)
            basis = self.case.guard.basis[item["key"]]
            self.case.guard.current[item["key"]] = replace(basis.initialized, tables=copy.deepcopy(basis.source.tables))
            with patch.object(self.case.guard, "observe_databases") as observe, \
                    patch.object(self.case.ledger, "finish") as finish, self.assertRaisesRegex(ValueError, "未知或未确认"):
                engine.finish()
            observe.assert_not_called()
            finish.assert_not_called()
            self.assertEqual(self.case.ledger.snapshot()["steps"][step]["phase"], "unknown")
        self.assertFalse(list(self.case.ledger_dir.glob("complete-*.json")))

    def test_reconciled_before_intent_cannot_finish_without_explicit_write(self):
        with self.case.open():
            step, _ = self.unknown_database(self.case.engine())
        with self.case.open(create=False, mode="reconcile"):
            self.assertEqual(self.case.engine().reconcile(step), "reconciled_before")
        with self.case.open(create=False, mode="resume"):
            engine = self.case.engine()
            with patch.object(self.case.guard, "observe_databases") as observe, self.assertRaisesRegex(ValueError, "未知或未确认"):
                engine.finish()
            observe.assert_not_called()
        self.assertEqual(len(self.case.writes()), 2)

    def failing_observation(self, mutate, message):
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            original = self.case.guard.observe_databases
            def observe():
                result = original()
                mutate(engine, result)
                return result
            with patch.object(self.case.guard, "observe_databases", side_effect=observe), \
                    patch.object(self.case.ledger, "finish") as finish, \
                    patch.object(self.case.guard, "verify_complete_snapshot") as complete, \
                    self.assertRaisesRegex(ValueError, message):
                engine.finish()
            finish.assert_not_called()
            complete.assert_not_called()
            self.assertFalse(engine.finished)
        self.assertEqual(len(self.case.writes()), 3)
        self.assertFalse(list(self.case.ledger_dir.glob("complete-*.json")))

    def test_missing_finish_vector_database_is_rejected(self):
        self.failing_observation(lambda _engine, value: value.pop(next(iter(value))), "缺少或包含额外")

    def test_finish_vector_ownership_change_is_rejected(self):
        def mutate(_engine, value):
            key = next(iter(value))
            value[key] = replace(value[key], ownership=())
        self.failing_observation(mutate, "ownership")

    def test_finish_postimage_drift_is_rejected(self):
        def mutate(_engine, value):
            item = next(iter(value.values()))
            item.tables[next(iter(item.tables))]["sha256"] = "f" * 64
        self.failing_observation(mutate, "后像不同")

    def test_finish_payload_change_after_vector_read_is_rejected(self):
        def mutate(engine, _value):
            path = engine.root / next(iter(engine.databases.values()))["artifact"]["file"]
            path.write_bytes(path.read_bytes() + b"\n")
        self.failing_observation(mutate, ".+")

    def test_finish_source_generation_change_after_vector_read_is_rejected(self):
        self.failing_observation(lambda _engine, _value: setattr(self.case.guard, "change_generation", True), "真实来源|初始化代次")

    def test_finish_ledger_drift_is_rejected_before_publish(self):
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            path = self.case.ledger.path("head.json")
            original_bytes = path.read_bytes()
            original = self.case.guard.observe_databases
            def observe():
                path.write_bytes(original_bytes + b"\n")
                return original()
            try:
                with patch.object(self.case.guard, "observe_databases", side_effect=observe), \
                        patch.object(self.case.ledger, "finish") as finish, self.assertRaises(LedgerError):
                    engine.finish()
                finish.assert_not_called()
            finally:
                path.write_bytes(original_bytes)  # 恢复测试注入的临时文件，核验正常释放。

    def test_finish_noop_mapping_drift_during_observation_is_rejected(self):
        self.noop("dedicated-a")
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            original = self.case.guard.observe_databases
            def observe():
                engine.noops.clear()
                return original()
            with patch.object(self.case.guard, "observe_databases", side_effect=observe), self.assertRaisesRegex(ValueError, "no-op 绑定变化"):
                engine.finish()

    def test_noop_mapping_cannot_change_after_database_batch_before_final_publish(self):
        self.noop("dedicated-a")
        with self.case.open():
            engine = self.case.engine()
            self.case.apply_all(engine)
            with patch.object(self.case.guard, "verify_complete_snapshot", side_effect=engine.noops.clear), \
                    patch.object(self.case.ledger, "finish") as finish, self.assertRaisesRegex(ValueError, "完整收尾期间 no-op"):
                engine.finish()
            finish.assert_not_called()


if __name__ == "__main__":
    unittest.main()
