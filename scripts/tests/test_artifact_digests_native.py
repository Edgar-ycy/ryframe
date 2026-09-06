"""用临时 PE 副本验证真实 Windows 文件共享约束，不访问业务服务。"""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import mmap
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import artifact_digests as frozen
from artifact_digests import file_digest


@unittest.skipUnless(os.name == "nt", "需要真实 Windows 文件共享语义")
class NativeDigestTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.directory = self.root / "binaries"
        self.directory.mkdir()
        self.path = self.directory / "read-only.exe"
        shutil.copyfile(Path(os.environ["SystemRoot"]) / "System32/where.exe", self.path)
        self.original = self.path.read_bytes()
        self.value = {"path": str(self.path), "sha256": hashlib.sha256(self.original).hexdigest()}
        self.addCleanup(self.assertIsNone, frozen._ACTIVE)

    def writable(self, path=None):
        with (path or self.path).open("r+b") as stream:
            stream.seek(-1, 2)
            stream.write(b"Z")

    def test_real_write_delete_replace_rename_rejected_but_read_and_execution_work(self):
        other = self.directory / "replacement.exe"
        shutil.copyfile(self.path, other)
        with frozen.protect_binaries([self.value]):
            for operation in (self.writable, self.path.unlink, lambda: os.replace(other, self.path),
                              lambda: self.path.rename(self.directory / "renamed.exe")):
                with self.subTest(operation=operation), self.assertRaises(OSError):
                    operation()
            self.assertEqual(self.path.read_bytes(), self.original)
            result = subprocess.run([str(self.path), "/?"], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(file_digest(self.path), {
                "bytes": len(self.original), "sha256": self.value["sha256"],
            })
        self.writable()

    def test_existing_writable_mapping_after_file_close_refuses_protection(self):
        stream = self.path.open("r+b")
        mapping = mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_WRITE)
        stream.close()
        try:
            with self.assertRaises(OSError), frozen.protect_binaries([self.value]):
                self.fail("仍有可写映射时不能进入保护阶段")
            self.assertIsNone(frozen._ACTIVE)
            mapping[-1] = (mapping[-1] + 1) % 256
            mapping.flush()
            self.assertNotEqual(self.path.read_bytes(), self.original)
        finally:
            mapping.close()
        self.value["sha256"] = hashlib.sha256(self.path.read_bytes()).hexdigest()
        with frozen.protect_binaries([self.value]):
            with self.assertRaises(OSError):
                self.path.open("r+b")

    def test_existing_writer_refuses_stage_and_releases_earlier_handles(self):
        other = self.directory / "second.exe"
        shutil.copyfile(self.path, other)
        with other.open("r+b"):
            with self.assertRaises(OSError), frozen.protect_binaries([
                    self.value, {"path": str(other), "sha256": self.value["sha256"]}]):
                self.fail("已有写句柄时不能进入阶段")
        self.writable()
        self.writable(other)

    def test_digest_mismatch_before_lock_and_race_before_open_are_rejected(self):
        with patch.object(frozen, "_open", wraps=frozen._open) as opened:
            with self.assertRaises(ValueError), frozen.protect_binaries([self.value | {"sha256": "0" * 64}]):
                pass
            opened.assert_not_called()
        original_open = frozen._open
        timestamp = self.path.stat()
        def changed_open(kernel, path):
            content = bytearray(path.read_bytes())
            content[-1] ^= 1
            path.write_bytes(content)
            os.utime(path, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))
            return original_open(kernel, path)
        with patch.object(frozen, "_open", side_effect=changed_open):
            with self.assertRaisesRegex(ValueError, "内容变化"), frozen.protect_binaries([self.value]):
                pass
        self.writable()

    def test_nested_same_set_reuses_digest_and_threads_share_real_protection(self):
        with patch.object(frozen, "_plain_digest", wraps=frozen._plain_digest) as hashed:
            with frozen.protect_binaries([self.value]):
                with frozen.protect_binaries([dict(self.value)]):
                    with ThreadPoolExecutor(max_workers=4) as executor:
                        values = list(executor.map(file_digest, [self.path] * 8))
                    self.assertTrue(all(item["sha256"] == self.value["sha256"] for item in values))
                with self.assertRaises(OSError):
                    self.writable()
                with self.assertRaises(ValueError), frozen.protect_binaries([self.value | {"sha256": "0" * 64}]):
                    pass
                self.assertEqual(hashed.call_count, 2)
            self.assertIsNone(frozen.protected_digest(self.path))
        self.writable()

    def test_nested_collection_expansion_and_other_controller_are_rejected(self):
        other = self.directory / "second.exe"
        shutil.copyfile(self.path, other)
        with frozen.protect_binaries([self.value]):
            with self.assertRaises(ValueError), frozen.protect_binaries([
                    self.value, {"path": str(other), "sha256": self.value["sha256"]}]):
                pass
            def foreign_controller():
                with frozen.protect_binaries([self.value]):
                    pass
            with ThreadPoolExecutor(max_workers=1) as executor:
                with self.assertRaises(ValueError):
                    executor.submit(foreign_controller).result()

    def test_exception_releases_all_handles(self):
        other = self.directory / "second.exe"
        shutil.copyfile(self.path, other)
        with self.assertRaisesRegex(RuntimeError, "fixture"), frozen.protect_binaries([
                self.value, {"path": str(other), "sha256": self.value["sha256"]}]):
            raise RuntimeError("fixture")
        self.writable()
        self.writable(other)

    def test_parent_path_change_is_blocked_or_detected_before_reuse(self):
        moved = self.root / "moved"
        renamed = False
        try:
            with frozen.protect_binaries([self.value]):
                try:
                    self.directory.rename(moved)
                except OSError:
                    self.assertEqual(file_digest(self.path)["sha256"], self.value["sha256"])
                else:
                    renamed = True
                    self.directory.mkdir()
                    self.path.write_bytes(self.original)
                    with self.assertRaises(ValueError):
                        frozen.protected_digest(self.path)
        except ValueError:
            if not renamed:
                raise
        if renamed:
            self.writable(moved / self.path.name)

    def test_actual_identity_is_checked_on_every_cache_hit(self):
        with frozen.protect_binaries([self.value]):
            protected = next(iter(frozen._ACTIVE["files"].values()))
            original = frozen._identity
            def changed(kernel, handle):
                result = original(kernel, handle)
                return result if handle == protected.handle else (result[0], result[1], result[2] + 1, *result[3:])
            with patch.object(frozen, "_identity", side_effect=changed):
                with self.assertRaisesRegex(ValueError, "当前路径"):
                    frozen.protected_digest(self.path)

    def test_unregistered_payload_and_linux_branch_always_read_current_bytes(self):
        payload = self.root / "payload.sql"
        payload.write_bytes(b"first")
        with frozen.protect_binaries([self.value]):
            first = file_digest(payload)
            payload.write_bytes(b"other")
            self.assertNotEqual(file_digest(payload), first)
            self.assertEqual(file_digest(payload), {
                "bytes": len(b"other"), "sha256": hashlib.sha256(b"other").hexdigest(),
            })
            self.assertIsNone(frozen.protected_digest(payload))
        with patch.object(frozen, "_WINDOWS", False), patch.object(frozen, "_kernel") as kernel:
            with frozen.protect_binaries([self.value]):
                before = file_digest(self.path)
                self.writable()
                self.assertNotEqual(file_digest(self.path), before)
                self.assertIsNone(frozen.protected_digest(self.path))
            kernel.assert_not_called()

    def test_non_binary_wrong_path_and_duplicate_hash_are_rejected(self):
        payload = self.directory / "not-binary.exe"
        payload.write_bytes(b"MZ" + bytes(128))
        for values in ([{"path": str(payload), "sha256": hashlib.sha256(payload.read_bytes()).hexdigest()}],
                       [self.value, self.value | {"sha256": "0" * 64}],
                       [self.value | {"path": "relative.exe"}], []):
            with self.subTest(values=values), self.assertRaises(ValueError), frozen.protect_binaries(values):
                pass
        self.writable(payload)

    def test_generic_digest_rejects_non_files_and_reads_current_unregistered_bytes(self):
        for path in (self.directory, self.directory / "missing"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                file_digest(path)
        payload = self.root / "payload.bin"
        for content in (b"first", b"changed and longer"):
            payload.write_bytes(content)
            self.assertEqual(file_digest(payload), {
                "bytes": len(content), "sha256": hashlib.sha256(content).hexdigest(),
            })


if __name__ == "__main__":
    unittest.main()
