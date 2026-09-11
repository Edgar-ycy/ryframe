"""只验证构建事件与来源收据；Cargo 和 Git 均使用替身，不创建真实构建。"""
import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_build as build

SOURCE = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": [], "clean": True}


class RestoreBuildTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.local = self.root / ".local-tests"
        self.local.mkdir()
        self.source = self.enterContext(patch.object(build, "source_snapshot", return_value=copy.deepcopy(SOURCE)))
        def inventory(_, source=None):
            snapshot = copy.deepcopy(source if source is not None else self.source())
            return {
                "source": {"snapshot": snapshot, "worktree_fingerprint": "sha256:" + "c" * 64},
                "files": [],
                "guard": {"head": snapshot["head"], "index_sha256": "d" * 64,
                          "modes_sha256": "e" * 64},
            }
        self.enterContext(patch.object(build, "capture_inventory", side_effect=inventory))

    def cargo_run(self, command, **kwargs):
        if command == ["rustc", "-vV"]:
            return subprocess.CompletedProcess(command, 0, stdout="rustc 1.91.0\nhost: x86_64-pc-windows-msvc\n")
        if command == ["cargo", "-V"]:
            return subprocess.CompletedProcess(command, 0, stdout="cargo 1.91.0\n")
        name = command[command.index("--bin") + 1]
        self.assertEqual(kwargs["cwd"], self.root)
        self.assertTrue(kwargs["check"])
        executable = self.local / (name + ".exe")
        executable.write_bytes(name.encode())
        event = {"reason": "compiler-artifact", "manifest_path": str(self.root / "crates/ryframe/Cargo.toml"),
                 "target": {"name": name, "kind": ["bin"]}, "executable": str(executable)}
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(event))

    def test_cargo_artifacts_are_bound_to_workspace_source_and_actual_file_bytes(self):
        receipt = build.build(self.root, self.cargo_run)
        build.verify_build(self.root, receipt, SOURCE["head"], self.cargo_run)
        self.assertEqual(receipt["sources"]["full"]["source"]["snapshot"], SOURCE)
        self.assertEqual(set(receipt["artifacts"]), {"api", "worker"})
        binary = Path(receipt["artifacts"]["api"]["executable"])
        binary.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "二进制文件"):
            build.verify_build(self.root, receipt, SOURCE["head"], self.cargo_run)

    def test_dirty_source_and_changes_during_build_never_become_formal_evidence(self):
        dirty = SOURCE | {"clean": False, "files": [{"path": "new.rs", "sha256": "c" * 64}]}
        self.source.return_value = dirty
        receipt = build.build(self.root, self.cargo_run)
        self.assertEqual(receipt["sources"]["full"]["source"]["snapshot"], dirty)
        with self.assertRaisesRegex(ValueError, "干净 SHA"):
            build.verify_build(self.root, receipt, SOURCE["head"])
        self.source.side_effect = [SOURCE, dirty]
        with self.assertRaisesRegex(ValueError, "源码发生变化"):
            build.build(self.root, self.cargo_run)

    def test_registered_source_requires_explicit_exact_clean_sha_before_build(self):
        with patch.object(build, "repository", side_effect=lambda path, _label: path):
            source, inventory, product = build.registered_source(
                self.root, self.root, SOURCE["head"]
            )
            self.assertEqual(source, self.root)
            self.assertEqual(inventory["source"]["snapshot"], SOURCE)
            self.assertIsNone(product)
            with self.assertRaisesRegex(ValueError, "完整小写 SHA"):
                build.registered_source(self.root, self.root, "short")
            with self.assertRaisesRegex(ValueError, "独立产品来源"):
                build.registered_source(
                    self.root, self.root, SOURCE["head"], product_backend=self.root
                )
            self.source.return_value = SOURCE | {"clean": False}
            with self.assertRaisesRegex(ValueError, "精确干净"):
                build.registered_source(self.root, self.root, SOURCE["head"])

    def test_registered_b0_adapter_is_bound_to_embedded_patch_and_unchanged_product_domains(self):
        import restore_comparison_source as comparison

        adapter = SOURCE | {"head": comparison.B0_ADAPTER_COMMIT}
        product = SOURCE | {"head": comparison.B0_BACKEND_COMMIT}
        self.source.side_effect = [adapter, product]
        with patch.object(build, "repository", side_effect=lambda path, _label: path), patch.object(
            comparison,
            "b0_adapter_evidence",
            return_value={"contract": "legacy-stable-readiness-b0-v1"},
        ) as evidence:
            source, inventory, bound_product = build.registered_source(
                self.root,
                self.root,
                comparison.B0_ADAPTER_COMMIT,
                adapter_contract="legacy-stable-readiness-b0-v1",
                product_backend=self.local,
            )
        self.assertEqual(source, self.root)
        self.assertEqual(inventory["source"]["snapshot"], adapter)
        self.assertEqual(bound_product[1]["source"]["snapshot"], product)
        evidence.assert_called_once_with(self.root, reconstruct=True)

    def test_registered_b0_preview_uses_readonly_evidence_without_rebuilding_git_tree(self):
        import restore_comparison_source as comparison

        adapter = SOURCE | {"head": comparison.B0_ADAPTER_COMMIT}
        product = SOURCE | {"head": comparison.B0_BACKEND_COMMIT}
        self.source.side_effect = [adapter, product]
        with patch.object(build, "repository", side_effect=lambda path, _label: path), \
                patch.object(comparison, "b0_adapter_evidence", return_value={"contract": "legacy-stable-readiness-b0-v1"}) as evidence:
            build.registered_source(self.root, self.root, comparison.B0_ADAPTER_COMMIT,
                                    adapter_contract="legacy-stable-readiness-b0-v1", product_backend=self.local, reconstruct=False)
        evidence.assert_called_once_with(self.root, reconstruct=False)

    def test_wrong_workspace_or_duplicate_cargo_events_cannot_register_artifact(self):
        output = self.cargo_run(["--bin", "ryframe"], cwd=self.root, check=True).stdout
        event = json.loads(output)
        cases = [output + "\n" + output, "",
                 json.dumps(event | {"manifest_path": str(self.root / "other/Cargo.toml")}),
                 json.dumps(event | {"target": {"name": "ryframe", "kind": ["lib"]}}),
                 json.dumps(event | {"target": {"name": "wrong", "kind": ["bin"]}})]
        for value in cases:
            with self.subTest(value=value), self.assertRaises(ValueError):
                build.cargo_artifact(self.root, "ryframe", value)

    def test_command_failure_propagates_without_retry_or_publishing_a_receipt(self):
        failure = subprocess.CalledProcessError(1, ["cargo", "build"])
        with patch.object(build, "write_new") as write, \
                patch.object(build.subprocess, "run", side_effect=failure) as run:
            with self.assertRaises(subprocess.CalledProcessError) as caught:
                build.build(self.root, run)
            self.assertIs(caught.exception, failure)
            self.assertEqual(run.call_count, 1)
            write.assert_not_called()
        self.assertEqual(list(self.local.iterdir()), [])

    def test_verification_rejects_missing_roles_changed_commands_and_wrong_sha(self):
        receipt = build.build(self.root, self.cargo_run)
        for role in ("api", "worker"):
            changed = copy.deepcopy(receipt)
            del changed["artifacts"][role]
            with self.assertRaises(ValueError):
                build.verify_build_artifacts(changed)
            changed = copy.deepcopy(receipt)
            changed["artifacts"][role]["command"] += ["--release"]
            with self.assertRaises(ValueError):
                build.verify_build_artifacts(changed)
        changed = copy.deepcopy(receipt)
        changed["artifacts"]["api"]["cargo_executable"] = changed["artifacts"]["api"]["executable"]
        with self.assertRaises(ValueError):
            build.verify_build_artifacts(changed)
        with self.assertRaises(ValueError):
            build.verify_build(self.root, receipt, "d" * 40, self.cargo_run)

    def test_build_context_change_rejects_artifact_reuse(self):
        receipt = build.build(self.root, self.cargo_run)
        changed = copy.deepcopy(receipt)
        changed["build"]["jobs"] = "8"
        build.validate_build_context(changed["build"])
        with self.assertRaisesRegex(ValueError, "干净 SHA"):
            build.verify_build(self.root, changed, SOURCE["head"], self.cargo_run)

    def test_build_environment_covers_toolchain_native_and_reproducibility_inputs(self):
        environment = {
            "RUSTUP_TOOLCHAIN": "stable-fixture",
            "RUSTC_BOOTSTRAP": "0",
            "RYFRAME_BUILD_COMMIT": "a" * 40,
            "SOURCE_DATE_EPOCH": "1",
            "PATH": "fixture-native-tools",
            "INCLUDE": "fixture-headers",
            "LIB": "fixture-libraries",
            "LIBCLANG_PATH": "fixture-clang",
            "PKG_CONFIG": "fixture-pkg-config",
            "AWS_LC_SYS": "fixture-aws-lc",
            "OPENSSL": "fixture-openssl-command",
            "AR_x86_64_pc_windows_msvc": "llvm-lib",
            "CXXFLAGS_x86_64_pc_windows_msvc": "/O2",
            "PKG_CONFIG_PATH": "fixture-pkg",
            "AWS_LC_SYS_CMAKE_BUILDER": "1",
            "OPENSSL_DIR": "fixture-openssl",
            "APP_SECRET": "must-not-be-recorded",
        }
        first = build.build_environment(environment)
        self.assertEqual(
            first["variables"],
            sorted(name for name in environment if name != "APP_SECRET"),
        )
        self.assertNotIn("must-not-be-recorded", json.dumps(first))
        changed = dict(environment, SOURCE_DATE_EPOCH="2")
        self.assertNotEqual(first["sha256"], build.build_environment(changed)["sha256"])

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
