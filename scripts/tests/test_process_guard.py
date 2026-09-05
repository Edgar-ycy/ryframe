"""验证本机内核互斥的输入边界、跨进程隔离和退出释放，不操作业务进程。"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

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
        temporary = tempfile.TemporaryDirectory(dir=base)
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

    def test_existing_guard_contents_are_preserved_across_repeated_acquisitions(self):
        path = self.directory / "test.guard"
        contents = b"existing ownership evidence"
        path.write_bytes(contents)
        for _ in range(2):
            with process_guard(self.directory, "test.guard"):
                self.assertEqual(path.stat().st_size, len(contents))
            self.assertEqual(path.read_bytes(), contents)


if __name__ == "__main__":
    unittest.main()
