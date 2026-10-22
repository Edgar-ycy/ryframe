"""验证 Windows 工作区临时目录的兼容接口。"""
import os
from pathlib import Path
import stat
import subprocess
import unittest
from unittest import mock

from workspace_directory import WorkspaceDirectory


class WorkspaceDirectoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.parent = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"

    def test_dir_keyword_creates_and_cleans_child_directory(self) -> None:
        temporary = WorkspaceDirectory(dir=self.parent, prefix="workspace-test-")
        self.addCleanup(temporary.cleanup)
        self.assertEqual(temporary.path.parent, self.parent.resolve())
        (temporary.path / "nested").mkdir()
        temporary.cleanup()
        self.assertFalse(temporary.path.exists())

    def test_context_manager_returns_directory_name(self) -> None:
        with WorkspaceDirectory(self.parent, prefix="workspace-context-") as name:
            temporary = Path(name)
            self.assertTrue(temporary.is_dir())
        self.assertFalse(temporary.exists())

    def test_rejects_ambiguous_or_missing_parent(self) -> None:
        with self.assertRaisesRegex(TypeError, "只能指定"):
            WorkspaceDirectory(self.parent, dir=self.parent)
        with self.assertRaisesRegex(TypeError, "必须指定"):
            WorkspaceDirectory()

    def _readonly_file(self, path: Path) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"git-object-sentinel")
        path.chmod(stat.S_IREAD)
        return path

    def _link_directory(self, link: Path, target: Path) -> None:
        if os.name == "nt":
            subprocess.run(
                ["cmd.exe", "/c", "mklink", "/J", str(link), str(target)],
                check=True, capture_output=True,
            )
        else:
            link.symlink_to(target, target_is_directory=True)

    def _unlink_directory(self, link: Path) -> None:
        if os.name == "nt":
            link.rmdir()
        else:
            link.unlink()

    def test_cleans_real_readonly_git_object(self) -> None:
        temporary = WorkspaceDirectory(self.parent, prefix="workspace-readonly-")
        self.addCleanup(temporary.cleanup)
        target = self._readonly_file(temporary.path / ".git/objects/ab" / ("c" * 38))
        if os.name == "nt":
            self.assertTrue(target.lstat().st_file_attributes & stat.FILE_ATTRIBUTE_READONLY)
        temporary.cleanup()
        self.assertFalse(temporary.path.exists())

    def test_nonreadonly_permission_error_preserves_file_and_propagates(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            temporary = WorkspaceDirectory(Path(name))
            self.addCleanup(temporary.cleanup)
            target = temporary.path / "locked-object"
            target.write_bytes(b"locked")
            if os.name == "nt":
                self.assertFalse(target.lstat().st_file_attributes & stat.FILE_ATTRIBUTE_READONLY)
                with target.open("rb"), mock.patch("workspace_directory.os.chmod") as chmod:
                    with self.assertRaises(PermissionError):
                        temporary.cleanup()
                    chmod.assert_not_called()
            else:
                error = PermissionError("真实权限错误不得吞掉")
                with mock.patch("workspace_directory.shutil.rmtree", side_effect=error):
                    with self.assertRaises(PermissionError) as raised:
                        temporary.cleanup()
                    self.assertIs(raised.exception, error)
            self.assertEqual(target.read_bytes(), b"locked")

    def _assert_replaced_path_rejected(self, location: str, during_cleanup: bool) -> None:
        with WorkspaceDirectory(self.parent) as name:
            outer = Path(name)
            parent = outer / "parent" if location == "parent" else outer
            temporary = WorkspaceDirectory(parent)
            original = temporary.path if location == "root" else temporary.path / "nested"
            if location == "parent":
                original = parent
            original.mkdir(exist_ok=True)
            victim_parent = temporary.path if location == "parent" else original
            victim = self._readonly_file(victim_parent / "object")
            external = outer / "external"
            sentinel = self._readonly_file(external / victim.relative_to(original))
            before = sentinel.lstat()
            parked = outer / "parked"
            error = PermissionError("模拟只读文件删除失败")

            def replace_and_retry(*_args, **kwargs):
                original.rename(parked)
                self._link_directory(original, external)
                if during_cleanup:
                    kwargs["onerror"](os.unlink, str(victim), (PermissionError, error, None))

            try:
                if not during_cleanup:
                    replace_and_retry()
                with mock.patch("workspace_directory.os.chmod") as chmod:
                    with self.assertRaises(OSError):
                        if during_cleanup and os.name == "nt":
                            with mock.patch(
                                "workspace_directory.shutil.rmtree", side_effect=replace_and_retry
                            ):
                                temporary.cleanup()
                        elif not during_cleanup and os.name == "nt":
                            with mock.patch(
                                "workspace_directory.shutil.rmtree",
                                side_effect=AssertionError("拒绝链接前不得递归删除"),
                            ):
                                temporary.cleanup()
                        else:
                            if during_cleanup:
                                replace_and_retry()
                            temporary._checked_cleanup_path(victim)
                    chmod.assert_not_called()
                self.assertEqual(sentinel.read_bytes(), b"git-object-sentinel")
                after = sentinel.lstat()
                self.assertEqual(after.st_mode, before.st_mode)
                self.assertEqual(
                    getattr(after, "st_file_attributes", 0),
                    getattr(before, "st_file_attributes", 0),
                )
                self.assertTrue(parked.is_dir())
            finally:
                if parked.exists():
                    if original.is_symlink() or original.exists():
                        self._unlink_directory(original)
                    parked.rename(original)
                temporary.cleanup()

    def test_rejects_root_and_intermediate_links_before_deletion(self) -> None:
        for location in ("root", "intermediate", "parent"):
            with self.subTest(location=location):
                self._assert_replaced_path_rejected(location, during_cleanup=False)

    def test_callback_rejects_root_and_intermediate_reparse_replacement(self) -> None:
        for location in ("root", "intermediate", "parent"):
            with self.subTest(location=location):
                self._assert_replaced_path_rejected(location, during_cleanup=True)

    def test_callback_rejects_outside_paths_and_changed_root(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            outer = Path(name)
            temporary = WorkspaceDirectory(outer)
            sentinel = self._readonly_file(outer / "outside/object")
            error = PermissionError("禁止越界重试")
            try:
                with mock.patch("workspace_directory.os.chmod") as chmod:
                    with self.assertRaises(PermissionError):
                        temporary._retry_readonly_file(
                            os.unlink, str(sentinel), (PermissionError, error, None)
                        )
                    temporary.path = sentinel.parent
                    with self.assertRaises(PermissionError):
                        temporary._checked_cleanup_path(sentinel)
                    chmod.assert_not_called()
                self.assertEqual(sentinel.read_bytes(), b"git-object-sentinel")
            finally:
                temporary.path = temporary._root_path
                temporary.cleanup()

    def test_callback_propagates_unknown_errors_and_operations(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            temporary = WorkspaceDirectory(Path(name))
            self.addCleanup(temporary.cleanup)
            target = self._readonly_file(temporary.path / "object")
            cases = (
                (os.unlink, ValueError("未知错误")),
                (os.rmdir, PermissionError("目录权限错误")),
                (lambda path: None, PermissionError("未知操作")),
            )
            for operation, error in cases:
                with self.subTest(operation=operation, error=error):
                    with mock.patch("workspace_directory.os.chmod") as chmod:
                        with self.assertRaises(type(error)) as raised:
                            temporary._retry_readonly_file(
                                operation, str(target), (type(error), error, None)
                            )
                        self.assertIs(raised.exception, error)
                        chmod.assert_not_called()
                    self.assertEqual(target.read_bytes(), b"git-object-sentinel")

    def test_callback_propagates_retry_failure(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            temporary = WorkspaceDirectory(Path(name))
            self.addCleanup(temporary.cleanup)
            target = self._readonly_file(temporary.path / "object")
            initial = PermissionError("首次删除失败")
            failure = PermissionError("重试仍然失败")
            with mock.patch("workspace_directory.os.unlink", side_effect=failure) as unlink:
                with self.assertRaises(PermissionError) as raised:
                    temporary._retry_readonly_file(
                        unlink, str(target), (PermissionError, initial, None)
                    )
                self.assertIs(raised.exception, failure if os.name == "nt" else initial)
            self.assertEqual(target.read_bytes(), b"git-object-sentinel")

    def test_callback_rejects_shared_hardlink(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            outer = Path(name)
            temporary = WorkspaceDirectory(outer)
            self.addCleanup(temporary.cleanup)
            sentinel = self._readonly_file(outer / "outside/object")
            target = temporary.path / "object"
            os.link(sentinel, target)
            error = PermissionError("共享文件不得改写")
            try:
                with mock.patch("workspace_directory.os.chmod") as chmod:
                    with self.assertRaises(PermissionError) as raised:
                        temporary._retry_readonly_file(
                            os.unlink, str(target), (PermissionError, error, None)
                        )
                    self.assertIs(raised.exception, error)
                    chmod.assert_not_called()
                self.assertEqual(sentinel.read_bytes(), b"git-object-sentinel")
            finally:
                # 只恢复本例创建的共享文件，解除硬链接后再交给目录清理。
                target.chmod(target.stat().st_mode | stat.S_IWRITE)
                target.unlink()

    def test_rejects_replaced_root_identity(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            outer = Path(name)
            temporary = WorkspaceDirectory(outer)
            parked = outer / "parked"
            temporary.path.rename(parked)
            temporary.path.mkdir()
            sentinel = temporary.path / "sentinel"
            sentinel.write_bytes(b"replacement")
            try:
                with mock.patch("workspace_directory.shutil.rmtree") as remove:
                    with self.assertRaises(PermissionError):
                        temporary._checked_cleanup_path(temporary.path)
                    if os.name == "nt":
                        with self.assertRaises(PermissionError):
                            temporary.cleanup()
                    remove.assert_not_called()
                self.assertEqual(sentinel.read_bytes(), b"replacement")
            finally:
                sentinel.unlink()
                temporary.path.rmdir()
                parked.rename(temporary.path)
                temporary.cleanup()

    def test_callback_rejects_nonreadonly_file_without_chmod(self) -> None:
        with WorkspaceDirectory(self.parent) as name:
            temporary = WorkspaceDirectory(Path(name))
            self.addCleanup(temporary.cleanup)
            target = temporary.path / "object"
            target.write_bytes(b"writable")
            error = PermissionError("非只读文件的权限错误")
            with mock.patch("workspace_directory.os.chmod") as chmod:
                with self.assertRaises(PermissionError) as raised:
                    temporary._retry_readonly_file(
                        os.remove, str(target), (PermissionError, error, None)
                    )
                self.assertIs(raised.exception, error)
                chmod.assert_not_called()
            self.assertEqual(target.read_bytes(), b"writable")
