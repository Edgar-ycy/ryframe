"""seed 模型适配器的失败、绑定、取消及无隐式重试边界。"""
from contextlib import contextmanager
import json
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import devex_clone_seed_guards as guards


@contextmanager
def executable(value):
    yield value


class GuardTransportTests(unittest.TestCase):
    def invoke_with(self, response, *, code=0):
        runner = Mock(return_value=SimpleNamespace(returncode=code, stdout=response))
        with patch.object(guards, "verified_tool", return_value=executable("bound-tool.exe")), \
                patch.object(guards, "tool_binding", return_value={"fixture": True}), \
                patch.object(guards, "run_native", runner):
            result = guards.guard("identity", {"value": {"fixture": True}})
        return result, runner

    def test_valid_response_requires_literal_zero_writes_and_no_retry_permission(self):
        response = {"ok": True, "result": {"value": {"validated": True}, "remote_writes": 0,
                                           "automatic_retry_permitted": False}}
        result, runner = self.invoke_with(json.dumps(response).encode())
        self.assertEqual(result, {"validated": True})
        runner.assert_called_once()
        command = runner.call_args.args[0]
        self.assertEqual(command, ["bound-tool.exe", "__seed-evidence"])
        payload = json.loads(runner.call_args.kwargs["input"])
        self.assertEqual(payload["operation"], "close-rebind-guard")
        self.assertNotIn("APP_CONFIG_DIR", runner.call_args.kwargs["env"])
        self.assertFalse(runner.call_args.kwargs["check"])

    def test_missing_formal_binding_fails_before_any_process_and_never_compiles(self):
        with patch.object(guards, "verified_tool", return_value=executable(None)), \
                patch.object(guards, "tool_binding", return_value=None), \
                patch.object(guards, "run_native") as runner, self.assertRaisesRegex(ValueError, "正式入口绑定"):
            guards.guard("identity", {"value": {}})
        runner.assert_not_called()

    def test_invalid_or_duplicate_response_and_nonzero_exit_fail_once(self):
        valid = {"ok": True, "result": {"value": None, "remote_writes": 0, "automatic_retry_permitted": False}}
        invalid = [b"not-json", b'{"ok":true,"ok":false}', json.dumps({"ok": True}).encode(),
                   json.dumps({"ok": False, "error": {"message": "secret-credential"}}).encode()]
        for key, value in (("remote_writes", True), ("remote_writes", 1), ("automatic_retry_permitted", True)):
            invalid.append(json.dumps({**valid, "result": {**valid["result"], key: value}}).encode())
        for response in invalid:
            with self.subTest(response=response), self.assertRaises(ValueError) as raised:
                self.invoke_with(response)
            self.assertNotIn("secret-credential", str(raised.exception))
        with self.assertRaises(ValueError):
            self.invoke_with(json.dumps(valid).encode(), code=1)

    def test_timeout_and_cancellation_do_not_retry_or_report_success(self):
        for error in (subprocess.TimeoutExpired("bound-tool", 30), KeyboardInterrupt()):
            runner = Mock(side_effect=error)
            with patch.object(guards, "verified_tool", return_value=executable("bound-tool.exe")), \
                    patch.object(guards, "tool_binding", return_value={}), \
                    patch.object(guards, "run_native", runner), \
                    self.assertRaises(KeyboardInterrupt if isinstance(error, KeyboardInterrupt) else ValueError):
                guards.guard("identity", {"value": {}})
            runner.assert_called_once()


if __name__ == "__main__":
    unittest.main()
