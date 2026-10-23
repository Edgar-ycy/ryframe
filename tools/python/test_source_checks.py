"""短暂源码检查窗口只复用已核验内容，不以文件时间代替实际摘要。"""
import os
import subprocess
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_tools
import devex_provenance
import restore_build
import source_fingerprints as sources
import source_inventory

ROOT = Path(__file__).resolve().parents[2]


class SourceCheckTests(unittest.TestCase):
    def setUp(self):
        parent = ROOT / ".local-tests/python-unit"
        parent.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=parent)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "tools" / "python").mkdir(parents=True)
        (self.root / "Cargo.toml").write_bytes(b"manifest")
        self.tool = self.root / "tools/python/check.py"
        self.tool.write_bytes(b"tool")
        (self.root / ".gitignore").write_text(".local-tests/\n", encoding="utf-8")
        self.git("init", "--quiet")
        self.git("config", "core.autocrlf", "false")
        self.git("add", ".")
        self.git("-c", "user.name=source-unit", "-c", "user.email=source-unit@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "fixture")
        self.head = self.git("rev-parse", "HEAD").decode().strip()
        self.snapshot = source_inventory.source_snapshot(self.root)

    def git(self, *arguments):
        return subprocess.check_output(["git", "-C", str(self.root), *arguments])

    def test_no_stage_does_not_add_git_requirements_or_cache_independent_calls(self):
        with patch.object(sources, "capture_inventory", side_effect=AssertionError("extra Git")), sources.source_check(self.root):
            self.assertIsNone(sources.checked_source(self.root))
            with patch.object(devex_clone_tools, "source_snapshot", return_value=self.snapshot) as snapshot:
                value = devex_clone_tools.source_binding(self.root)
                self.assertEqual(value["snapshot"], self.snapshot)
                snapshot.assert_called_once_with(self.root)

    def test_nested_checks_share_one_content_scan_and_consumers_use_current_complete_source(self):
        original = sources.capture_inventory(self.root)
        context = {"commands": {role: restore_build.build_command(role) for role in restore_build.ROLES},
                   "profile": "dev", "target": "fixture", "jobs": "cargo-default",
                   "toolchain": {"cargo": "fixture", "rustc": "fixture"},
                   "environment": {"variables": [], "sha256": source_inventory.canonical_digest([])}}
        receipt = {"format_version": 2, "kind": "restore-backend-build",
                   "sources": source_inventory.build_source_domains(original, "backend"),
                   "build": context, "artifacts": {}}
        with patch.object(restore_build, "build_context", return_value=context), \
                sources.artifact_sources(self.root, []) as stage:
            with patch.object(sources, "file_inventory", wraps=sources.file_inventory) as scan, sources.source_check(self.root):
                with sources.source_check(self.root):
                    for _ in range(3):
                        with patch.object(devex_clone_tools, "source_snapshot", side_effect=AssertionError("repeated Git")), \
                                patch.object(sources, "current_execution_source", side_effect=AssertionError("repeated capture")):
                            binding = devex_clone_tools.source_binding(self.root)
                            self.assertEqual(binding["snapshot"], stage["snapshot"])
                            self.assertEqual(devex_provenance.verify_source(self.root, receipt,
                                             original["source"]["worktree_fingerprint"]),
                                             original["source"]["snapshot"])
                self.assertEqual(scan.call_count, 1)
            self.assertIsNone(sources.checked_source(self.root))

    def test_same_length_and_restored_mtime_content_change_is_detected_in_next_window(self):
        with sources.artifact_sources(self.root, []):
            with sources.source_check(self.root):
                pass
            stamp = self.tool.stat()
            self.tool.write_bytes(b"evil")
            os.utime(self.tool, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            self.assertEqual(self.tool.stat().st_size, stamp.st_size)
            self.assertEqual(self.tool.stat().st_mtime_ns, stamp.st_mtime_ns)
            with self.assertRaisesRegex(ValueError, "内容"), sources.source_check(self.root):
                pass
            self.tool.write_bytes(b"tool")

    def test_file_add_delete_and_rename_each_invalidate_the_current_stage(self):
        added = self.root / "tools/python/added.py"
        renamed = self.root / "tools/python/renamed.py"
        for action in ("add", "delete", "rename"):
            with self.subTest(action=action), sources.artifact_sources(self.root, []):
                if action == "add":
                    added.write_bytes(b"new")
                elif action == "delete":
                    self.tool.unlink()
                else:
                    self.tool.rename(renamed)
                with self.assertRaisesRegex(ValueError, "集合"), sources.source_check(self.root):
                    pass
                if action == "add":
                    added.unlink()
                elif action == "delete":
                    self.tool.write_bytes(b"tool")
                else:
                    renamed.rename(self.tool)

    def test_head_and_index_modes_are_checked_without_git_diff(self):
        for field in ("head", "index"):
            with self.subTest(field=field), sources.artifact_sources(self.root, []):
                if field == "head":
                    self.git("-c", "user.name=source-unit", "-c", "user.email=source-unit@example.invalid",
                             "-c", "commit.gpgsign=false", "commit", "--quiet", "--allow-empty", "-m", "drift")
                else:
                    self.git("update-index", "--chmod=+x", "tools/python/check.py")
                try:
                    with self.assertRaisesRegex(ValueError, "Git 模式"), sources.source_check(self.root):
                        pass
                finally:
                    if field == "head":
                        self.git("update-ref", "HEAD", self.head)
                    else:
                        self.git("update-index", "--chmod=-x", "tools/python/check.py")

    def test_native_mode_guard_change_is_detected_even_with_identical_content(self):
        # Windows 不提供 POSIX 可执行位；此处验证调用方拒绝原生 guard 漂移。
        with sources.artifact_sources(self.root, []):
            observed = sources.file_inventory(self.root)
            observed["modes_sha256"] = "0" * 64
            with patch.object(sources, "file_inventory", return_value=observed):
                with self.assertRaisesRegex(ValueError, "Git 模式"), sources.source_check(self.root):
                    pass

    def test_ignored_evidence_does_not_change_source_and_returned_values_cannot_mutate_cached_proof(self):
        with sources.artifact_sources(self.root, []) as stage:
            stage["snapshot"]["head"] = "f" * 40
            ignored = self.root / ".local-tests"
            ignored.mkdir()
            (ignored / "evidence.json").write_text("{}")
            with sources.source_check(self.root) as checked:
                self.assertEqual(checked["snapshot"]["head"], self.head)
                checked["snapshot"]["head"] = "f" * 40
                self.assertEqual(sources.checked_source(self.root)["snapshot"]["head"], self.head)

    def test_cross_root_nested_context_and_exception_cleanup_cannot_leak_cached_proof(self):
        with sources.artifact_sources(self.root, []):
            with self.assertRaisesRegex(ValueError, "当前验收"), sources.source_check(self.root / "other"):
                pass
            with sources.source_check(self.root):
                with self.assertRaisesRegex(ValueError, "跨仓库"), sources.source_check(self.root / "other"):
                    pass
            with self.assertRaisesRegex(RuntimeError, "fixture"), sources.source_check(self.root):
                raise RuntimeError("fixture")
            self.assertIsNone(sources.checked_source(self.root))
        self.assertIsNone(sources.checked_source(self.root))

    def test_complete_inventory_is_checked_at_stage_exit_even_without_another_guard(self):
        with self.assertRaisesRegex(ValueError, "阶段结束"), sources.artifact_sources(self.root, []):
            with sources.source_check(self.root):
                pass
            self.tool.write_bytes(b"evil")
        self.assertIsNone(sources.checked_source(self.root))
        with sources.source_check(self.root):
            self.assertIsNone(sources.checked_source(self.root))

    def test_native_path_rejection_propagates_without_cached_proof(self):
        # 路径链实际拒绝由原生 symlink/junction 测试覆盖，调用方不得吞掉拒绝。
        with sources.artifact_sources(self.root, []):
            with patch.object(sources, "file_inventory", side_effect=ValueError("源码文件越界")):
                with self.assertRaisesRegex(ValueError, "越界"), sources.source_check(self.root):
                    pass
            self.assertIsNone(sources.checked_source(self.root))


if __name__ == "__main__":
    unittest.main()
