"""只读来源核心的编码、路径、完整输入和竞争校验；Git 使用固定响应替身。"""
import copy
import hashlib
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import sys
from types import SimpleNamespace
import unittest
import uuid
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_inventory as sources


class SourceInventoryTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = base / f"source-inventory-{uuid.uuid4()}"
        temporary.mkdir()
        self.addCleanup(lambda: shutil.rmtree(temporary, ignore_errors=True))
        self.root = temporary.resolve() / "中文 来源"
        self.root.mkdir()
        self.head, self.patch, self.index = "a" * 40, b"binary diff", b"index fixture"
        self.untracked = ["新增 文件.txt"]
        self.tracked = ["Cargo.toml", "README.md", "tools/python/check.py", "deleted.rs"]
        for relative in [*self.tracked[:-1], *self.untracked]:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(relative.encode("utf-8"))
        self.git_probe = self.enterContext(patch.object(sources, "git", side_effect=self.git))

    def git(self, root, *arguments):
        self.assertEqual(root, self.root)
        commands = {
            ("rev-parse", "--show-toplevel"): str(self.root).encode(),
            ("rev-parse", "HEAD"): self.head.encode(),
            ("diff", "--binary", "HEAD"): self.patch,
            ("diff", "--binary", "--no-ext-diff", "HEAD", "--", "."): self.patch,
            ("ls-files", "--others", "--exclude-standard", "-z"):
                ("\0".join(self.untracked) + "\0").encode(),
            ("ls-files", "--cached", "--others", "--exclude-standard", "-z"):
                ("\0".join([*self.tracked, *self.untracked]) + "\0").encode(),
            ("ls-files", "--stage", "-z"): self.index,
            ("status", "--porcelain", "--untracked-files=all"): b"?? untracked\n",
        }
        return commands[arguments]

    def test_snapshot_and_worktree_fingerprint_preserve_existing_encoding(self):
        value, binary_patch = sources.snapshot(self.root)
        self.assertEqual(binary_patch, self.patch)
        self.assertEqual(value, {
            "head": self.head, "patch_sha256": hashlib.sha256(self.patch).hexdigest(),
            "files": [{"path": name, "sha256": hashlib.sha256((self.root / name).read_bytes()).hexdigest()}
                      for name in self.untracked],
        })
        parts = [self.head.encode(), self.patch]
        for name in self.untracked:
            parts.extend([name.encode(), (self.root / name).read_bytes()])
        encoded = b"".join(struct.pack("<Q", len(value)) + value for value in parts)
        self.assertEqual(sources.worktree_fingerprint(self.root, self.head),
                         "sha256:" + hashlib.sha256(encoded).hexdigest())
        self.assertFalse(sources.source_snapshot(self.root)["clean"])

    def test_inventory_binds_tracked_untracked_and_deleted_files_with_git_guards(self):
        value = sources.capture_inventory(self.root)
        expected = sorted([*self.tracked[:-1], *self.untracked])
        self.assertEqual([item["path"] for item in value["files"]], expected)
        for item in value["files"]:
            self.assertEqual(item["sha256"], hashlib.sha256((self.root / item["path"]).read_bytes()).hexdigest())
        self.assertEqual(value["guard"]["head"], self.head)
        self.assertEqual(value["guard"]["index_sha256"], hashlib.sha256(self.index).hexdigest())
        self.assertEqual(value["source"]["snapshot"], sources.source_snapshot(self.root))
        self.assertEqual(sources.fingerprints(value)["test_tools"]["files"], 1)

    def test_content_changes_with_same_size_and_timestamp_change_fingerprint(self):
        path = self.root / self.untracked[0]
        path.write_bytes(b"first")
        before = sources.worktree_fingerprint(self.root, self.head)
        observed = path.stat()
        path.write_bytes(b"other")
        os.utime(path, ns=(observed.st_atime_ns, observed.st_mtime_ns))
        self.assertNotEqual(sources.worktree_fingerprint(self.root, self.head), before)

    def test_capture_rejects_snapshot_worktree_and_head_changes(self):
        source = sources.source_snapshot(self.root)
        guard = sources.file_inventory(self.root)
        fingerprint = "sha256:" + "b" * 64
        cases = [
            ([source, source | {"clean": True}], [fingerprint, fingerprint], guard),
            ([source, source], [fingerprint, "sha256:" + "c" * 64], guard),
            ([source, source], [fingerprint, fingerprint], guard | {"head": "d" * 40}),
        ]
        for snapshots, fingerprints, inventory in cases:
            with self.subTest(snapshots=snapshots, fingerprints=fingerprints), \
                    patch.object(sources, "source_snapshot", side_effect=snapshots), \
                    patch.object(sources, "worktree_fingerprint", side_effect=fingerprints), \
                    patch.object(sources, "file_inventory", return_value=inventory):
                with self.assertRaisesRegex(ValueError, "源码发生变化"):
                    sources.capture_inventory(self.root)

    def test_invalid_git_root_or_head_and_git_failure_cannot_be_clean_evidence(self):
        with patch.object(sources, "git", return_value=str(self.root / "other").encode()) as git:
            with self.assertRaises(ValueError):
                sources.source_snapshot(self.root)
            git.assert_called_once_with(self.root, "rev-parse", "--show-toplevel")
        self.head = "invalid"
        with self.assertRaises(ValueError):
            sources.snapshot(self.root)
        with patch.object(sources, "git", side_effect=subprocess.CalledProcessError(1, ["git"])):
            with self.assertRaises(subprocess.CalledProcessError):
                sources.source_snapshot(self.root)

    def test_path_escape_is_rejected_before_reading_file_content(self):
        for name in (".", "../outside", "C:/outside", "C:relative", "nested\\file", "/absolute"):
            self.untracked = [name]
            with self.subTest(name=name), patch.object(sources, "file_digest") as digest:
                for operation in (sources.snapshot, sources.file_inventory):
                    with self.assertRaises(ValueError):
                        operation(self.root)
                with self.assertRaises(ValueError):
                    sources.worktree_fingerprint(self.root, self.head)
                # 已跟踪文件可先被扫描；非法文件本身不得进入内容读取。
                self.assertTrue(all(call.args[0].is_relative_to(self.root) for call in digest.call_args_list))

    def test_intermediate_reparse_point_is_rejected_before_file_read(self):
        directory = self.root / "nested"
        directory.mkdir()
        (directory / "file.txt").write_text("fixture", encoding="utf-8")
        original_lstat = Path.lstat

        def lstat(path):
            observed = original_lstat(path)
            if path == directory:
                return SimpleNamespace(st_mode=stat.S_IFDIR, st_file_attributes=0x400)
            return observed

        with patch.object(Path, "lstat", autospec=True, side_effect=lstat), \
                self.assertRaisesRegex(ValueError, "junction"):
            sources.source_file(self.root, "nested/file.txt")

    def test_product_classification_stays_conservative_and_tools_remain_separate(self):
        for name in ("Cargo.lock", "config/app.toml", "vendor/lib.rs", "xtask/src/main.rs",
                     "tools/python/release_stage.py", "unknown/input.dat"):
            self.assertEqual(sources.source_domain(name), "product")
        for name in ("tools/python/check.py", "crates/app/tests/fixture.json", ".github/workflows/ci.yml"):
            self.assertEqual(sources.source_domain(name), "test_tools")
        self.assertEqual(sources.source_domain("README.md"), "support")
        files = [{"path": "Cargo.toml", "sha256": "a" * 64}, {"path": "tools/python/check.py", "sha256": "b" * 64}]
        before = sources.fingerprints({"files": files})
        changed = sources.fingerprints({"files": [files[0], files[1] | {"sha256": "c" * 64}]})
        self.assertEqual(before["product"], changed["product"])
        self.assertNotEqual(before["test_tools"], changed["test_tools"])

    def test_build_domains_keep_tool_only_changes_out_of_product_roles(self):
        before_inventory = sources.capture_inventory(self.root)
        after_inventory = copy.deepcopy(before_inventory)
        tool = next(item for item in after_inventory["files"] if item["path"] == "tools/python/check.py")
        tool["sha256"] = "f" * 64
        before = sources.build_source_domains(before_inventory, "backend")
        after = sources.build_source_domains(after_inventory, "backend")
        self.assertEqual(before["product"], after["product"])
        self.assertNotEqual(before["tools"], after["tools"])
        self.assertNotEqual(before["full"], after["full"])
        self.assertNotIn("tools/python/check.py", before["product"]["api"]["files"])
        self.assertNotIn("tools/python/check.py", before["product"]["worker"]["files"])
        inventory_with_classifier = copy.deepcopy(before_inventory)
        inventory_with_classifier["files"].append(
            {"path": "tools/python/source_inventory.py", "sha256": "9" * 64}
        )
        inventory_with_classifier["files"].sort(key=lambda item: item["path"])
        classified = sources.build_source_domains(inventory_with_classifier, "backend")
        self.assertIn("tools/python/source_inventory.py", classified["tools"]["files"])
        self.assertNotIn("tools/python/source_inventory.py", classified["product"]["api"]["files"])
        self.assertNotIn("tools/python/source_inventory.py", classified["product"]["worker"]["files"])

    def test_duplicate_unordered_or_malformed_inventory_is_rejected(self):
        first = {"path": "Cargo.toml", "sha256": "a" * 64}
        second = {"path": "tools/python/check.py", "sha256": "b" * 64}
        for files in ([first, first], [second, first], [first | {"sha256": "invalid"}],
                      [first | {"path": "../outside"}], [first | {"path": "C:/outside"}]):
            with self.subTest(files=files), self.assertRaises(ValueError):
                sources.fingerprints({"files": files})


if __name__ == "__main__":
    unittest.main()
