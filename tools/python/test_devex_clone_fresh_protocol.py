"""fresh-target 的 xtask 私有协议与旧 argv 拒绝回归。"""
from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone
import devex_clone_run_cli as cli


class FreshTargetProtocolTests(unittest.TestCase):
    def payload(self):
        backend = Path(__file__).resolve().parents[2]
        return {
            "format_version": 1,
            "kind": cli.FRESH_TARGET_PROTOCOL_KIND,
            "request": {
                "backend_dir": str(backend),
                "operation": "status",
                "workspace": str(backend / ".local-tests/fresh protocol"),
                "request": None,
                "environment": None,
                "storage_run": None,
                "observation_dir": None,
                "write": False,
            },
        }

    def test_decoder_produces_typed_namespace_from_exact_versioned_document(self):
        backend, request = cli.decode_fresh_target_protocol(json.dumps(self.payload()))
        self.assertTrue(backend.is_absolute())
        self.assertEqual(request.command, "fresh-target")
        self.assertEqual(request.operation, "status")
        self.assertTrue(request.workspace.is_absolute())
        self.assertFalse(request.write)
        self.assertIsNone(request.request)

    def test_decoder_rejects_unknown_duplicate_and_invalid_typed_fields(self):
        unknown = self.payload()
        unknown["extra"] = True
        wrong_write = self.payload()
        wrong_write["request"]["write"] = 0
        boolean_version = self.payload()
        boolean_version["format_version"] = True
        relative = self.payload()
        relative["request"]["workspace"] = ".local-tests/fresh"
        newline = self.payload()
        newline["request"]["workspace"] += "\nother"
        duplicate = (
            '{"format_version":1,"format_version":1,'
            f'"kind":"{cli.FRESH_TARGET_PROTOCOL_KIND}","request":{{}}}}'
        )
        for source in (
                "{}", json.dumps(unknown), json.dumps(wrong_write), json.dumps(boolean_version),
                json.dumps(relative), json.dumps(newline), duplicate, "not-json"):
            with self.subTest(source=source), self.assertRaises(cli.FreshTargetProtocolError):
                cli.decode_fresh_target_protocol(source)

    def test_private_protocol_accepts_no_argv_and_preserves_domain_failure_code(self):
        source = json.dumps(self.payload())
        output = StringIO()

        def dispatched(_request, _backend):
            self.assertNotIn(cli.FRESH_TARGET_PROTOCOL_ENV, os.environ)
            return {"status": "fresh_target_unregistered"}

        with patch.dict(os.environ, {cli.FRESH_TARGET_PROTOCOL_ENV: source}), \
                patch.object(cli, "dispatch", side_effect=dispatched) as dispatch, \
                redirect_stdout(output):
            self.assertEqual(devex_clone.main([]), 0)
        dispatch.assert_called_once()
        self.assertEqual(json.loads(output.getvalue())["status"], "fresh_target_unregistered")

        with patch.dict(os.environ, {cli.FRESH_TARGET_PROTOCOL_ENV: source}), \
                patch.object(cli, "dispatch", side_effect=ValueError("domain")), \
                redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main([]), 1)

    def test_private_protocol_rejects_argv_and_malformed_input_with_code_two(self):
        source = json.dumps(self.payload())
        with patch.dict(os.environ, {cli.FRESH_TARGET_PROTOCOL_ENV: source}), \
                patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main(["status"]), 2)
        dispatch.assert_not_called()
        with patch.dict(os.environ, {cli.FRESH_TARGET_PROTOCOL_ENV: "{}"}), \
                patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main([]), 2)
        dispatch.assert_not_called()

    def test_old_fresh_target_argv_is_not_a_private_program_entry(self):
        with patch.dict(os.environ, {}, clear=True), redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main(["fresh-target", "--workspace", "ignored"]), 2)


if __name__ == "__main__":
    unittest.main()
