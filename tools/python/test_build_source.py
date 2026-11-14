from pathlib import Path
import os
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from build_source import build_process_environment


SHA = "0123456789abcdef0123456789abcdef01234567"


class BuildSourceTests(unittest.TestCase):
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
