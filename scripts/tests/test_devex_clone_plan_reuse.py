"""同进程语义结果复用的失败关闭检查；不执行真实资源命令。"""
import copy
from pathlib import Path
import pickle
import shutil
import unittest
from tests.workspace_directory import WorkspaceDirectory
from unittest.mock import patch

import devex_clone as clone
import test_devex_clone_transfer as fixtures
from devex_clone_transfer import TransferSteps


class PlanReuseTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.TransferTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.backend, self.path = self.case.backend, self.case.plan_file

    def issue(self):
        return clone.verify_plan_result(self.backend, str(self.path))

    def engine(self, token):
        return TransferSteps(self.backend, self.path, self.case.tools, self.case.ledger,
                             self.case.guard, verified_plan=token)

    def test_full_verification_then_constructor_semantically_parses_exactly_once(self):
        with patch.object(clone, "create_plan", wraps=clone.create_plan) as parse:
            token = self.issue()
            with self.case.open():
                engine = self.engine(token)
                self.assertEqual(engine.plan, token.plan)
            self.assertEqual(parse.call_count, 1)
        self.assertEqual(self.case.writes(), [])

    def test_plan_write_then_constructor_semantically_parses_exactly_once(self):
        output = self.backend / ".local-tests/written-plan.json"
        with patch.object(clone, "create_plan", wraps=clone.create_plan) as parse:
            token = clone.write_plan_result(self.backend, str(self.backend / ".local-tests/input.json"), str(output))
            self.path = output
            with self.case.open():
                self.engine(token)
            self.assertEqual(parse.call_count, 1)

    def test_independent_constructor_still_fully_validates_and_rejects_bad_payload(self):
        with patch.object(clone, "create_plan", wraps=clone.create_plan) as parse:
            with self.case.open():
                self.case.engine()
            self.assertEqual(parse.call_count, 1)
        (self.case.fixture.root / "file.bin").write_bytes(b"evil")
        with self.assertRaisesRegex(ValueError, "变化"):
            clone.verify_plan(self.backend, str(self.path))

    def test_external_json_and_unissued_objects_cannot_authorize_reuse(self):
        for token in ({"verified": True, "plan": self.case.plan}, object.__new__(clone.VerifiedPlan), object()):
            with self.subTest(kind=type(token).__name__), self.assertRaisesRegex(ValueError, "正式签发"):
                clone.reuse_plan(self.backend, str(self.path), token)
        with self.assertRaises(TypeError):
            clone.VerifiedPlan()

    def test_snapshot_is_immutable_and_copy_pickle_do_not_create_accepted_tokens(self):
        token = self.issue()
        first = token.plan
        first["objects"][0]["metadata"]["Metadata"]["changed"] = "changed"
        self.assertNotEqual(first, token.plan)
        with self.assertRaises(AttributeError):
            token.plan = first
        for make in (copy.copy, copy.deepcopy, lambda value: pickle.loads(pickle.dumps(value))):
            try:
                candidate = make(token)
            except TypeError:
                continue
            with self.assertRaisesRegex(ValueError, "正式签发"):
                clone.reuse_plan(self.backend, str(self.path), candidate)
        wrapper, declaration, bindings = clone.reuse_plan(self.backend, str(self.path), token)
        wrapper["plan"]["status"] = "tampered"
        declaration["copy_id"] = "tampered"
        bindings.clear()
        current = clone.reuse_plan(self.backend, str(self.path), token)
        self.assertEqual(current[0]["plan"], token.plan)
        self.assertNotEqual(current[1]["copy_id"], "tampered")
        self.assertTrue(current[2])

    def test_other_backend_identical_plan_file_and_different_token_are_rejected(self):
        token = self.issue()
        sibling = self.backend / ".local-tests/another-plan.json"
        shutil.copyfile(self.path, sibling)
        with self.assertRaisesRegex(ValueError, "不属于"):
            clone.reuse_plan(self.backend, str(sibling), token)
        with WorkspaceDirectory(dir=self.backend.parent) as other:
            other = Path(other)
            (other / ".local-tests").mkdir()
            path = other / ".local-tests/plan.json"
            shutil.copyfile(self.path, path)
            with self.assertRaisesRegex(ValueError, "不属于"):
                clone.reuse_plan(other, str(path), token)
        second = clone.verify_plan_result(self.backend, str(sibling))
        with self.assertRaisesRegex(ValueError, "不属于"):
            clone.reuse_plan(self.backend, str(self.path), second)

    def test_plan_and_input_changes_after_verification_are_rejected(self):
        token = self.issue()
        for path in (self.path, self.backend / ".local-tests/input.json"):
            with self.subTest(file=path.name):
                original = path.read_bytes()
                path.write_bytes(original + b" ")
                try:
                    with self.assertRaisesRegex(ValueError, "完整输入文件变化"):
                        clone.reuse_plan(self.backend, str(self.path), token)
                finally:
                    path.write_bytes(original)

    def test_all_catalog_inputs_change_including_unsupported_generated_tables(self):
        token = self.issue()
        for relative in clone.CATALOG_INPUTS:
            with self.subTest(path=relative):
                path = self.backend / relative
                original = path.read_bytes()
                path.write_bytes(original + b"\n// changed\n")
                try:
                    with self.assertRaisesRegex(ValueError, "源码策略"):
                        clone.reuse_plan(self.backend, str(self.path), token)
                finally:
                    path.write_bytes(original)

    def test_tool_source_content_and_file_set_are_bound_without_rewriting_real_tools(self):
        # 使用额外隔离目录模拟当前加载模型位置，不改实际候选或主源码。
        root = self.backend / ".local-tests/policy"
        root.mkdir()
        module = root / "devex_clone_model.py"
        module.write_text("# policy fixture\n", encoding="utf-8")
        with patch.object(clone.devex_clone_model, "__file__", str(module)):
            token = self.issue()
            original = module.read_bytes()
            module.write_bytes(original.replace(b"fixture", b"changed"))
            with self.assertRaisesRegex(ValueError, "源码策略"):
                clone.reuse_plan(self.backend, str(self.path), token)
            module.write_bytes(original)
            helper = root / "new_helper.py"
            helper.write_text("# new policy input\n")
            with self.assertRaisesRegex(ValueError, "源码策略"):
                clone.reuse_plan(self.backend, str(self.path), token)
            helper.unlink()
            module.unlink()
            with self.assertRaises((ValueError, OSError)):
                clone.reuse_plan(self.backend, str(self.path), token)

    def test_full_evidence_and_payload_hashes_are_checked_at_reuse(self):
        token = self.issue()
        files = [*self.case.fixture.value["evidence"].values(),
                 *[item["artifact"] for item in self.case.fixture.value["databases"]],
                 self.case.fixture.value["objects"][-1]["entries"][0]["artifact"]]
        for binding in files:
            path = self.case.fixture.root / binding["file"]
            original = path.read_bytes()
            path.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
            try:
                with self.subTest(file=path.name), self.assertRaisesRegex(ValueError, "变化"):
                    clone.reuse_plan(self.backend, str(self.path), token)
            finally:
                path.write_bytes(original)

    def test_payload_mutation_after_constructor_and_immediately_before_write_prevents_put(self):
        token = self.issue()
        item = self.case.plan["objects"][0]
        payload = self.case.fixture.root / item["artifact"]["file"]
        for timing in ("after-constructor", "write-check"):
            with self.subTest(timing=timing):
                self.case.ledger_dir = self.case.work / timing
                payload.write_bytes(b"data")
                with self.case.open():
                    engine = self.engine(token)
                    if timing == "after-constructor":
                        payload.write_bytes(b"evil")
                    else:
                        original = engine._check
                        calls = 0

                        def changed(step=None):
                            nonlocal calls
                            original(step)
                            calls += 1
                            if calls == 2:
                                payload.write_bytes(b"evil")

                        engine._check = changed
                    with self.assertRaisesRegex(ValueError, "变化"):
                        engine.apply_object(item["bucket"], item["target_key"])
                self.assertEqual(self.case.writes(), [])
        payload.write_bytes(b"data")

    def test_external_tools_and_live_generation_remain_checked_after_reuse(self):
        token = self.issue()
        with self.case.open():
            engine = self.engine(token)
            self.case.tools.plan["target"]["scope_id"] = "unexpected-target"
            with self.assertRaisesRegex(ValueError, "绑定变化"):
                engine.apply_database(self.case.plan["databases"][0]["key"])
        self.assertEqual(self.case.writes(), [])


class CompositionReuseTests(unittest.TestCase):
    def test_factory_passes_its_single_semantic_result_to_transfer(self):
        from test_devex_clone_factory import FactoryTests

        case = FactoryTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        with patch.object(clone, "create_plan", wraps=clone.create_plan) as parse:
            result = case.execute()
            self.assertEqual(result["status"], "data_steps_verified")
            self.assertEqual(parse.call_count, 1)

    def test_hydrate_passes_its_single_semantic_result_to_reconciliation(self):
        from test_devex_clone_resume import ResumeTests

        case = ResumeTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        case.interrupted()
        with patch.object(clone, "create_plan", wraps=clone.create_plan) as parse:
            result = case.continue_copy("reconcile")
            self.assertEqual(result["status"], "copy_reconciled")
            self.assertEqual(parse.call_count, 1)


if __name__ == "__main__":
    unittest.main()
