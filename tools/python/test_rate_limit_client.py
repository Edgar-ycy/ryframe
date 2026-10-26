import json
import os
from collections.abc import Mapping
from contextlib import nullcontext
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
        self.binding = {"path": str(ROOT / ".local-tests/managed-tool.exe")}
        self.enterContext(mock.patch.object(rate_limit_client, "tool_binding", return_value=self.binding))
        self.enterContext(mock.patch.object(
            rate_limit_client, "verified_tool",
            side_effect=lambda value: nullcontext(None if value is None else value["path"]),
        ))

    def invoke(self, operation, environment, *, config_dir=None):
        function = {
            "load-table": rate_limit_client.load_app_table,
            "settings": rate_limit_client.rate_limit_settings,
            "authority": rate_limit_client.rate_limit_authority,
        }[operation]
        return function(ROOT, environment, config_dir=config_dir)

    def call(self, response, *, code=0, operation="load-table", config_dir=None):
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
            value = self.invoke(
                operation, {"APP_ENV": "test", "APP_CONFIG_DIR": "ignored"},
                config_dir=config_dir,
            )
        return value, run.call_args

    def test_success_uses_the_bound_hidden_entry_without_a_shell(self):
        value, call = self.call(
            {"ok": True, "result": {"app": {"host": "127.0.0.1"}}}, config_dir="fixture"
        )
        self.assertEqual(value, {"app": {"host": "127.0.0.1"}})
        argv = call.args[0]
        self.assertEqual(
            argv,
            [
                self.binding["path"],
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
                "app_env": "test",
                "config_dir": "fixture",
            },
        )

    def test_missing_binding_rejects_without_cargo_or_path_fallback(self):
        for operation in sorted(rate_limit_client.OPERATIONS):
            with (
                self.subTest(operation=operation),
                mock.patch.dict(os.environ, {"CARGO": "untrusted-cargo", "PATH": "untrusted-path"}),
                mock.patch.object(rate_limit_client, "tool_binding", return_value=None),
                mock.patch.object(rate_limit_client.subprocess, "run") as run,
                self.assertRaisesRegex(ValueError, "正式 xtask 或 run_checks.py") as raised,
            ):
                self.invoke(operation, {"APP_ENV": "test"})
            run.assert_not_called()
            self.assertNotIn("untrusted", str(raised.exception))

    def test_error_codes_map_to_stable_python_exceptions_without_private_values(self):
        missing = {
            "ok": False,
            "error": {"code": "config_file_missing", "message": "配置目录中缺少 app.test.toml"},
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
            ([], 0),
            ({"ok": 1, "result": {}}, 0),
            ({"ok": True, "result": {}, "extra": True}, 0),
            ({"ok": True, "result": {}}, 1),
            ({"ok": True, "result": []}, 0),
            ({"ok": False, "error": {"code": "x"}}, 1),
            ({"ok": False, "error": {"code": 1, "message": "safe"}}, 1),
            ({"ok": False, "error": {"code": "x", "message": 1}}, 1),
            ({"ok": False, "error": {"code": "x", "message": "safe", "extra": 1}}, 1),
            ({"ok": False, "result": {}}, 1),
        ]:
            with self.subTest(response=response, code=code), self.assertRaises(
                (RuntimeError, ValueError)
            ):
                self.call(response, code=code)
        for output in (
            b"not-json", b"\xffprivate-value",
            b"x" * (rate_limit_client.MAX_OUTPUT_BYTES + 1),
        ):
            completed = subprocess.CompletedProcess([], 0, output)
            with (
                self.subTest(output_bytes=len(output)),
                mock.patch.object(rate_limit_client.subprocess, "run", return_value=completed),
                self.assertRaises(RuntimeError) as raised,
            ):
                rate_limit_client.load_app_table(ROOT, {"APP_ENV": "test"})
            self.assertNotIn("private-value", str(raised.exception))

    def test_non_text_environment_is_rejected_before_spawning(self):
        with mock.patch.object(rate_limit_client.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                rate_limit_client.load_app_table(ROOT, {"APP_ENV": 1})
        run.assert_not_called()

    def test_legacy_configuration_values_never_change_the_file_request(self):
        variants = [
            {"APP_ENV": "test"},
            {"APP_ENV": "test", "APP_CONFIG_DIR": "private-value\x00"},
            {"APP_ENV": "test", "APP_CONFIG_DIR": object()},
            {"APP_ENV": "test", "APP_RATE_LIMIT_CAPACITY": {"private-value": []}},
            {"APP_ENV": "test", "APP_DATABASE_PASSWORD": 1, "APP_RATE_LIMIT_FILE": False},
            {"APP_ENV": "test", "APP_UNKNOWN": b"private-value", 42: object()},
        ]
        result = {}
        completed = subprocess.CompletedProcess(
            [], 0, json.dumps({"ok": True, "result": result}).encode("utf-8")
        )
        for operation in sorted(rate_limit_client.OPERATIONS):
            for config_dir in (None, "选定 配置"):
                expected = {"backend": str(ROOT), "app_env": "test"}
                if config_dir is not None:
                    expected["config_dir"] = config_dir
                payloads = []
                for environment in variants:
                    with (
                        self.subTest(operation=operation, config_dir=config_dir),
                        mock.patch.dict(os.environ, {
                            "APP_ENV": "prod", "APP_CONFIG_DIR": "host-private-value",
                            "APP_RATE_LIMIT_CAPACITY": "host-private-value",
                        }, clear=True),
                        mock.patch.object(
                            rate_limit_client.subprocess, "run", return_value=completed
                        ) as run,
                    ):
                        self.assertEqual(
                            self.invoke(operation, environment, config_dir=config_dir), result
                        )
                    call = run.call_args
                    payloads.append(call.kwargs["input"])
                    self.assertEqual(json.loads(payloads[-1].decode("utf-8")), expected)
                    self.assertFalse(any(key.upper().startswith("APP_") for key in call.kwargs["env"]))
                    self.assertNotIn(b"private-value", payloads[-1])
                self.assertTrue(all(payload == payloads[0] for payload in payloads))

    def test_environment_mapping_is_only_queried_for_app_env(self):
        class AppEnvOnly(Mapping):
            def __getitem__(self, key):
                if key != "APP_ENV":
                    raise AssertionError("不得读取其他配置项")
                return "dev"

            def __iter__(self):
                raise AssertionError("不得扫描配置映射")

            def __len__(self):
                raise AssertionError("不得扫描配置映射")

        result = {}
        for operation in sorted(rate_limit_client.OPERATIONS):
            with (
                self.subTest(operation=operation),
                mock.patch.object(rate_limit_client, "_invoke", return_value=result) as invoke,
            ):
                self.invoke(operation, AppEnvOnly())
            request = invoke.call_args.args[1]
            self.assertEqual(request["app_env"], "dev")
            self.assertNotIn("environment", request)
            self.assertNotIn("config_dir", request)

    def test_app_env_requires_exact_dev_or_test_before_spawning(self):
        invalid = (None, "", "prod", "DEV", "TEST", " dev", "test ", "test\n",
                   "private-value", 1, False, b"test", [], {})
        for operation in sorted(rate_limit_client.OPERATIONS):
            environments = [{}, *({"APP_ENV": value} for value in invalid), None, []]
            for environment in environments:
                with (
                    self.subTest(operation=operation, environment=environment),
                    mock.patch.object(rate_limit_client.subprocess, "run") as run,
                    self.assertRaisesRegex(ValueError, "APP_ENV") as raised,
                ):
                    self.invoke(operation, environment)
                run.assert_not_called()
                self.assertNotIn("private-value", str(raised.exception))

    def test_config_dir_preserves_relative_absolute_and_pathlike_text(self):
        class Directory(os.PathLike):
            def __fspath__(self):
                return "自定义 配置"

        values = ("./配置 目录/../选定", Path("配置 目录"), ROOT / "绝对 配置", Directory())
        result = {}
        for operation in sorted(rate_limit_client.OPERATIONS):
            for directory in values:
                with (
                    self.subTest(operation=operation, directory=os.fspath(directory)),
                    mock.patch.object(rate_limit_client, "_invoke", return_value=result) as invoke,
                ):
                    self.invoke(operation, {"APP_ENV": "dev"}, config_dir=directory)
                request = invoke.call_args.args[1]
                self.assertEqual(request["config_dir"], os.fspath(directory))
                self.assertEqual(request["backend"], str(ROOT.resolve()))

    def test_invalid_config_dir_has_safe_field_diagnostics_before_spawning(self):
        class InvalidDirectory(os.PathLike):
            def __fspath__(self):
                raise ValueError("private-value")

        invalid = ("", "private-value\x00", "private-value\n", "private-value\t",
                   "private-value\x7f", "private-value\x85", b"private-value", 1,
                   False, [], {"private-value": "directory"}, object(), InvalidDirectory())
        for operation in sorted(rate_limit_client.OPERATIONS):
            for directory in invalid:
                with (
                    self.subTest(operation=operation, kind=type(directory).__name__),
                    mock.patch.object(rate_limit_client.subprocess, "run") as run,
                    self.assertRaisesRegex(ValueError, "config_dir") as raised,
                ):
                    self.invoke(operation, {"APP_ENV": "test"}, config_dir=directory)
                run.assert_not_called()
                self.assertEqual(
                    str(raised.exception), "config_dir 必须是非空且不含控制字符的文本路径"
                )
                self.assertNotIn("private-value", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
