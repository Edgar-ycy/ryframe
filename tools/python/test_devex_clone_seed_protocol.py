"""正式 seed 来源链的 xtask 私有协议与直接进程边界回归。"""
from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from io import StringIO, BytesIO
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone
import devex_clone_run_cli as cli
import rate_limit_tool
from restore_build import file_digest

READ_ONLY_OPERATIONS = {"source-generation-status", "source-generation-reboot-status"}


class SeedSourceProtocolTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch("rate_limit_tool._COORDINATOR_BINDING", None))
        self.backend = Path(__file__).resolve().parents[2]
        self.run_dir = self.backend / ".local-tests/seed source protocol"
        self.request = self.run_dir / "generation 请求.json"

    def payload(self, operation="source-generation-status"):
        return {
            "format_version": 1,
            "kind": cli.SEED_SOURCE_PROTOCOL_KIND,
            "native_tool": {"format_version": 1, "kind": "ryframe-native-rate-limit-tool",
                            "path": str(Path(sys.executable).resolve()),
                            **file_digest(Path(sys.executable).resolve())},
            "request": {
                "backend_dir": str(self.backend),
                "operation": operation,
                "run_dir": str(self.run_dir),
                "request": str(self.request) if operation in cli.SEED_SOURCE_REQUEST_OPERATIONS else None,
                "write": operation not in READ_ONLY_OPERATIONS,
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
                self.assertEqual(request.write, operation not in READ_ONLY_OPERATIONS)
                self.assertEqual(
                    request.request,
                    self.request if operation in cli.SEED_SOURCE_REQUEST_OPERATIONS else None,
                )
                self.assertIsNone(request.producer_binding)
                self.assertEqual(request.native_tool, self.payload()["native_tool"])

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
            json.dumps(newline), duplicate, "not-json", self.encoded("source-register"),
        ):
            with self.subTest(source=source), patch.object(cli, "execute") as execute:
                with self.assertRaises(cli.SeedSourceProtocolError):
                    cli.decode_seed_source_protocol(source)
                execute.assert_not_called()
        with patch.object(cli, "execute") as execute, patch.object(cli, "evidence_path") as inputs:
            with self.assertRaisesRegex(ValueError, "Python seed-runtime 不接受此操作"):
                cli.dispatch(SimpleNamespace(command="seed-runtime", operation="source-register"), self.backend)
            execute.assert_not_called()
            inputs.assert_not_called()

    def test_private_protocol_preserves_success_domain_failure_and_removed_environment(self):
        source = self.encoded()
        output = StringIO()

        def dispatched(request, backend):
            self.assertEqual(backend, self.backend)
            self.assertNotIn(source, os.environ.values())
            return {"status": request.operation}

        with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(source.encode("utf-8")))), \
                patch("rate_limit_tool.install_binding", wraps=rate_limit_tool.install_binding) as install, \
                patch.object(cli, "dispatch", side_effect=dispatched) as dispatch, \
                redirect_stdout(output):
            self.assertEqual(devex_clone.main(["--private-stdin"]), 0)
        dispatch.assert_called_once()
        install.assert_called_once_with(json.dumps(self.payload()["native_tool"], ensure_ascii=False))
        self.assertEqual(json.loads(output.getvalue())["status"], "source-generation-status")

        with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(source.encode("utf-8")))), \
                patch("rate_limit_tool._COORDINATOR_BINDING", None), \
                patch.object(cli, "dispatch", side_effect=ValueError("domain")), \
                redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main(["--private-stdin"]), 1)

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
        def run(arguments=("--private-stdin",), protocol=None):
            return subprocess.run(
                [sys.executable, "-X", "utf8", "-B", str(script), *arguments], cwd=self.backend,
                input=protocol or "", capture_output=True, text=True, encoding="utf-8", check=False,
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
            run(protocol=self.encoded() + "{}"),
        ):
            self.assertEqual(result.returncode, 2, result.stderr)


if __name__ == "__main__":
    unittest.main()
