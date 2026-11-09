"""共享create-only发布边界；原生构建风险由Rust和来源行为用例覆盖。"""
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory
import restore_build as build


class RestoreBuildPublicationTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.local = self.root / ".local-tests"
        self.local.mkdir()

    def test_receipt_write_is_explicit_scoped_and_never_overwrites_existing_evidence(self):
        path = self.local / "receipt.json"
        with patch.object(build, "git", return_value=b".local-tests/receipt.json") as git:
            build.write_new(path, {"result": "fixture"}, self.root)
            git.assert_called_once_with(self.root, "check-ignore", ".local-tests/receipt.json")
            before = path.read_bytes()
            with self.assertRaises(FileExistsError):
                build.write_new(path, {"result": "changed"}, self.root)
            self.assertEqual(path.read_bytes(), before)
            git.reset_mock()
            with self.assertRaises(ValueError):
                build.write_new(self.root / "outside.json", {}, self.root)
            git.assert_not_called()
        unignored = self.local / "not-ignored.json"
        with patch.object(build, "git", side_effect=subprocess.CalledProcessError(1, ["git"])):
            with self.assertRaises(subprocess.CalledProcessError):
                build.write_new(unignored, {}, self.root)
        self.assertFalse(unignored.exists())


if __name__ == "__main__":
    unittest.main()
