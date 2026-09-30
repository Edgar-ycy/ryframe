import os
import shutil
import stat
import subprocess
import sys
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_stage import release_stage, stable_version, workspace_version


@contextmanager
def temporary_workspace(parent):
    root = parent / f"release-stage-{uuid.uuid4().hex}"
    root.mkdir()
    try:
        yield root
    finally:
        root.resolve().relative_to(parent.resolve())

        def writable(function, path, _error):
            os.chmod(path, stat.S_IWRITE)
            function(path)

        shutil.rmtree(root, onexc=writable)


class ReleaseStageTests(unittest.TestCase):
    def test_shallow_history_cannot_disable_stable_protection(self):
        parent = Path.cwd() / ".local-tests"
        parent.mkdir(exist_ok=True)
        with temporary_workspace(parent) as root:
            manifest = root / "Cargo.toml"
            manifest.write_text(
                '[workspace.package]\nversion = "0.12.1"\n', encoding="utf-8"
            )
            with patch("release_stage.git", return_value="true\n") as read_git:
                with self.assertRaisesRegex(ValueError, "浅克隆"):
                    release_stage(root)
                read_git.assert_called_once_with(
                    root, "rev-parse", "--is-shallow-repository"
                )
            manifest.write_text(
                '[workspace.package]\nversion = "1.0.0"\n', encoding="utf-8"
            )
            with patch(
                "release_stage.git",
                side_effect=AssertionError("稳定阶段不需要降级豁免"),
            ):
                self.assertTrue(release_stage(root)["stable"])

    def test_version_requires_workspace_semver(self):
        self.assertFalse(stable_version("0.99.0"))
        self.assertTrue(stable_version("1.0.0"))
        self.assertTrue(stable_version("2.0.0"))
        for value in ("[workspace]", '[workspace.package]\nversion = "latest"'):
            with self.assertRaises(ValueError):
                workspace_version(value)

    def test_downgrade_rejected_after_commit_or_tag(self):
        parent = Path.cwd() / ".local-tests"
        parent.mkdir(exist_ok=True)
        with temporary_workspace(parent) as directory:
            root = Path(directory)

            def git(*args):
                subprocess.run(
                    ["git", "-C", str(root), *args], capture_output=True, check=True
                )

            def version(value):
                (root / "Cargo.toml").write_text(
                    f'[workspace.package]\nversion = "{value}"\n', encoding="utf-8"
                )

            def commit():
                git("add", "Cargo.toml")
                git(
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-m",
                    "test",
                )

            git("init")
            version("0.12.1")
            commit()
            baseline = subprocess.check_output(
                ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
            ).strip()
            self.assertEqual(
                release_stage(root), {"version": "0.12.1", "stable": False}
            )
            version("1.0.0")
            self.assertTrue(release_stage(root)["stable"])
            commit()
            version("0.12.2")
            with self.assertRaisesRegex(ValueError, "禁止降低"):
                release_stage(root)
            commit()
            with self.assertRaisesRegex(ValueError, "禁止降低"):
                release_stage(root)
            git("tag", "v1.0.0")
            with self.assertRaisesRegex(ValueError, "禁止降低"):
                release_stage(root, baseline)


if __name__ == "__main__":
    unittest.main()
