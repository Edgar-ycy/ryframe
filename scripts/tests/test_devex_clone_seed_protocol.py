"""正式 seed 来源链的 xtask 私有协议与直接进程边界回归。"""
from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import devex_clone
import devex_clone_run_cli as cli


class SeedSourceProtocolTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.run_dir = self.backend / ".local-tests/seed source protocol"
        self.request = self.run_dir / "generation 请求.json"

    def payload(self, operation="source-generation-status"):
        return {
            "format_version": 1,
            "kind": cli.SEED_SOURCE_PROTOCOL_KIND,
            "request": {
                "backend_dir": str(self.backend),
                "operation": operation,
                "run_dir": str(self.run_dir),
                "request": str(self.request) if operation in cli.SEED_SOURCE_REQUEST_OPERATIONS else None,
                "write": operation != "source-generation-status",
            },
        }

    def encoded(self, operation="source-generation-status"):
        return json.dumps(self.payload(operation))

    def test_decoder_maps_all_operations_to_the_existing_typed_namespace(self):
        for operation in sorted(cli.SEED_SOURCE_OPERATIONS):
            with self.subTest(operation=operation):
                backend, request = cli.decode_seed_source_protocol(self.encoded(operation))
                self.assertEqual(backend, self.backend)
                self.assertEqual(request.command, "seed-runtime")
                self.assertEqual(request.operation, operation)
                self.assertEqual(request.run_dir, self.run_dir)
                self.assertEqual(request.write, operation != "source-generation-status")
                self.assertEqual(
                    request.request,
                    self.request if operation in cli.SEED_SOURCE_REQUEST_OPERATIONS else None,
                )
                self.assertIsNone(request.producer_binding)

    def test_decoder_rejects_unknown_duplicate_bad_types_and_cross_operation_fields(self):
        unknown = json.loads(self.encoded())
        unknown["unknown"] = True
        wrong_write = json.loads(self.encoded())
        wrong_write["request"]["write"] = 0
        wrong_version = json.loads(self.encoded())
        wrong_version["format_version"] = True
        missing_request = json.loads(self.encoded("source-rebind"))
        missing_request["request"]["request"] = None
        extra_request = json.loads(self.encoded("source-export"))
        extra_request["request"]["request"] = str(self.request)
        relative = json.loads(self.encoded())
        relative["request"]["run_dir"] = ".local-tests/source"
        newline = json.loads(self.encoded())
        newline["request"]["run_dir"] += "\nother"
        duplicate = self.encoded().replace(
            '"operation": "source-generation-status"',
            '"operation": "source-generation-status", "operation": "source-generation-status"',
        )
        for source in (
            "{}", json.dumps(unknown), json.dumps(wrong_write), json.dumps(wrong_version),
            json.dumps(missing_request), json.dumps(extra_request), json.dumps(relative),
            json.dumps(newline), duplicate, "not-json",
        ):
            with self.subTest(source=source), self.assertRaises(cli.SeedSourceProtocolError):
                cli.decode_seed_source_protocol(source)

    def test_private_protocol_preserves_success_domain_failure_and_removed_environment(self):
        source = self.encoded()
        output = StringIO()

        def dispatched(request, backend):
            self.assertEqual(backend, self.backend)
            self.assertNotIn(cli.SEED_SOURCE_PROTOCOL_ENV, os.environ)
            return {"status": request.operation}

        with patch.dict(os.environ, {cli.SEED_SOURCE_PROTOCOL_ENV: source}), \
                patch.object(cli, "dispatch", side_effect=dispatched) as dispatch, \
                redirect_stdout(output):
            self.assertEqual(devex_clone.main([]), 0)
        dispatch.assert_called_once()
        self.assertEqual(json.loads(output.getvalue())["status"], "source-generation-status")

        with patch.dict(os.environ, {cli.SEED_SOURCE_PROTOCOL_ENV: source}), \
                patch.object(cli, "dispatch", side_effect=ValueError("domain")), \
                redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main([]), 1)

    def test_explicit_main_argv_is_rejected(self):
        arguments = [
            "seed-runtime", "--backend-dir", str(self.backend), "--run-dir", str(self.run_dir),
            "--operation", "source-register", "--write",
        ]
        with patch.dict(os.environ, {}, clear=True), patch.object(cli, "execute") as execute, \
                redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main(arguments), 2)
        execute.assert_not_called()

    def test_direct_process_uses_only_the_private_protocol_and_stable_exit_codes(self):
        script = Path(devex_clone.__file__).resolve()
        environment = {
            key: value for key, value in os.environ.items()
            if key not in {cli.SEED_SOURCE_PROTOCOL_ENV, cli.FRESH_TARGET_PROTOCOL_ENV}
        }

        def run(arguments=(), protocol=None, extra=None):
            current = dict(environment)
            if protocol is not None:
                current[cli.SEED_SOURCE_PROTOCOL_ENV] = protocol
            current.update(extra or {})
            return subprocess.run(
                [sys.executable, "-B", str(script), *arguments], cwd=self.backend,
                env=current, capture_output=True, text=True, check=False,
            )

        valid = run(protocol=self.encoded())
        self.assertEqual(valid.returncode, 1, valid.stderr)
        self.assertIn("开发复制未完成", valid.stderr)
        repeated = self.encoded().replace(
            '"operation": "source-generation-status"',
            '"operation": "source-generation-status", "operation": "source-generation-status"',
        )
        unknown = self.payload()
        unknown["unknown"] = True
        for result in (
            run(("seed-runtime", "--backend-dir", str(self.backend), "--run-dir", str(self.run_dir),
                 "--operation", "source-register", "--write")),
            run(protocol="{}"),
            run(protocol=repeated),
            run(protocol=json.dumps(unknown)),
            run(protocol=self.encoded(), extra={cli.FRESH_TARGET_PROTOCOL_ENV: "{}"}),
        ):
            self.assertEqual(result.returncode, 2, result.stderr)


if __name__ == "__main__":
    unittest.main()
