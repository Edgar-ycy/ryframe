"""原生目标复核的只读历史协议，保留完整谱系核验。"""

from contextlib import nullcontext
import copy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

import devex_clone_run_cli as cli
import devex_clone_target as target
from devex_clone_run_state import binding
from restore_build import file_digest
import test_devex_clone_factory_context as history_fixture


class AuthorityProtocolTests(unittest.TestCase):
    def payload(self, **changes):
        root = Path(__file__).resolve().parents[2]
        request = {
            "backend_dir": str(root), "run_dir": str(root / ".local-tests/authority-run"),
            "stage": "target-verify", "mode": "run", "operation": "target-authority",
            "attempt": None, "write": False,
        }
        request.update(changes)
        return json.dumps({"format_version": 1, "kind": cli.STAGE_PROTOCOL_KIND, "request": request,
                           "native_tool": {"format_version": 1, "kind": "ryframe-native-rate-limit-tool",
                                           "path": str(Path(sys.executable).resolve()),
                                           **file_digest(Path(sys.executable).resolve())}})

    def test_authority_is_readonly_and_has_no_attempt(self):
        root, arguments = cli.decode_clone_stage_protocol(self.payload())
        self.assertEqual(root, Path(__file__).resolve().parents[2])
        self.assertIsNone(arguments.attempt)
        self.assertIs(arguments.write, False)
        with patch.object(cli, "evidence_path", return_value=arguments.run_dir), patch.object(
            target, "read_target_authority", return_value={"remote_writes": 0}
        ) as observe, patch("devex_clone_run.execute_operation") as mutate:
            self.assertEqual(cli.dispatch_stage_operation(arguments, root), {"remote_writes": 0})
        observe.assert_called_once_with(root, arguments.run_dir)
        mutate.assert_not_called()

    def test_other_stages_modes_writes_and_attempts_are_rejected(self):
        for change in (
            {"stage": "export"}, {"stage": "copy"}, {"mode": "resume"},
            {"mode": "reconcile"}, {"write": True}, {"write": 0},
            {"attempt": 1}, {"attempt": False}, {"backend_dir": "relative"},
        ):
            with self.subTest(change=change), self.assertRaises(cli.StageProtocolError):
                cli.decode_clone_stage_protocol(self.payload(**change))

    def test_duplicate_unknown_and_wrong_version_fields_are_rejected(self):
        text = self.payload()
        for malformed in (
            text.replace('"format_version": 1', '"format_version": 1, "format_version": 1'),
            text.replace('"format_version": 1', '"format_version": true'),
            text.replace('"format_version": 1', '"format_version": 2'),
            text.replace('"write": false', '"write": false, "extra": null'),
        ):
            with self.subTest(malformed=malformed), self.assertRaises(cli.StageProtocolError):
                cli.decode_clone_stage_protocol(malformed)

    def test_malformed_selector_types_report_protocol_errors(self):
        for field in ("stage", "mode", "operation"):
            for invalid in ([], {}, None, False, 1.5):
                with self.subTest(field=field, invalid=invalid), self.assertRaises(cli.StageProtocolError):
                    cli.decode_clone_stage_protocol(self.payload(**{field: invalid}))


class TargetAuthorityTests(unittest.TestCase):
    def setUp(self):
        self.history = history_fixture.TargetHistoryTests("runTest")
        self.addCleanup(self.history.doCleanups)
        self.history.setUp()
        self.f = self.history.f
        self.manifest = {
            "copy_stage": "source_to_seed", "initialized": binding(self.f.output / "initialized.json"),
            "build_bridges": [],
        }
        self.expected = {
            key: value for key, value in self.history.initialized["generation"].items()
            if key != "storage"
        }
        self.enterContext(patch("devex_clone_run.read_manifest", side_effect=lambda *_: copy.deepcopy(self.manifest)))
        self.enterContext(patch("devex_clone_run.environments", return_value=self))
        self.enterContext(patch("source_fingerprints.artifact_sources", return_value=nullcontext()))
        self.generation = self.enterContext(patch.object(target, "generation", return_value=self.expected))

    def use(self, side):
        self.assertEqual(side, "target")
        return nullcontext()

    def observe(self):
        return target.read_target_authority(self.f.root, self.f.local / "run", self.f.run)

    def test_full_history_and_bindings_are_returned_without_writes(self):
        before = target.target_files(self.f.output)
        result = self.observe()
        self.assertEqual(result["files"], before)
        self.assertEqual(result["generation"], self.expected)
        self.assertEqual(result["initialized"], self.manifest["initialized"])
        self.assertEqual(result["request"], target.read_json(self.f.output / "prepare.json")["request"])
        self.assertEqual(result["remote_writes"], 0)
        for field in ("target_ready", "clone_verified", "restore_qualified"):
            self.assertIs(result[field], False)
        self.assertEqual(target.target_files(self.f.output), before)

    def test_current_generation_drift_is_rejected(self):
        self.generation.return_value = {**self.expected, "unknown": True}
        with self.assertRaisesRegex(ValueError, "不同于初始化代次"):
            self.observe()

    def test_changed_history_cannot_be_replaced_by_field_only_authority(self):
        (self.f.output / "reset.intent.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(ValueError):
            self.observe()
        self.generation.assert_not_called()

    def test_changes_during_external_observation_fail_final_tree_check(self):
        def mutate(*_):
            (self.f.output / "unexpected.json").write_text("{}", encoding="utf-8")
            return self.expected
        self.generation.side_effect = mutate
        with self.assertRaises(ValueError):
            self.observe()

    def test_seed_to_arm_remains_with_complete_existing_state_machine(self):
        self.manifest["copy_stage"] = "seed_to_arm"
        with self.assertRaisesRegex(ValueError, "其他阶段继续使用完整状态机"):
            self.observe()
        self.generation.assert_not_called()


if __name__ == "__main__":
    unittest.main()
