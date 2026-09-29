"""验证 Windows 工作区临时目录的兼容接口。"""
from pathlib import Path
import unittest

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
