import hashlib
import json
import os
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import prepare_full_stack_fixture as fixture
import full_stack_process
import source_inventory
import reference_fixture_control_protocol as protocol

ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / ".local-tests/python-unit"


class FullStackFixtureTests(unittest.TestCase):
    def setUp(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        self.directory = WorkspaceDirectory(dir=TEMP)
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
                self.backend, self.frontend, self.backend / ".local-tests/business"
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
        self.assertIs(fixture.write_receipt, full_stack_process.write_receipt)
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

    def test_formal_source_requires_exact_clean_head(self):
        expected = "a" * 40
        clean = {
            "head": expected,
            "patch_sha256": hashlib.sha256(b"").hexdigest(),
            "files": [],
        }
        with (
            patch.object(fixture, "snapshot", return_value=(clean, b"")),
            patch.object(fixture, "git", return_value=b""),
        ):
            self.assertEqual(
                fixture.exact_clean_snapshot(self.backend, expected), (clean, b"")
            )

        cases = (
            ({**clean, "head": "b" * 40}, b"", b""),
            (clean, b"patch", b" M tracked.rs"),
            ({**clean, "files": [{"path": "new.rs", "sha256": "c" * 64}]}, b"", b"?? new.rs"),
        )
        for receipt, diff, status in cases:
            with (
                self.subTest(receipt=receipt, diff=diff, status=status),
                patch.object(fixture, "snapshot", return_value=(receipt, diff)),
                patch.object(fixture, "git", return_value=status),
                self.assertRaisesRegex(ValueError, "精确、干净"),
            ):
                fixture.exact_clean_snapshot(self.backend, expected)
        with self.assertRaisesRegex(ValueError, "SHA 无效"):
            fixture.exact_clean_snapshot(self.backend, "main")

    def test_formal_pair_is_validated_before_output_creation(self):
        output = self.backend / ".local-tests/formal-business"
        with (
            patch.object(fixture, "validate_paths"),
            patch.object(
                fixture,
                "source_inputs",
                side_effect=ValueError("正式夹具必须使用精确、干净的候选提交"),
            ),
            self.assertRaisesRegex(ValueError, "精确、干净"),
        ):
            fixture.prepare(
                self.backend,
                self.frontend,
                output,
                expected_backend_sha="a" * 40,
                expected_frontend_sha="b" * 40,
            )
        self.assertFalse(output.exists())

    def test_formal_pair_is_rechecked_immediately_before_output_creation(self):
        definition = self.backend / "tools/python/fixtures/order-business/src/resources/mod.rs"
        definition.parent.mkdir(parents=True)
        definition.write_text("[resource]\n", encoding="utf-8")
        sources = (
            ({"head": "a" * 40, "patch_sha256": "0" * 64, "files": []}, b""),
            ({"head": "b" * 40, "patch_sha256": "0" * 64, "files": []}, b""),
        )
        output = self.backend / ".local-tests/formal-business"
        with (
            patch.object(fixture, "validate_paths"),
            patch.object(
                fixture,
                "source_inputs",
                side_effect=[sources, ValueError("候选源码发生变化")],
            ) as source_inputs,
            self.assertRaisesRegex(ValueError, "候选源码发生变化"),
        ):
            fixture.prepare(
                self.backend,
                self.frontend,
                output,
                expected_backend_sha="a" * 40,
                expected_frontend_sha="b" * 40,
            )
        self.assertEqual(source_inputs.call_count, 2)
        self.assertFalse(output.exists())

    def test_formal_pair_requires_both_commits(self):
        with self.assertRaisesRegex(ValueError, "同时指定"):
            fixture.source_inputs(self.backend, self.frontend, "a" * 40, None)

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

    def test_business_fixture_is_created_and_explicitly_registered(self):
        cargo = self.backend / "crates/ryframe/Cargo.toml"
        cargo.parent.mkdir(parents=True)
        cargo.write_text("[dependencies]\n", encoding="utf-8")
        registry = self.backend / "crates/ryframe/src/business.rs"
        registry.parent.mkdir(parents=True)
        registry.write_text("pub fn business_modules() {\n    vec![]\n}\n", encoding="utf-8")
        with patch.object(fixture, "run") as run:
            model = fixture.create_business_fixture(self.backend, self.root / "log")

        self.assertIn(b"biz_order", model)
        self.assertIn('order-business', cargo.read_text(encoding="utf-8"))
        self.assertIn('order_business::module()', registry.read_text(encoding="utf-8"))
        self.assertTrue((self.backend / "crates/order-business/src/resources/mod.rs").is_file())
        self.assertEqual(run.call_count, 1)

    def test_reference_fixture_root_is_created_once_for_new_business_worktree(self):
        root = fixture.reference_fixture_root(self.backend)
        self.assertEqual(root, self.backend / ".local-tests/reference-fixture")
        self.assertTrue(root.is_dir())
        with self.assertRaisesRegex(ValueError, "已存在"):
            fixture.reference_fixture_root(self.backend)

    def test_private_protocol_reconstructs_prepare_arguments_without_leaking_to_children(self):
        value = {
            "backend_dir": str(self.backend),
            "domain": "prepare",
            "format_version": 1,
            "frontend_dir": str(self.frontend),
            "kind": protocol.PROTOCOL_KIND,
            "operation": "prepare",
            "output_dir": str(self.backend / ".local-tests/business"),
            "expected_backend_sha": "a" * 40,
            "expected_frontend_sha": "b" * 40,
            "write": True,
        }
        raw = json.dumps(value, separators=(",", ":"))
        arguments = protocol.private_arguments(
            "prepare",
            fixture.PROTOCOL_SCHEMAS,
            positional_operation=False,
            argv=[],
            environment={protocol.PROTOCOL_KEY: raw},
        )
        self.assertEqual(
            arguments,
            [
                "--backend-dir",
                str(self.backend),
                "--frontend-dir",
                str(self.frontend),
                "--output-dir",
                str(self.backend / ".local-tests/business"),
                "--expected-backend-sha",
                "a" * 40,
                "--expected-frontend-sha",
                "b" * 40,
                "--write",
            ],
        )

        observed = []

        def main(_arguments):
            observed.append(protocol.PROTOCOL_KEY in os.environ)

        with (
            patch.dict(os.environ, {protocol.PROTOCOL_KEY: raw}),
            patch.object(sys, "argv", ["prepare_full_stack_fixture.py"]),
        ):
            self.assertEqual(
                protocol.run_private(
                    "prepare",
                    fixture.PROTOCOL_SCHEMAS,
                    main,
                    positional_operation=False,
                ),
                0,
            )
            self.assertIn(protocol.PROTOCOL_KEY, os.environ)
        self.assertEqual(observed, [False])


if __name__ == "__main__":
    unittest.main()
