from pathlib import Path
import os
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from build_source import bound_source, build_process_environment
from workspace_directory import WorkspaceDirectory


SHA = "0123456789abcdef0123456789abcdef01234567"


class BuildSourceTests(unittest.TestCase):
    def setUp(self):
        parent = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        parent.mkdir(parents=True, exist_ok=True)
        self.temporary = WorkspaceDirectory(dir=parent)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()

    def test_exact_source_exists_only_during_context(self):
        source = self.root / ".local-tests/build/source-commit"
        with bound_source(self.root, SHA):
            self.assertEqual(source.read_text(encoding="utf-8"), SHA + "\n")
            self.assertTrue(source.with_name("source-commit.lock").is_file())
        self.assertFalse(source.exists())
        self.assertFalse(source.with_name("source-commit.lock").exists())

    def test_invalid_concurrent_and_drifted_sources_fail_closed(self):
        with self.assertRaises(ValueError):
            with bound_source(self.root, "HEAD"):
                self.fail("无效来源不得执行")
        source = self.root / ".local-tests/build/source-commit"
        with self.assertRaisesRegex(ValueError, "漂移"):
            with bound_source(self.root, SHA):
                source.write_text("changed\n", encoding="utf-8")
        self.assertEqual(source.read_text(encoding="utf-8"), "changed\n")
        with self.assertRaisesRegex(ValueError, "控制器|现场"):
            with bound_source(self.root, SHA):
                self.fail("未确认现场不得重放")

    def test_build_environment_removes_project_configuration(self):
        actual = build_process_environment({
            "APP_ENV": "test",
            "APP_DATABASE_PASSWORD": "secret",
            "RYFRAME_LEGACY_CONFIG": SHA,
            "VITE_APP_PROXY_TARGET": "secret",
            "SNOWFLAKE_WORKER_ID": "2",
            "CARGO_TARGET_DIR": "D:/target",
            "PATH": os.environ.get("PATH", "fixture"),
        })
        self.assertEqual(actual["APP_ENV"], "test")
        self.assertEqual(actual["CARGO_TARGET_DIR"], "D:/target")
        self.assertFalse(any(name.startswith(("RYFRAME_", "VITE_", "SNOWFLAKE_"))
                             for name in actual))
        self.assertNotIn("APP_DATABASE_PASSWORD", actual)


if __name__ == "__main__":
    unittest.main()
