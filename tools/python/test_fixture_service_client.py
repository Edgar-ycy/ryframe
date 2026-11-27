"""原生历史调用桥只传协议，不继承服务凭据或解释生命周期。"""
from contextlib import nullcontext
import json
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import fixture_service_client as client


class FixtureServiceClientTests(unittest.TestCase):
    def test_missing_binding_refuses_before_any_child(self):
        with patch.object(client, "tool_binding", return_value=None), patch.object(client.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "缺少必需原生工具绑定"):
                client._invoke("service-document", {"path": Path("local.json")})
            run.assert_not_called()

    def test_wrong_binding_digest_never_starts_child(self):
        binding = client.tool_binding()
        self.assertIsNotNone(binding, "本测试须由正式 run_checks 绑定工具")
        with patch.object(client, "tool_binding", return_value={**binding, "sha256": "0" * 64}), patch.object(client.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                client._invoke("service-document", {"path": Path("local.json")})
            run.assert_not_called()

    def invoke(self, stdout, code=0):
        with (patch.object(client, "verified_tool", return_value=nullcontext("native-xtask.exe")),
              patch.object(client.subprocess, "run", return_value=subprocess.CompletedProcess(
                  [], code, stdout, b"private stderr marker")) as run):
            result = client._invoke("service-document", {"path": Path("local.json")})
        return result, run.call_args

    def test_request_uses_stdin_and_excludes_credentials_and_project_overrides(self):
        inherited = {**os.environ, "APP_ENV": "prod", "APP_REDIS_PASSWORD": "secret",
                     "AWS_SECRET_ACCESS_KEY": "secret", "RYFRAME_CONFIG": "secret"}
        # 在 Python 层替换映射，避免 Windows 还原空环境值时将它删除。
        with patch.object(os, "environ", inherited):
            result, call = self.invoke(b'{"ok":true,"result":null}')
        self.assertIsNone(result)
        self.assertEqual(call.args[0], ["native-xtask.exe", "__fixture-service-history"])
        self.assertNotIn("secret", repr(call))
        request = json.loads(call.kwargs["input"])
        self.assertEqual(request["operation"], "service-document")
        self.assertEqual(request["arguments"], {"path": "local.json"})
        self.assertEqual(call.kwargs["stderr"], subprocess.DEVNULL)
        self.assertNotIn("shell", call.kwargs)

    def test_path_protocol_preserves_json_types_and_rejects_arbitrary_objects(self):
        with (patch.object(client, "verified_tool", return_value=nullcontext("native-xtask.exe")),
              patch.object(client.subprocess, "run", return_value=subprocess.CompletedProcess(
                  [], 0, b'{"ok":true,"result":null}', b"")) as run):
            client._invoke("service-document", {"path": Path("local.json")})
        self.assertEqual(json.loads(run.call_args.kwargs["input"])["arguments"], {"path": "local.json"})
        with self.assertRaises(TypeError):
            client._invoke("service-document", {"path": object()})

    def test_invalid_envelopes_duplicates_and_exit_status_never_succeed(self):
        for raw, code in [(b'{"ok":true,"result":null}', 2),
                          (b'{"ok":true,"result":null,"extra":true}', 0),
                          (b'{"ok":true,"ok":false,"result":null}', 0),
                          (b'private non-json marker', 0), (b'\xff', 0)]:
            with self.subTest(raw=raw), self.assertRaises(RuntimeError) as raised:
                self.invoke(raw, code)
            self.assertNotIn("private", str(raised.exception))

    def test_launch_failure_and_timeout_do_not_echo_child_errors(self):
        for failure in [OSError("credential marker"), subprocess.TimeoutExpired("credential marker", 1)]:
            with (self.subTest(failure=type(failure).__name__),
                  patch.object(client, "verified_tool", return_value=nullcontext("native-xtask.exe")),
                  patch.object(client.subprocess, "run", side_effect=failure),
                  self.assertRaises(RuntimeError) as raised):
                client._invoke("service-document", {"path": Path("local.json")})
            self.assertNotIn("credential marker", str(raised.exception))

if __name__ == "__main__":
    unittest.main()
