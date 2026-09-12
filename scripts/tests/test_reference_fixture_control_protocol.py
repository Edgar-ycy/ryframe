import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import reference_fixture_environment as fixture_environment
import reference_fixture_request as fixture_request
import reference_fixture_review as fixture_review
import reference_fixture_services as fixture_services
import reference_fixture_successor as fixture_successor
from reference_fixture_control_protocol import (
    FixtureControlProtocolError,
    PROTOCOL_KEY,
    private_arguments,
)


KIND = "ryframe-reference-fixture-control"


class FixtureControlProtocolTests(unittest.TestCase):
    def setUp(self):
        self.root = str(Path(__file__).resolve().parents[2])

    def protocol(self, domain, operation, write, **fields):
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
            argv=[],
            environment={PROTOCOL_KEY: raw},
        )

    def test_reconstructs_exact_operations_for_all_five_domains(self):
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
        for raw in cases:
            with self.subTest(raw=raw), self.assertRaises(FixtureControlProtocolError):
                self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, raw, positional=True)
        missing_output = self.protocol(
            "successor", "generation-request", True,
            successor=path, source_backend=str(Path(self.root) / ".local-tests"),
            expected_head="a" * 40, backend_build=path, maintenance_build=path,
            source_environment=path, id="source-r24",
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
                positional_operation=True, argv=["status"], environment={PROTOCOL_KEY: raw},
            )
        with self.assertRaisesRegex(FixtureControlProtocolError, "未知字段"):
            private_arguments(
                "services", fixture_services.PROTOCOL_SCHEMAS, positional_operation=True, argv=[],
                environment={PROTOCOL_KEY: raw, "RYFRAME_REFERENCE_FIXTURE_CONTROL_EXTRA": "x"},
            )
        relative = self.protocol(
            "services", "status", False, review="relative.json", environment=path
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "绝对路径"):
            self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, relative, positional=True)

    def test_all_direct_python_programs_reject_argv_before_business_logic(self):
        scripts = [
            "reference_fixture_environment.py",
            "reference_fixture_review.py",
            "reference_fixture_request.py",
            "reference_fixture_successor.py",
            "reference_fixture_services.py",
        ]
        environment = dict(os.environ)
        environment.pop(PROTOCOL_KEY, None)
        for name in scripts:
            script = Path(__file__).resolve().parents[1] / name
            with self.subTest(script=name):
                result = subprocess.run(
                    [sys.executable, "-B", str(script), "untrusted-argv"],
                    cwd=script.parent.parent,
                    env=environment,
                    text=True,
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 2)
                self.assertIn("protocol_error", result.stderr)


if __name__ == "__main__":
    unittest.main()
