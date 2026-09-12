"""当前来源和比较业务派发；私有协议与公开参数由专门测试覆盖。"""
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


class SourceDispatchTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = restore_reference.work_directory(self.plan)

    def write(self, name, value):
        path = self.work / name
        restore_reference.write_json(path, value)
        return path

    def test_source_request_delegates_only_generation_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        request = source.SourceRequest("verify", {
            "backend_dir": self.backend,
            "source_generation": start,
            "output": output,
            "write": True,
        })
        result = {"output": str(output), "status": "source_runtime_verified"}
        with patch.object(
                source, "execute_source_verification", return_value=result) as verify, \
                patch("builtins.print") as printed:
            source.main(request)
        verify.assert_called_once_with(self.backend.resolve(), start, output)
        self.assertEqual(json.loads(printed.call_args.args[0]), result)

    def test_request_failure_preserves_no_lock_or_success_receipt(self):
        start = self.work / "results/start.json"
        output = self.work / "g0001/verification/source-runtime.json"
        request = source.SourceRequest("verify", {
            "backend_dir": self.backend,
            "source_generation": start,
            "output": output,
            "write": True,
        })
        failure = subprocess.CalledProcessError(1, ["node"], stderr=b"preserved protocol failure")
        with patch.object(
                source, "execute_source_verification", side_effect=failure
        ), self.assertRaises(subprocess.CalledProcessError):
            source.main(request)
        self.assertFalse(output.exists())
        self.assertFalse((self.work / ".reference-lock").exists())

    def test_comparison_capture_binds_the_exact_export_result(self):
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
        values = {
            name.replace("-", "_"): path
            for name, path in {**roots, **builds}.items()
        }
        values.update({
            "backend_dir": self.backend,
            "source_export_result": export,
            "output": output,
            "write": True,
        })
        request = source.SourceRequest("comparison-capture", values)
        receipt = {"format_version": 1, "kind": "restore-comparison-sources"}
        with patch.object(
            source, "validate_new_output", return_value=output
        ) as validate, patch.object(
            source, "capture_comparison_sources", return_value=receipt
        ) as capture, patch.object(
            source, "write_new"
        ) as write, patch("builtins.print"):
            source.main(request)
        validate.assert_called_once_with(output, self.backend)
        self.assertEqual(capture.call_args.kwargs["source_export_result"], {
            "path": str(export), **restore_build.file_digest(export),
        })
        write.assert_called_once_with(output, receipt, self.backend)

    def test_comparison_verify_is_read_only(self):
        receipt = self.write("comparison-sources.json", {
            "format_version": 1, "kind": "restore-comparison-sources",
        })
        result = {"source_export": {"identity_sha256": "a" * 64}}
        request = source.SourceRequest("comparison-verify", {
            "backend_dir": self.backend,
            "receipt": receipt,
            "write": False,
        })
        with patch.object(source, "verify_comparison_sources", return_value=result) as verify, \
                patch.object(source, "write_new") as write, patch("builtins.print"):
            source.main(request)
        verify.assert_called_once_with(
            self.backend, json.loads(receipt.read_text(encoding="utf-8"))
        )
        write.assert_not_called()



if __name__ == "__main__":
    unittest.main()
