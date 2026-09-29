"""夹具运行时的私有协议必须拒绝歧义，并只产生结构化请求。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_runtime as runtime


class ReferenceFixtureRuntimeProtocolTests(unittest.TestCase):
    def setUp(self):
        root = Path(__file__).resolve().parents[2]
        self.runtime = root / ".local-tests/device/runtime-r1"
        self.base = {
            "backend_dir": str(root),
            "environment": str(root / ".local-tests/device/bootstrap.json"),
            "format_version": 1,
            "operation": "verify",
            "output": str(self.runtime),
            "write": False,
        }

    def parse(self, value: dict) -> runtime.RuntimeRequest:
        environment = {runtime.PROTOCOL_KEY: json.dumps(value, separators=(",", ":"))}
        return runtime.private_protocol_request([], environment)

    def test_parses_exact_bind_protocol(self):
        value = {
            **self.base,
            "operation": "bind",
            "write": True,
            "browser_binding": str(self.runtime / "browser-binding-r24-device.json"),
            "run_id": "r24-device",
            "server": "preview",
        }
        request = self.parse(value)
        self.assertEqual(request.operation, "bind")
        self.assertEqual(request.browser_binding.name, "browser-binding-r24-device.json")
        self.assertEqual(request.server, "preview")

    def test_rejects_direct_arguments_before_reading_protocol(self):
        with self.assertRaisesRegex(runtime.RuntimeProtocolError, "不接受命令行参数"):
            runtime.private_protocol_request(["verify"], {})

    def test_direct_process_reports_argv_as_parameter_error(self):
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(runtime.PROTOCOL_PREFIX)}
        environment[runtime.PROTOCOL_KEY] = json.dumps(self.base, separators=(",", ":"))
        environment["PYTHONUTF8"] = "1"
        result = subprocess.run(
            [sys.executable, str(Path(runtime.__file__).resolve()), "verify"],
            cwd=Path(runtime.__file__).resolve().parents[1], env=environment,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("私有协议无效", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_rejects_duplicate_or_unknown_fields(self):
        raw = json.dumps(self.base)[:-1] + ',"operation":"status"}'
        for value in (
                raw,
                json.dumps({**self.base, "unknown": "value"}),
                "[]",
                "not-json",
        ):
            with self.subTest(value=value):
                with self.assertRaises(runtime.RuntimeProtocolError):
                    runtime.private_protocol_request([], {runtime.PROTOCOL_KEY: value})

    def test_rejects_wrong_write_mode_and_ambiguous_browser_fields(self):
        invalid = [
            {**self.base, "write": True},
            {**self.base, "operation": "start", "write": False},
            {**self.base, "operation": "browser", "write": True,
             "browser_binding": str(self.runtime / "binding.json"), "server": "dev"},
            {**self.base, "operation": "bind", "write": True,
             "browser_binding": str(self.runtime / "wrong.json"),
             "run_id": "r24-device", "server": "preview"},
        ]
        for value in invalid:
            with self.subTest(value=value):
                with self.assertRaises(runtime.RuntimeProtocolError):
                    self.parse(value)

    def test_rejects_relative_or_cross_directory_binding_paths(self):
        invalid = [
            {**self.base, "environment": "bootstrap.json"},
            {**self.base, "operation": "browser", "write": True,
             "browser_binding": str(self.runtime.parent / "binding.json")},
        ]
        for value in invalid:
            with self.subTest(value=value):
                with self.assertRaises(runtime.RuntimeProtocolError):
                    self.parse(value)

    def test_rejects_unknown_prefixed_environment(self):
        environment = {
            runtime.PROTOCOL_KEY: json.dumps(self.base),
            runtime.PROTOCOL_PREFIX + "EXTRA": "unexpected",
        }
        with self.assertRaisesRegex(runtime.RuntimeProtocolError, "未知字段"):
            runtime.private_protocol_request([], environment)


if __name__ == "__main__":
    unittest.main()
