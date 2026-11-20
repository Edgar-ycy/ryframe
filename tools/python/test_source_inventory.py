"""来源清单临时适配的协议、身份入口和返回类型测试；算法覆盖由 Rust 承担。"""
import hashlib
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_inventory as sources
import native_s3_client


class SourceInventoryAdapterTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(sources, "tool_binding", return_value={"fixture": True}))

    def test_missing_binding_rejects_before_transport_or_cargo_fallback(self):
        with patch.object(sources, "tool_binding", return_value=None), \
                patch.object(native_s3_client, "_invoke") as invoke, self.assertRaisesRegex(ValueError, "工具绑定"):
            sources.source_domain("Cargo.toml")
        invoke.assert_not_called()

    def test_native_rejection_is_propagated_without_python_fallback(self):
        response = {"ok": False, "error": {"category": "SourceEvidenceRejected",
            "message": "源码路径不能包含符号链接或 junction", "remote_write_may_have_occurred": False,
            "automatic_retry_permitted": False}}
        with patch.object(native_s3_client, "_invoke", return_value=(SimpleNamespace(returncode=1), response)), \
                self.assertRaisesRegex(ValueError, "junction"):
            sources.source_domain("nested/file")

    def test_transport_failure_propagates_without_git_or_algorithm_fallback(self):
        with patch.object(native_s3_client, "_invoke", side_effect=OSError("fixture unavailable")), \
                patch.object(sources, "git") as git, self.assertRaises(OSError):
            sources.capture_inventory(Path("D:/source"))
        git.assert_not_called()

    def test_shared_canonical_digest_preserves_utf8_compact_sorted_keys(self):
        value = [{"sha256": "a" * 64, "path": "中文 来源.rs"}]
        raw = ('[{"path":"中文 来源.rs","sha256":"' + "a" * 64 + '"}]').encode("utf-8")
        self.assertEqual(sources.canonical_digest(value), hashlib.sha256(raw).hexdigest())

    def test_shared_git_helper_retains_arguments_and_disables_optional_locks(self):
        source = Path("D:/source")
        with patch.object(subprocess, "check_output", return_value=b"raw\x00") as run:
            self.assertEqual(sources.git(source, "ls-files", "-z"), b"raw\x00")
        self.assertEqual(run.call_args.args[0], ["git", "-C", str(source), "ls-files", "-z"])
        self.assertEqual(run.call_args.kwargs["env"]["GIT_OPTIONAL_LOCKS"], "0")


if __name__ == "__main__":
    unittest.main()
