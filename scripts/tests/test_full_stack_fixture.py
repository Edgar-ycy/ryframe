import hashlib
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import prepare_full_stack_fixture as fixture
import source_inventory

ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / ".local-tests/python-unit"


class FullStackFixtureTests(unittest.TestCase):
    def setUp(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=TEMP)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.backend, self.frontend = self.root / "backend", self.root / "frontend"
        for root, marker in (
            (self.backend, "Cargo.toml"),
            (self.frontend, "package.json"),
        ):
            root.mkdir()
            (root / marker).touch()

    def test_only_new_ignored_output_below_backend_is_accepted(self):
        def git(root, *arguments):
            return (
                str(root).encode() if arguments[0] == "rev-parse" else b".local-tests"
            )

        with patch.object(fixture, "git", side_effect=git):
            fixture.validate_paths(
                self.backend, self.frontend, self.backend / ".local-tests/device"
            )
            for path in (
                self.backend,
                self.frontend,
                self.root / "outside",
                self.backend / ".local-tests",
            ):
                with self.subTest(path=path), self.assertRaises(ValueError):
                    fixture.validate_paths(self.backend, self.frontend, path)
            existing = self.backend / ".local-tests/existing"
            existing.mkdir(parents=True)
            with self.assertRaises(ValueError):
                fixture.validate_paths(self.backend, self.frontend, existing)

    def test_untracked_content_is_fingerprinted_and_escape_is_rejected(self):
        self.assertIs(fixture.git, source_inventory.git)
        self.assertIs(fixture.snapshot, source_inventory.snapshot)
        (self.backend / "new.txt").write_bytes(b"current source")
        responses = [b"a" * 40, b"binary patch", b"new.txt\0"]
        with patch.object(source_inventory, "git", side_effect=responses):
            receipt, diff = fixture.snapshot(self.backend)
        self.assertEqual(diff, b"binary patch")
        self.assertEqual(
            receipt["files"],
            [
                {
                    "path": "new.txt",
                    "sha256": hashlib.sha256(b"current source").hexdigest(),
                }
            ],
        )
        with patch.object(
            source_inventory,
            "git",
            side_effect=[b"a" * 40, b"", b"../frontend/package.json\0"],
        ):
            with self.assertRaises(ValueError):
                fixture.snapshot(self.backend)

    def test_source_change_during_snapshot_copy_fails_closed(self):
        receipt = {"head": "a" * 40, "files": [], "patch_sha256": "original"}
        with (
            patch.object(fixture, "run") as run,
            patch.object(
                fixture,
                "snapshot",
                side_effect=[
                    (receipt, b""),
                    ({**receipt, "patch_sha256": "changed"}, b""),
                ],
            ),
            self.assertRaises(ValueError),
        ):
            fixture.copy_snapshot(
                self.backend,
                self.root / "copy",
                receipt,
                b"patch",
                self.root / "log",
            )
        self.assertEqual(
            run.call_args_list[0].args[0][:4], ["git", "worktree", "add", "--detach"]
        )
        self.assertEqual(
            run.call_args_list[1].args[0], ["git", "apply", "--check", "--binary", "-"]
        )

    def test_fixture_migration_respects_shared_release_stage(self):
        for stable, options in (
            (False, ["--refresh-baseline", "--write"]),
            (True, ["--freeze"]),
        ):
            with (
                self.subTest(stable=stable),
                patch.object(fixture, "release_stage", return_value={"stable": stable}),
                patch.object(fixture, "run") as run,
            ):
                fixture.register_fixture_migration(self.backend, self.root / "log")
                self.assertEqual(run.call_args.args[0][4:], options)
        with (
            patch.object(fixture, "release_stage", side_effect=ValueError("禁止降级")),
            patch.object(fixture, "run") as run,
        ):
            with self.assertRaisesRegex(ValueError, "禁止降级"):
                fixture.register_fixture_migration(self.backend, self.root / "log")
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
