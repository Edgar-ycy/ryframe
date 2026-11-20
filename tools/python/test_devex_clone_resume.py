"""原目录接续的离线回归；真实账本配合明确协议替身，不声明真实复制通过。"""
import copy
from dataclasses import replace
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_factory as factory
from devex_clone_ledger import CloneLedger, LedgerError
from devex_clone_resume import continue_copy
from restore_reference_io import ObjectCreateError
import test_devex_clone_factory as fixtures


class ResumeTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.FactoryTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.output, self.backend = self.case.output, self.case.backend

    def continue_copy(self, mode, **environments):
        values = self.case.environments | environments
        return continue_copy(self.backend, self.output, values["source"], values["target"],
                             mode=mode, run=self.case.case.runner, source_storage_run=self.case.run_dir)

    def interrupted(self, failure="commit-response-lost"):
        self.case.case.runner.failure = failure
        expected = ObjectCreateError if failure == "put-response-lost" else subprocess.TimeoutExpired
        with self.assertRaises(expected):
            self.case.execute()
        self.case.case.runner.failure = None
        return len(self.case.case.writes())

    def test_prewrite_interruption_reuses_plan_export_and_initialized_directory(self):
        with patch.object(factory, "TransferSteps", side_effect=RuntimeError("offline interrupted before intent")):
            with self.assertRaises(RuntimeError):
                self.case.execute()
        paths = [self.output / name for name in ("plan.json", "input.json", "session.json")]
        original = {path: path.read_bytes() for path in paths}
        with patch.object(factory.CloneSession, "prepare", side_effect=AssertionError("must not prepare again")), \
                patch.object(factory, "target_binding", side_effect=AssertionError("copied target is not fresh")):
            result = self.continue_copy("reconcile")
            self.assertEqual(result["steps"], {})
            self.assertEqual(len(result["pending_steps"]), 3)
            self.continue_copy("resume")
        self.assertEqual(original, {path: path.read_bytes() for path in paths})
        self.assertEqual(len(self.case.case.writes()), 3)


if __name__ == "__main__":
    unittest.main()
