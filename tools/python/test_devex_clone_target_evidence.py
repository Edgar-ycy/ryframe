"""目标证据原生协议的失败关闭与单次调用边界。"""
from contextlib import nullcontext
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import devex_clone_target_evidence as client


class NativeEvidenceTransportTests(unittest.TestCase):
    def setUp(self):
        self.target = Path(__file__).resolve().parents[2] / ".local-tests/native-target-evidence"
        self.backend = self.target.parent.parent
        self.enterContext(patch.object(client, "tool_binding", return_value={"fixed": True}))
        self.enterContext(patch.object(client, "verified_tool", return_value=nullcontext("bound-xtask.exe")))
        self.run = self.enterContext(patch.object(client.subprocess, "run"))

    def response(self, value, status=0):
        self.run.return_value = subprocess.CompletedProcess([], status, json.dumps(value).encode(), b"")

    def test_bound_tool_receives_stdin_and_no_project_credentials(self):
        self.response({"ok": True, "result": {"status": "completed"}})
        result = client.evidence("history", self.backend, self.target)
        self.assertEqual(result, {"status": "completed"})
        args, options = self.run.call_args
        self.assertEqual(args[0], ["bound-xtask.exe", "__target-evidence"])
        payload = json.loads(options["input"])
        self.assertEqual(payload["backend"], str(self.target.parent.parent))
        self.assertEqual(payload["target"], str(self.target))
        self.assertEqual(payload["operation"], "history")
        self.assertNotIn("APP_ENV", options["env"])
        self.assertIs(options["stderr"], subprocess.DEVNULL)

    def test_unknown_or_inconsistent_results_are_never_retried(self):
        for status, response in [(1, {"ok": True, "result": {}}),
                                 (0, {"ok": True, "result": []}),
                                 (0, {"ok": True, "result": {}, "extra": True}),
                                 (0, {"ok": False, "error": {"message": "untrusted"}})]:
            with self.subTest(status=status, response=response):
                self.run.reset_mock()
                self.response(response, status)
                with self.assertRaises(ValueError):
                    client.evidence("history", self.backend, self.target)
                self.run.assert_called_once()

    def test_transport_failure_does_not_reveal_raw_output_or_retry(self):
        self.run.side_effect = subprocess.TimeoutExpired(["fake-secret"], 30,
                                                        output=b"fake-secret", stderr=b"fake-secret")
        with self.assertRaises(ValueError) as captured:
            client.evidence("history", self.backend, self.target)
        self.assertNotIn("fake-secret", str(captured.exception))
        self.run.assert_called_once()

    def test_duplicate_json_response_is_rejected(self):
        self.run.return_value = subprocess.CompletedProcess([], 0, b'{"ok":true,"result":{},"result":{}}', b"")
        with self.assertRaises(ValueError):
            client.evidence("history", self.backend, self.target)
        self.run.assert_called_once()

    def test_missing_tool_binding_never_invokes_cargo_or_a_subprocess(self):
        with patch.object(client, "tool_binding", return_value=None), self.assertRaisesRegex(
                ValueError, "缺少受管 Rust 工具"):
            client.evidence("history", self.backend, self.target)
        self.run.assert_not_called()

    def test_invalid_boundary_and_oversized_request_never_launch(self):
        for target, fields in [(self.target.parent, {}),
                               (self.target, {"manifest": "x" * (1024 * 1024)})]:
            with self.assertRaises(ValueError):
                client.evidence("history", self.backend, target, **fields)
        self.run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
