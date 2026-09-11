"""当前来源和比较 CLI；同代生命周期与副作用由专门测试覆盖。"""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_build
import restore_reference
import restore_source as source
from restore_reference_fixture import environment


class SourceCliTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = restore_reference.work_directory(self.plan)

    def write(self, name, value):
        path = self.work / name
        restore_reference.write_json(path, value)
        return path

    def test_source_cli_requires_write_and_delegates_only_generation_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        args = ["restore_source", "verify", "--backend-dir", str(self.backend),
                "--source-generation", str(start), "--output", str(output)]
        with patch.object(sys, "argv", args), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(source, "execute_source_verification") as verify, \
                self.assertRaises(SystemExit) as error:
            source.main()
        self.assertEqual(error.exception.code, 2)
        verify.assert_not_called()
        result = {"output": str(output), "status": "source_runtime_verified"}
        with patch.object(sys, "argv", [*args, "--write"]), patch.object(
                source, "execute_source_verification", return_value=result) as verify, \
                patch("builtins.print") as printed:
            source.main()
        verify.assert_called_once_with(self.backend.resolve(), start, output)
        printed.assert_called_once_with(json.dumps(result))

    def test_cli_failure_propagates_without_creating_legacy_lock_or_success_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        args = ["restore_source", "verify", "--backend-dir", str(self.backend),
                "--source-generation", str(start), "--output", str(output), "--write"]
        failure = subprocess.CalledProcessError(1, ["node"], stderr=b"preserved protocol failure")
        with patch.object(sys, "argv", args), patch.object(
                source, "execute_source_verification", side_effect=failure
        ), self.assertRaises(subprocess.CalledProcessError):
            source.main()
        self.assertFalse(output.exists())
        self.assertFalse((self.work / ".reference-lock").exists())

    def test_comparison_capture_requires_write_and_binds_the_exact_export_result(self):
        export = self.write("source-export-result.json", {"published": True})
        output = self.work / "comparison-sources.json"
        roots = {
            "b0-backend": self.backend / "b0-backend",
            "b0-adapter-backend": self.backend / "b0-adapter",
            "b0-frontend": self.backend / "b0-frontend",
            "b1-backend": self.backend / "b1-backend",
            "b1-frontend": self.backend / "b1-frontend",
        }
        builds = {
            "b0-backend-build": self.backend / "b0-backend.json",
            "b0-frontend-build": self.backend / "b0-frontend.json",
            "b1-backend-build": self.backend / "b1-backend.json",
            "b1-frontend-build": self.backend / "b1-frontend.json",
        }
        args = ["restore_source", "comparison-capture", "--backend-dir", str(self.backend)]
        for name, path in {**roots, **builds}.items():
            args.extend(["--" + name, str(path)])
        args.extend(["--source-export-result", str(export), "--output", str(output)])
        with patch.object(sys, "argv", args), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(source, "capture_comparison_sources") as capture, \
                self.assertRaises(SystemExit) as error:
            source.main()
        self.assertEqual(error.exception.code, 2)
        capture.assert_not_called()

        receipt = {"format_version": 1, "kind": "restore-comparison-sources"}
        with patch.object(sys, "argv", [*args, "--write"]), \
                patch.object(source, "validate_new_output", return_value=output) as validate, \
                patch.object(source, "capture_comparison_sources", return_value=receipt) as capture, \
                patch.object(source, "write_new") as write, contextlib.redirect_stdout(io.StringIO()):
            source.main()
        validate.assert_called_once_with(output, self.backend)
        self.assertEqual(capture.call_args.kwargs["source_export_result"], {
            "path": str(export), **restore_build.file_digest(export),
        })
        write.assert_called_once_with(output, receipt, self.backend)

    def test_comparison_verify_is_read_only_and_rejects_write_argument(self):
        receipt = self.write("comparison-sources.json", {
            "format_version": 1, "kind": "restore-comparison-sources",
        })
        result = {"source_export": {"identity_sha256": "a" * 64}}
        args = ["restore_source", "comparison-verify", "--backend-dir", str(self.backend),
                "--receipt", str(receipt)]
        with patch.object(sys, "argv", [*args, "--write"]), contextlib.redirect_stderr(io.StringIO()), \
                patch.object(source, "verify_comparison_sources") as verify, \
                self.assertRaises(SystemExit) as error:
            source.main()
        self.assertEqual(error.exception.code, 2)
        verify.assert_not_called()
        with patch.object(sys, "argv", args), \
                patch.object(source, "verify_comparison_sources", return_value=result) as verify, \
                patch.object(source, "write_new") as write, contextlib.redirect_stdout(io.StringIO()):
            source.main()
        verify.assert_called_once_with(self.backend, json.loads(receipt.read_text(encoding="utf-8")))
        write.assert_not_called()



if __name__ == "__main__":
    unittest.main()
