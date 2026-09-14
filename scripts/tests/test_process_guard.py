"""验证本机内核互斥的输入边界、跨进程隔离和退出释放，不操作业务进程。"""
import os
from pathlib import Path
import stat
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from process_guard import process_guard


CHILD = """from pathlib import Path
import os
import sys
from process_guard import process_guard
with process_guard(Path(sys.argv[1]), 'test.guard'):
    print('locked', flush=True)
    if sys.stdin.read(1) == '!':
        os._exit(0)
"""


class ProcessGuardTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()

    def child(self):
        environment = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1]))
        process = subprocess.Popen(
            [sys.executable, "-B", "-c", CHILD, str(self.directory)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, env=environment,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )

        def cleanup():
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)

        self.addCleanup(cleanup)
        self.assertEqual(process.stdout.readline().strip(), "locked")
        return process

    def test_invalid_directory_and_filename_never_create_guard_files(self):
        cases = [
            (Path("."), "test.guard"),
            (self.directory / "missing", "test.guard"),
            (self.directory, ""),
            (self.directory, "."),
            (self.directory, ".."),
            (self.directory, str(Path("nested") / "test.guard")),
            (self.directory, str(self.directory / "absolute.guard")),
        ]
        for directory, filename in cases:
            with self.subTest(directory=directory, filename=filename):
                with self.assertRaises(ValueError), process_guard(directory, filename):
                    self.fail("无效路径不能进入受保护操作")
                self.assertEqual(list(self.directory.iterdir()), [])

    def test_live_owner_excludes_contender_and_normal_exit_releases_guard(self):
        child = self.child()
        with self.assertRaisesRegex(ValueError, "并发"), process_guard(self.directory, "test.guard"):
            self.fail("另一个进程仍持有相同互斥")
        self.assertIsNone(child.poll())
        _, error = child.communicate("x", timeout=5)
        self.assertEqual(child.returncode, 0, error)
        with process_guard(self.directory, "test.guard"):
            self.assertTrue((self.directory / "test.guard").exists())

    def test_process_exit_without_context_cleanup_releases_kernel_guard(self):
        child = self.child()
        _, error = child.communicate("!", timeout=5)
        self.assertEqual(child.returncode, 0, error)
        with process_guard(self.directory, "test.guard"):
            self.assertTrue((self.directory / "test.guard").exists())
        self.assertEqual((self.directory / "test.guard").read_bytes(), b"\0")

    def test_exception_releases_guard_and_preserves_original_error(self):
        failure = RuntimeError("原始操作失败")
        with self.assertRaises(RuntimeError) as caught:
            with process_guard(self.directory, "test.guard"):
                raise failure
        self.assertIs(caught.exception, failure)
        with process_guard(self.directory, "test.guard"):
            self.assertTrue((self.directory / "test.guard").exists())
        self.assertEqual((self.directory / "test.guard").read_bytes(), b"\0")

    def test_invalid_existing_guard_is_rejected_without_rewriting_it(self):
        path = self.directory / "test.guard"
        for contents in (b"", b"x", b"existing ownership evidence"):
            with self.subTest(contents=contents):
                path.write_bytes(contents)
                with self.assertRaisesRegex(ValueError, "控制互斥文件"), process_guard(
                        self.directory, "test.guard"):
                    self.fail("非 NUL guard 不能伪装成已持有的合法互斥")
                self.assertEqual(path.read_bytes(), contents)

    def test_new_guard_is_a_single_link_regular_file(self):
        path = self.directory / "test.guard"
        with process_guard(self.directory, "test.guard") as lease:
            metadata = path.lstat()
            self.assertTrue(stat.S_ISREG(metadata.st_mode))
            self.assertEqual(metadata.st_nlink, 1)
            lease.check()
        self.assertEqual(path.read_bytes(), b"\0")

    def test_existing_hard_link_is_rejected_without_modifying_source(self):
        source = self.directory / "source"
        path = self.directory / "test.guard"
        source.write_bytes(b"\0")
        try:
            os.link(source, path)
        except OSError as error:
            self.skipTest(f"当前文件系统不支持硬链接：{error}")
        with self.assertRaisesRegex(ValueError, "控制互斥文件"), process_guard(
                self.directory, "test.guard"):
            self.fail("硬链接不能作为控制互斥")
        self.assertEqual(source.read_bytes(), b"\0")
        self.assertEqual(path.read_bytes(), b"\0")

    def test_lease_rejects_hard_link_added_after_acquisition(self):
        path = self.directory / "test.guard"
        alias = self.directory / "alias.guard"
        with process_guard(self.directory, "test.guard") as lease:
            try:
                os.link(path, alias)
            except OSError as error:
                self.skipTest(f"当前文件系统不支持硬链接：{error}")
            with self.assertRaisesRegex(ValueError, "控制互斥文件"):
                lease.check()
            alias.unlink()
            lease.check()

    @unittest.skipIf(os.name == "nt", "Windows 不允许替换已打开的 guard")
    def test_lease_rejects_guard_path_replacement(self):
        path = self.directory / "test.guard"
        moved = self.directory / "moved.guard"
        with process_guard(self.directory, "test.guard") as lease:
            path.rename(moved)
            path.write_bytes(b"\0")
            try:
                with self.assertRaisesRegex(ValueError, "控制互斥文件"):
                    lease.check()
            finally:
                path.unlink()
                moved.rename(path)
            lease.check()

    def test_existing_directory_is_rejected_without_modification(self):
        path = self.directory / "test.guard"
        path.mkdir()
        with self.assertRaisesRegex(ValueError, "控制互斥文件"), process_guard(
                self.directory, "test.guard"):
            self.fail("目录不能作为控制互斥")
        self.assertTrue(path.is_dir())

    @unittest.skipUnless(hasattr(os, "mkfifo"), "当前平台不支持 FIFO")
    def test_existing_fifo_is_rejected_without_opening_or_modification(self):
        path = self.directory / "test.guard"
        os.mkfifo(path)
        with self.assertRaisesRegex(ValueError, "控制互斥文件"), process_guard(
                self.directory, "test.guard"):
            self.fail("FIFO 不能作为控制互斥")
        self.assertTrue(stat.S_ISFIFO(path.lstat().st_mode))

    @unittest.skipIf(os.name == "nt", "Windows 不允许重命名包含已打开 guard 的目录")
    def test_lease_rejects_parent_directory_replacement(self):
        original = self.directory
        moved = original.with_name(original.name + "-moved")
        with process_guard(original, "test.guard") as lease:
            original.rename(moved)
            original.mkdir()
            try:
                with self.assertRaisesRegex(ValueError, "父目录"):
                    lease.check()
            finally:
                original.rmdir()
                moved.rename(original)
            lease.check()


if __name__ == "__main__":
    unittest.main()
