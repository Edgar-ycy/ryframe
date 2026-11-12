from rate_limit_tool import tool_binding
import json
import io
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import reference_fixture_request as fixture_request
from reference_fixture_control_protocol import (
    FixtureControlProtocolError,
    private_arguments,
    run_private,
)


KIND = "ryframe-reference-fixture-control"


class FixtureControlProtocolTests(unittest.TestCase):
    class BinaryInput:
        def __init__(self, payload):
            self.buffer = io.BytesIO(payload)

    def test_domain_failure_reports_validation_reason_without_credentials(self):
        raw = self.request()
        output = io.StringIO()

        def fail(_args):
            raise ValueError("审阅来源错误：secret-for-test")
        with (
            mock.patch.dict(os.environ, {"APP_PASSWORD": "secret-for-test"}),
            mock.patch.object(sys, "argv", ["reference_fixture_request.py"]),
            mock.patch.object(sys, "stderr", output),
        ):
            result = run_private(
                "request", fixture_request.PROTOCOL_SCHEMAS, fail,
                positional_operation=False, raw=raw,
            )
        self.assertEqual(result, 1)
        self.assertIn("审阅来源错误", output.getvalue())
        self.assertIn("[REDACTED]", output.getvalue())
        self.assertNotIn("secret-for-test", output.getvalue())

    def setUp(self):
        self.root = str(Path(__file__).resolve().parents[2])

    def request(self, **changes):
        local = Path(self.root) / ".local-tests"
        value = {
            "backend_dir": self.root,
            "domain": "request",
            "format_version": 1,
            "kind": KIND,
            "operation": "publish",
            "write": True,
            "environment": str(local / "environment.json"),
            "service_run": str(local / "run"),
            "id": "fresh-base",
            "side": "base",
            "output": str(local / "request.json"),
            "native_tool": json.dumps(tool_binding()),
        }
        value.update(changes)
        return json.dumps(value, separators=(",", ":"))

    def arguments(self, raw=None, *, argv=()):
        return private_arguments(
            "request",
            fixture_request.PROTOCOL_SCHEMAS,
            positional_operation=False,
            raw=raw,
            argv=list(argv),
        )

    def test_stdin_protocol_is_bounded_and_strict_utf8(self):
        raw = self.request()
        with mock.patch.object(sys, "stdin", self.BinaryInput(raw.encode("utf-8"))):
            self.assertEqual(self.arguments(), self.arguments(raw))
        for payload, reason in [
            (b"", "缺失"),
            (b"\xff", "UTF-8"),
            (b"x" * 65537, "过长"),
        ]:
            with (
                self.subTest(reason=reason),
                mock.patch.object(sys, "stdin", self.BinaryInput(payload)),
                self.assertRaisesRegex(FixtureControlProtocolError, reason),
            ):
                self.arguments()

    def test_reconstructs_exact_request_options(self):
        raw = self.request()
        value = json.loads(raw)
        self.assertEqual(self.arguments(raw), [
            "--backend-dir", self.root, "--environment", value["environment"],
            "--service-run", value["service_run"], "--id", "fresh-base", "--side", "base",
            "--output", value["output"], "--native-tool", value["native_tool"], "--write",
        ])

    def test_rejects_duplicate_unknown_wrong_domain_and_ambiguous_write(self):
        valid = self.request()
        cases = [
            valid[:-1] + ',"side":"candidate"}',
            self.request(unknown="x"),
            self.request(domain="services"),
            self.request(operation="status"),
            self.request(operation=[]),
            self.request(write=False),
            valid.replace('"format_version":1', '"format_version":true'),
        ]
        for raw in cases:
            with self.subTest(raw=raw), self.assertRaises(FixtureControlProtocolError):
                self.arguments(raw)

    def test_rejects_public_argv_and_non_absolute_paths(self):
        with self.assertRaisesRegex(FixtureControlProtocolError, "不接受命令行参数"):
            self.arguments(argv=["unexpected", self.request()])
        with self.assertRaisesRegex(FixtureControlProtocolError, "绝对路径"):
            self.arguments(self.request(environment="relative.json"))

    def test_private_request_program_rejects_argv_before_business_logic(self):
        script = Path(__file__).resolve().with_name("reference_fixture_request.py")
        result = subprocess.run(
            [sys.executable, "-X", "utf8", "-B", str(script), "untrusted-argv"],
            cwd=script.parent.parent,
            text=True,
            encoding="utf-8",
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("protocol_error", result.stderr)

if __name__ == "__main__":
    unittest.main()
