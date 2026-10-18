import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rate_limit_client
import rate_limit_tool


ROOT = Path(__file__).resolve().parents[2]


class RateLimitClientTests(unittest.TestCase):
    def setUp(self):
        coordinator = mock.patch.object(rate_limit_tool, "_COORDINATOR_BINDING", None)
        coordinator.start()
        self.addCleanup(coordinator.stop)

    def call(self, response, *, code=0, operation="load-table"):
        completed = subprocess.CompletedProcess(
            args=[], returncode=code, stdout=json.dumps(response).encode("utf-8"), stderr=b""
        )
        environment = {
            "CARGO": "cargo-fixture",
            "PATH": "/fixture/bin",
            "PYTHONUTF8": "1",
            "PYTHONIOENCODING": "cp936",
            "APP_DATABASE_PASSWORD": "private-value",
        }
        with (
            mock.patch.dict(os.environ, environment, clear=True),
            mock.patch.object(
                rate_limit_client.subprocess, "run", return_value=completed
            ) as run,
        ):
            if operation == "load-table":
                value = rate_limit_client.load_app_table(
                    ROOT, {"APP_ENV": "test", "APP_CONFIG_DIR": "fixture"}
                )
            elif operation == "settings":
                value = rate_limit_client.rate_limit_settings(ROOT, {"APP_ENV": "test"})
            elif operation == "authority":
                value = rate_limit_client.rate_limit_authority(
                    ROOT, {"APP_ENV": "test"}
                )
            else:
                value = rate_limit_client.login_budget_environment(
                    ROOT,
                    {"APP_ENV": "test"},
                    ROOT / ".local-tests/budget.json",
                )
        return value, run.call_args

    def test_success_uses_the_cargo_hidden_entry_without_a_shell(self):
        value, call = self.call({"ok": True, "result": {"app": {"host": "127.0.0.1"}}})
        self.assertEqual(value, {"app": {"host": "127.0.0.1"}})
        argv = call.args[0]
        self.assertEqual(
            argv,
            [
                "cargo-fixture",
                "run",
                "--quiet",
                "--locked",
                "-p",
                "xtask",
                "--",
                "__rate-limit",
                "load-table",
            ],
        )
        self.assertNotIn("shell", call.kwargs)
        self.assertEqual(call.kwargs["cwd"], rate_limit_client.ROOT)
        self.assertEqual(call.kwargs["timeout"], 30)
        self.assertEqual(call.kwargs["stdout"], subprocess.PIPE)
        self.assertEqual(call.kwargs["stderr"], subprocess.DEVNULL)
        self.assertNotIn("APP_DATABASE_PASSWORD", call.kwargs["env"])
        self.assertNotIn("PYTHONUTF8", call.kwargs["env"])
        self.assertNotIn("PYTHONIOENCODING", call.kwargs["env"])
        if rate_limit_client._HOST_PROCESS_ENVIRONMENT:
            self.assertTrue(
                set(rate_limit_client._HOST_PROCESS_ENVIRONMENT)
                & set(call.kwargs["env"])
            )
        request = json.loads(call.kwargs["input"].decode("utf-8"))
        self.assertEqual(
            request,
            {
                "backend": str(ROOT),
                "environment": {"APP_ENV": "test", "APP_CONFIG_DIR": "fixture"},
            },
        )

    def test_host_toolchain_locator_survives_environment_clear(self):
        completed = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=b'{"ok":true,"result":{}}', stderr=b""
        )
        with (
            mock.patch.dict(os.environ, {"APP_ENV": "test"}, clear=True),
            mock.patch.object(
                rate_limit_client.subprocess, "run", return_value=completed
            ) as run,
        ):
            self.assertEqual(
                rate_limit_client.load_app_table(ROOT, {"APP_ENV": "test"}), {}
            )
        self.assertEqual(run.call_args.args[0][0], rate_limit_client._CARGO_EXECUTABLE)
        if rate_limit_client._HOST_PROCESS_ENVIRONMENT:
            self.assertTrue(
                set(rate_limit_client._HOST_PROCESS_ENVIRONMENT)
                & set(run.call_args.kwargs["env"])
            )

    def test_login_budget_keeps_environment_nested_and_returns_exact_fields(self):
        response = {
            "ok": True,
            "result": {
                "RYFRAME_E2E_LOGIN_BUDGET_STATE": str(
                    ROOT / ".local-tests/budget.json"
                ),
                "RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY": "5",
                "RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS": "60",
            },
        }
        value, call = self.call(response, operation="login-budget")
        self.assertEqual(set(value), set(response["result"]))
        body = json.loads(call.kwargs["input"].decode("utf-8"))
        self.assertEqual(body["environment"], {"APP_ENV": "test"})
        self.assertEqual(body["state_path"], str(ROOT / ".local-tests/budget.json"))
        self.assertEqual(set(body), {"backend", "environment", "state_path"})

    def test_error_codes_map_to_stable_python_exceptions_without_private_values(self):
        missing = {
            "ok": False,
            "error": {"code": "config_file_missing", "message": "配置目录中缺少 app.toml"},
        }
        with self.assertRaises(FileNotFoundError):
            self.call(missing, code=1)
        invalid = {
            "ok": False,
            "error": {
                "code": "rate_limit_integer_invalid",
                "message": "rate_limit 数值字段必须是在允许范围内的正整数",
            },
        }
        with self.assertRaises(ValueError) as raised:
            self.call(invalid, code=1)
        self.assertIn("rate_limit_integer_invalid", str(raised.exception))
        self.assertNotIn("private-value", str(raised.exception))

    def test_invalid_or_inconsistent_protocol_fails_closed(self):
        for response, code in [
            ({"ok": True, "result": {}, "extra": True}, 0),
            ({"ok": True, "result": {}}, 1),
            ({"ok": False, "error": {"code": "x"}}, 1),
            ({"ok": False, "result": {}}, 1),
        ]:
            with self.subTest(response=response, code=code), self.assertRaises(
                (RuntimeError, ValueError)
            ):
                self.call(response, code=code)
        completed = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=b"not-json", stderr=b""
        )
        with mock.patch.object(
            rate_limit_client.subprocess, "run", return_value=completed
        ):
            with self.assertRaises(RuntimeError):
                rate_limit_client.load_app_table(ROOT, {"APP_ENV": "test"})
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=b"x" * (rate_limit_client.MAX_OUTPUT_BYTES + 1),
            stderr=b"",
        )
        with mock.patch.object(
            rate_limit_client.subprocess, "run", return_value=completed
        ):
            with self.assertRaises(RuntimeError):
                rate_limit_client.load_app_table(ROOT, {"APP_ENV": "test"})

    def test_non_text_environment_is_rejected_before_spawning(self):
        with mock.patch.object(rate_limit_client.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                rate_limit_client.load_app_table(ROOT, {"APP_ENV": 1})
        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
