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

import reference_fixture_environment as fixture_environment
import reference_fixture_request as fixture_request
import reference_fixture_review as fixture_review
import reference_fixture_services as fixture_services
import reference_fixture_source_pair as fixture_source_pair
import reference_fixture_successor as fixture_successor
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
        raw = self.protocol(
            "review", "renew", True,
            template=self.root + "/.local-tests/template.json",
            fixture=self.root + "/.local-tests/fixture.json",
            future_root=self.root + "/.local-tests/future",
            id="review-test",
            api_port=18000,
            worker_port=18003,
            frontend_port=18006,
            rustfs_api_port=19000,
            rustfs_console_port=19001,
            redis_port=16000,
            output=self.root + "/.local-tests/output.json",
        )
        output = io.StringIO()

        def fail(_args):
            raise ValueError("计划来源错误：secret-for-test")
        with (
            mock.patch.dict(os.environ, {"APP_PASSWORD": "secret-for-test"}),
            mock.patch.object(sys, "argv", ["review.py"]),
            mock.patch.object(sys, "stderr", output),
        ):
            result = run_private(
                "review", fixture_review.PROTOCOL_SCHEMAS, fail,
                positional_operation=False, raw=raw,
            )
        self.assertEqual(result, 1)
        self.assertIn("计划来源错误", output.getvalue())
        self.assertIn("[REDACTED]", output.getvalue())
        self.assertNotIn("secret-for-test", output.getvalue())

    def setUp(self):
        self.root = str(Path(__file__).resolve().parents[2])

    def protocol(self, domain, operation, write, **fields):
        if domain in {"environment", "request", "services", "successor"}:
            fields.setdefault("native_tool", json.dumps(tool_binding()))
        return json.dumps({
            "backend_dir": self.root,
            "domain": domain,
            "format_version": 1,
            "kind": KIND,
            "operation": operation,
            "write": write,
            **fields,
        }, separators=(",", ":"))

    def arguments(self, domain, schemas, raw, *, positional):
        return private_arguments(
            domain,
            schemas,
            positional_operation=positional,
            raw=raw,
            argv=[],
        )

    def test_stdin_protocol_is_bounded_and_strict_utf8(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        raw = self.protocol(
            "services", "status", False, review=path, environment=path
        )
        with mock.patch.object(sys, "stdin", self.BinaryInput(raw.encode("utf-8"))):
            arguments = private_arguments(
                "services",
                fixture_services.PROTOCOL_SCHEMAS,
                positional_operation=True,
                argv=[],
            )
        self.assertEqual(arguments[0], "status")
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
                private_arguments(
                    "services",
                    fixture_services.PROTOCOL_SCHEMAS,
                    positional_operation=True,
                    argv=[],
                )

    def test_retired_python_generation_request_cannot_dispatch_a_second_producer(self):
        receipt = str(Path(self.root) / ".local-tests" / "input.json")
        raw = self.protocol("successor", "generation-request", False, successor=receipt,
                            source_backend=self.root, expected_head="a" * 40,
                            backend_build=receipt, maintenance_build=receipt,
                            source_environment=receipt, id="source-current")
        with self.assertRaises(FixtureControlProtocolError):
            self.arguments("successor", fixture_successor.PROTOCOL_SCHEMAS, raw, positional=True)

    def test_reconstructs_exact_operations_for_control_domains(self):
        input_path = str(Path(self.root) / ".local-tests" / "input.json")
        directory = str(Path(self.root) / ".local-tests" / "run")
        environment = self.protocol(
            "environment", "plan", False,
            review=input_path, fixture=input_path, maintenance_build=input_path, side="seed",
        )
        self.assertEqual(
            self.arguments("environment", fixture_environment.PROTOCOL_SCHEMAS, environment, positional=True)[0],
            "plan",
        )
        review = self.protocol(
            "review", "renew", True,
            template=input_path, fixture=input_path, future_root=directory, id="r24-current",
            api_port=18230, worker_port=19230, frontend_port=4200, rustfs_api_port=29210,
            rustfs_console_port=29211, redis_port=16391, output=input_path,
        )
        review_args = self.arguments(
            "review", fixture_review.PROTOCOL_SCHEMAS, review, positional=False
        )
        self.assertEqual(review_args[:2], ["--backend-dir", self.root])
        self.assertNotIn("renew", review_args)
        request = self.protocol(
            "request", "publish", True,
            environment=input_path, service_run=directory, id="fresh-base", side="base", output=input_path,
        )
        self.assertNotIn(
            "publish",
            self.arguments("request", fixture_request.PROTOCOL_SCHEMAS, request, positional=False),
        )
        source_pair = self.protocol(
            "source-pair", "publish", True, output=input_path
        )
        self.assertEqual(
            self.arguments(
                "source-pair",
                fixture_source_pair.PROTOCOL_SCHEMAS,
                source_pair,
                positional=False,
            ),
            ["--backend-dir", self.root, "--output", input_path, "--write"],
        )
        services = self.protocol(
            "services", "status", False, review=input_path, environment=input_path
        )
        self.assertEqual(
            self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, services, positional=True)[0],
            "status",
        )
        successor = self.protocol(
            "successor", "arm-request", False,
            successor=input_path, source_export_result=input_path, workspace=directory,
            id="arm-base", side="base", copy_directory=directory,
        )
        self.assertEqual(
            self.arguments("successor", fixture_successor.PROTOCOL_SCHEMAS, successor, positional=True)[0],
            "arm-request",
        )

    def test_rejects_duplicate_unknown_wrong_domain_and_ambiguous_write(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        valid = self.protocol(
            "services", "status", False, review=path, environment=path
        )
        cases = [
            valid[:-1] + ',"review":"' + path.replace("\\", "\\\\") + '"}',
            self.protocol("services", "status", False, review=path, environment=path, unknown="x"),
            self.protocol("request", "status", False, review=path, environment=path),
            self.protocol("services", "status", True, review=path, environment=path),
            valid.replace('"format_version":1', '"format_version":true'),
        ]
        non_text_operation = json.loads(valid)
        non_text_operation["operation"] = []
        cases.append(json.dumps(non_text_operation, separators=(",", ":")))
        for raw in cases:
            with self.subTest(raw=raw), self.assertRaises(FixtureControlProtocolError):
                self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, raw, positional=True)
        missing_output = self.protocol(
            "successor", "arm-request", True, successor=path, source_export_result=path,
            workspace=self.root, id="source-r24", side="base", copy_directory=self.root,
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "写入授权"):
            self.arguments(
                "successor", fixture_successor.PROTOCOL_SCHEMAS,
                missing_output, positional=True,
            )

    def test_rejects_public_argv_related_environment_and_non_absolute_paths(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        raw = self.protocol(
            "services", "status", False, review=path, environment=path
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "不接受命令行参数"):
            private_arguments(
                "services", fixture_services.PROTOCOL_SCHEMAS,
                positional_operation=True, argv=["status"],
            )
        with self.assertRaisesRegex(FixtureControlProtocolError, "不接受命令行参数"):
            private_arguments(
                "services", fixture_services.PROTOCOL_SCHEMAS, positional_operation=True,
                argv=["unexpected", raw],
            )
        relative = self.protocol(
            "services", "status", False, review="relative.json", environment=path
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "绝对路径"):
            self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, relative, positional=True)


    def test_all_direct_python_programs_reject_argv_before_business_logic(self):
        scripts = [
            "full_stack_ci_support.py",
            "reference_fixture_environment.py",
            "reference_fixture_review.py",
            "reference_fixture_request.py",
            "reference_fixture_successor.py",
            "reference_fixture_services.py",
            "reference_fixture_source_pair.py",
        ]
        for name in scripts:
            script = Path(__file__).resolve().parent / name
            with self.subTest(script=name):
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
