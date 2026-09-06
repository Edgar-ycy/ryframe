"""短暂源码检查窗口只复用已核验内容，不以文件时间代替实际摘要。"""
import os
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_tools
import devex_provenance
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
        (self.root / "scripts").mkdir()
        (self.root / "Cargo.toml").write_bytes(b"manifest")
        self.tool = self.root / "scripts/check.py"
        self.tool.write_bytes(b"tool")
        self.head, self.index = "a" * 40, b"100644 fixed index"
        self.snapshot = {"head": self.head, "clean": False, "patch_sha256": "b" * 64, "files": []}
        self.enterContext(patch.object(source_inventory, "source_snapshot", side_effect=lambda _: dict(self.snapshot)))
        self.enterContext(patch.object(source_inventory, "worktree_fingerprint", return_value="sha256:" + "c" * 64))
        self.enterContext(patch.object(source_inventory, "git", side_effect=self.git))

    def git(self, _root, *args):
        if args == ("rev-parse", "HEAD"):
            return self.head.encode()
        if args == ("ls-files", "--stage", "-z"):
            return self.index
        if args == ("ls-files", "--cached", "--others", "--exclude-standard", "-z"):
            files = [path.relative_to(self.root).as_posix().encode() for path in self.root.rglob("*")
                     if path.relative_to(self.root).parts[0] != ".local-tests" and (path.is_file() or path.is_symlink())]
            return b"\0".join(sorted(files)) + b"\0"
        raise AssertionError(args)

    def test_no_stage_does_not_add_git_requirements_or_cache_independent_calls(self):
        with patch.object(sources, "capture_inventory", side_effect=AssertionError("extra Git")), sources.source_check(self.root):
            self.assertIsNone(sources.checked_source(self.root))
            with patch.object(devex_clone_tools, "source_snapshot", return_value=self.snapshot) as snapshot:
                value = devex_clone_tools.source_binding(self.root)
                self.assertEqual(value["snapshot"], self.snapshot)
                snapshot.assert_called_once_with(self.root)

    def test_nested_checks_share_one_content_scan_and_consumers_use_current_complete_source(self):
        original = sources.capture_inventory(self.root)
        receipt = {"kind": "restore-backend-build", "source": original["source"]["snapshot"], "source_inventory": original}
        with sources.artifact_sources(self.root, []) as stage:
            with patch.object(sources, "file_inventory", wraps=sources.file_inventory) as scan, sources.source_check(self.root):
                with sources.source_check(self.root):
                    for _ in range(3):
                        with patch.object(devex_clone_tools, "source_snapshot", side_effect=AssertionError("repeated Git")), \
                                patch.object(sources, "current_execution_source", side_effect=AssertionError("repeated capture")):
                            binding = devex_clone_tools.source_binding(self.root)
                            self.assertEqual(binding["snapshot"], stage["snapshot"])
                            self.assertEqual(devex_provenance.verify_source(self.root, receipt,
                                             original["source"]["worktree_fingerprint"]), receipt["source"])
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
        added = self.root / "scripts/added.py"
        renamed = self.root / "scripts/renamed.py"
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
        with sources.artifact_sources(self.root, []):
            for field, changed in (("head", "f" * 40), ("index", b"100755 same content")):
                old = getattr(self, field)
                setattr(self, field, changed)
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, "Git 模式"), sources.source_check(self.root):
                    pass
                setattr(self, field, old)

    def test_worktree_executable_mode_change_is_detected_even_with_identical_content(self):
        original = Path.lstat
        def changed(path):
            value = original(path)
            if path != self.tool:
                return value
            fields = list(value)
            fields[0] ^= 0o111
            return os.stat_result(fields)
        with sources.artifact_sources(self.root, []):
            with patch.object(Path, "lstat", autospec=True, side_effect=changed):
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

    def test_link_or_outside_resolved_path_is_rejected(self):
        original = Path.resolve
        with sources.artifact_sources(self.root, []):
            with patch.object(Path, "resolve", autospec=True, side_effect=lambda value, *args, **kwargs:
                              self.root.parent / "outside" if value == self.tool else original(value, *args, **kwargs)):
                with self.assertRaisesRegex(ValueError, "越界"), sources.source_check(self.root):
                    pass


if __name__ == "__main__":
    unittest.main()
