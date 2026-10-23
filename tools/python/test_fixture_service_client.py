"""原生历史调用桥只传协议，不继承服务凭据或解释生命周期。"""
from contextlib import nullcontext
import io
import json
import os
import subprocess
import unittest
from unittest.mock import patch

import fixture_service_client as client


class FixtureServiceClientTests(unittest.TestCase):
    def invoke(self, stdout, code=0):
        with (patch.object(client, "verified_tool", return_value=nullcontext("native-xtask.exe")),
              patch.object(client.subprocess, "run", return_value=subprocess.CompletedProcess(
                  [], code, stdout, b"private stderr marker")) as run):
            result = client._invoke("manifest", {"value": {}})
        return result, run.call_args

    def test_request_uses_stdin_and_excludes_credentials_and_project_overrides(self):
        with patch.dict(os.environ, {"APP_ENV": "prod", "APP_REDIS_PASSWORD": "secret",
                                     "AWS_SECRET_ACCESS_KEY": "secret", "RYFRAME_CONFIG": "secret"}):
            result, call = self.invoke(b'{"ok":true,"result":null}')
        self.assertIsNone(result)
        self.assertEqual(call.args[0], ["native-xtask.exe", "__fixture-service-history"])
        self.assertNotIn("secret", repr(call))
        self.assertEqual(json.loads(call.kwargs["input"])["arguments"], {"value": {}})
        self.assertEqual(call.kwargs["stderr"], subprocess.DEVNULL)
        self.assertNotIn("shell", call.kwargs)

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
                client._invoke("manifest", {"value": {}})
            self.assertNotIn("credential marker", str(raised.exception))

    def test_services_entry_installs_explicit_coordinator_binding_before_dispatch(self):
        import reference_fixture_services as services
        arguments = ["status", "--backend-dir", str(client.ROOT), "--review", "review.json",
                     "--environment", "bootstrap.json", "--native-tool", "bound-tool"]
        with (patch("rate_limit_tool.install_binding") as install,
              patch("reference_fixture_service_context.status", return_value={"status": "service_status"}) as status,
              patch("sys.stdout", io.StringIO())):
            services.main(arguments)
        install.assert_called_once_with("bound-tool")
        status.assert_called_once()

    def test_request_entry_installs_explicit_coordinator_binding_before_dispatch(self):
        import reference_fixture_request as request
        arguments = ["--backend-dir", str(client.ROOT), "--environment", "bootstrap.json",
                     "--service-run", "run", "--id", "fixture-request", "--side", "seed",
                     "--output", "request.json", "--write", "--native-tool", "bound-tool"]
        with (patch("rate_limit_tool.install_binding") as install,
              patch.object(request, "publish", return_value={}) as publish,
              patch("sys.stdout", io.StringIO())):
            request.main(arguments)
        install.assert_called_once_with("bound-tool")
        publish.assert_called_once()


if __name__ == "__main__":
    unittest.main()
