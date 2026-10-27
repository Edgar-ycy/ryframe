"""保留的 Python 原语桥只执行明确叶子调用，不解释生命周期。"""
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import fixture_service_runtime as runtime

ROOT = Path(__file__).resolve().parents[2]


class FixtureServiceRuntimeTests(unittest.TestCase):
    def test_unknown_operation_fields_and_escaped_paths_reject_before_side_effects(self):
        for operation, args in [("close", {}), ("registered", {"context": {}, "settled": True}),
                                ("observe", {"services": {}}), ("cache-stop", {"password": "private"}),
                                ("controller", {"run": str(ROOT)})]:
            with (self.subTest(operation=operation),
                  patch("devex_clone_cache_process.stop") as stop,
                  self.assertRaises(ValueError)):
                runtime.dispatch(ROOT, operation, args)
            stop.assert_not_called()

    def test_context_backend_and_local_boundary_cannot_be_rebound(self):
        for context in [{"backend": str(ROOT.parent), "run": str(ROOT / ".local-tests/run")},
                        {"backend": str(ROOT), "run": str(ROOT)}]:
            with self.assertRaises(ValueError):
                runtime._context(context, ROOT)

    def test_tree_control_preserves_explicit_crash_and_leaf_result(self):
        with patch("full_stack_process_tree.terminate_owned_process_tree", return_value=False) as stop:
            self.assertIs(runtime.dispatch(ROOT, "tree-stop", {"tree": {"exact": True}, "crash": False}), False)
        stop.assert_called_once_with({"exact": True}, crash=False)

    def test_start_passes_supervision_and_guard_callback_without_stage_logic(self):
        context = {"backend": str(ROOT), "run": str(ROOT / ".local-tests/run")}
        arguments = {"request": {}, "private": {}, "output": str(ROOT / ".local-tests/output"),
                     "context": context, "running": {"bound": True}, "manifest": {},
                     "controller": {}, "number": 4}
        with (patch("devex_clone_storage_process.start", return_value={"started": True}) as start,
              patch("reference_fixture_service_context.guard") as guard):
            self.assertEqual(runtime.dispatch(ROOT, "storage-start", arguments), {"started": True})
            start.call_args.args[7]()
            self.assertTrue(start.call_args.kwargs["supervised"])
            guard.assert_called_once_with(context, {"bound": True})
        private = {"APP_ENV": "test", "TEMP": str(ROOT / ".local-tests/tmp"), "TMP": str(ROOT / ".local-tests/tmp")}
        cache = {key: arguments[key] for key in ("request", "output", "context", "running")}
        cache["private"] = private
        with (patch("devex_clone_cache_process.start", return_value={"started": True}) as start,
              patch("reference_fixture_service_context.guard") as guard):
            self.assertEqual(runtime.dispatch(ROOT, "cache-start", cache), {"started": True})
            self.assertEqual(start.call_args.args[0], ROOT)
            self.assertIs(start.call_args.args[2], private)
            start.call_args.args[4]()
        guard.assert_called_once_with(context, arguments["running"])

    def test_private_process_rejects_bad_protocol_without_echoing_input(self):
        cases = [b'{"format_version":1,"format_version":2,"password":"private-marker"}',
                 b'{"password":"private-marker"}', b"x" * (runtime.LIMIT + 1)]
        for payload in cases:
            result = subprocess.run([sys.executable, "-B", str(ROOT / "tools/python/fixture_service_runtime.py")],
                                    input=payload, capture_output=True, check=False)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stderr, b"")
            self.assertEqual(json.loads(result.stdout), {"ok": False, "error": "ValueError"})
            self.assertNotIn(b"private-marker", result.stdout)


if __name__ == "__main__":
    unittest.main()
