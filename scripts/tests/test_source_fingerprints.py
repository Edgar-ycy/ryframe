"""产品与验收工具指纹及显式产物复用的离线边界测试。"""
import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest
from tests.workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_provenance
import devex_clone_tools
import restore_build
import source_fingerprints as sources
import source_inventory

ROOT = Path(__file__).resolve().parents[2]
CAPTURE = sources.capture_inventory
CURRENT_EXECUTION = sources.current_execution_source


def inventory(tool="a", product="b", head="c"):
    files = [{"path": "Cargo.toml", "sha256": product * 64},
             {"path": "scripts/check.py", "sha256": tool * 64}]
    return {"source": {"snapshot": {"head": head * 40, "patch_sha256": tool * 64,
                                      "files": [], "clean": False},
                       "worktree_fingerprint": "sha256:" + tool * 64}, "files": files,
            "guard": {"head": head * 40, "index_sha256": "1" * 64, "modes_sha256": "2" * 64}}


def execution(value):
    return {**value["source"], "fingerprints": sources.fingerprints(value)}


class SourceFingerprintsTests(unittest.TestCase):
    def test_execution_source_records_snapshot_worktree_and_domains(self):
        value = sources.execution_source(inventory())
        self.assertEqual(value["snapshot"]["head"], "c" * 40)
        self.assertEqual(value["worktree_fingerprint"], "sha256:" + "a" * 64)
        self.assertEqual(set(value["fingerprints"]), {"product", "test_tools", "support"})

    def test_current_execution_source_uses_one_protected_inventory(self):
        value = inventory()
        with patch.object(sources, "capture_inventory", return_value=value) as capture:
            self.assertEqual(
                CURRENT_EXECUTION(Path("repository")),
                sources.execution_source(value),
            )
        capture.assert_called_once_with(Path("repository"))

    def test_build_source_accepts_product_and_maintenance_receipt_shapes(self):
        snapshot = inventory()["source"]["snapshot"]
        self.assertEqual(
            sources.build_source({"kind": "restore-backend-build", "source": snapshot}),
            {"snapshot": snapshot},
        )
        value = inventory()["source"]
        self.assertEqual(
            sources.build_source({"kind": "devex-clone-tool-build", "source": value}),
            value,
        )

    def test_inventory_source_must_match_build_snapshot(self):
        value = inventory()
        snapshot = value["source"]["snapshot"]
        sources.verify_inventory_source(
            value,
            {"kind": "restore-backend-build", "source": snapshot},
        )
        changed = copy.deepcopy(value)
        changed["source"]["snapshot"]["head"] = "0" * 40
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(
                changed,
                {"kind": "restore-backend-build", "source": snapshot},
            )

    def test_maintenance_inventory_must_match_complete_fingerprint(self):
        value = inventory()
        receipt = {
            "kind": "devex-clone-tool-build",
            "source": copy.deepcopy(value["source"]),
        }
        sources.verify_inventory_source(value, receipt)
        receipt["source"]["worktree_fingerprint"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(value, receipt)

    def test_inventory_rejects_invalid_worktree_fingerprint(self):
        value = inventory()
        value["source"]["worktree_fingerprint"] = "invalid"
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(
                value,
                {"kind": "restore-backend-build", "source": value["source"]["snapshot"]},
            )

    def test_receipt_without_inventory_cannot_claim_reusable_source(self):
        with patch.object(
            sources,
            "current_execution_source",
            side_effect=AssertionError("unexpected scan"),
        ):
            self.assertIsNone(
                sources.reusable_artifact_source(
                    Path("repository"),
                    {"source": inventory()["source"]["snapshot"]},
                )
            )

    def test_tool_only_change_reuses_original_product_source(self):
        original = inventory(tool="d")
        current = sources.execution_source(inventory(tool="e"))
        receipt = {
            "kind": "restore-backend-build",
            "source": original["source"]["snapshot"],
            "source_inventory": original,
        }
        with patch.object(sources, "current_execution_source", return_value=current):
            result = sources.reusable_artifact_source(Path("repository"), receipt)
        self.assertEqual(result, original["source"])
        result["snapshot"]["head"] = "0" * 40
        self.assertEqual(original["source"]["snapshot"]["head"], "c" * 40)

    def test_product_change_rejects_existing_artifact(self):
        original = inventory(product="c")
        current = sources.execution_source(inventory(product="0"))
        receipt = {
            "kind": "restore-backend-build",
            "source": original["source"]["snapshot"],
            "source_inventory": original,
        }
        with patch.object(sources, "current_execution_source", return_value=current), \
                self.assertRaisesRegex(ValueError, "重新编译"):
            sources.reusable_artifact_source(Path("repository"), receipt)

    def setUp(self):
        parent = ROOT / ".local-tests/python-unit"
        parent.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=parent)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.local = self.root / ".local-tests"
        self.local.mkdir()
        self.original = inventory()
        self.set_current(inventory(tool="d"))
        self.probe = self.enterContext(patch.object(sources, "current_execution_source", side_effect=lambda _: self.current))
        self.enterContext(patch.object(sources, "capture_inventory", side_effect=lambda _: copy.deepcopy(self.current_inventory)))
        artifacts = {}
        for role, (feature, name) in restore_build.ROLES.items():
            binary = self.local / f"{name}.exe"
            binary.write_bytes(("fixed " + role).encode())
            command = ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                       "--features", feature, "--bin", name, "--message-format=json"]
            artifacts[role] = {"executable": str(binary), "command": command,
                               **restore_build.file_digest(binary)}
        self.receipt = {"format_version": 1, "kind": "restore-backend-build",
                        "source": self.original["source"]["snapshot"],
                        "artifacts": artifacts}
        self.build_path = self.write("build.json", self.receipt)
        self.inventory_path = self.write("inventory.json", self.original)
        self.bridge_path = self.local / "bridge.json"

    def write(self, name, value):
        path = self.local / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def set_current(self, value):
        self.current_inventory = value
        self.current = execution(value)

    def bridge(self):
        sources.write_bridge(self.root, self.build_path, self.inventory_path, self.bridge_path)
        return sources.file_binding(self.bridge_path)

    def maintenance_bridge(self):
        directory = self.local / "maintenance"
        directory.mkdir()
        target = devex_clone_tools.target_directory(self.root)
        cargo_directory = target / "debug"
        cargo_directory.mkdir(parents=True)
        manifest = self.root / "crates/ryframe/Cargo.toml"
        artifacts = {}
        for role, (_, name) in devex_clone_tools.TOOLS.items():
            executable = directory / f"{name}.exe"
            executable.write_bytes(("fixed " + role).encode())
            cargo_executable = cargo_directory / executable.name
            output = directory / f"{role}.cargo.jsonl"
            event = {"reason": "compiler-artifact", "target": {"name": name, "kind": ["bin"]},
                     "manifest_path": str(manifest), "executable": str(cargo_executable)}
            output.write_text(json.dumps(event) + "\n", encoding="utf-8")
            log = directory / f"{role}.cargo.log"
            log.write_text("fixture\n", encoding="utf-8")
            artifacts[role] = {
                "executable": str(executable), "cargo_executable": str(cargo_executable),
                "command": devex_clone_tools.command(role, target),
                "cargo_output": {"file": output.name, **restore_build.file_digest(output)},
                "cargo_log": {"file": log.name, **restore_build.file_digest(log)},
                **restore_build.file_digest(executable),
            }
        receipt = {
            "format_version": 1, "kind": "devex-clone-tool-build", "backend_root": str(self.root),
            "source": self.original["source"], "source_inventory": self.original,
            "toolchain": {"rustc": "fixture", "cargo": "fixture"}, "target_directory": str(target),
            "artifacts": artifacts, "restore_qualified": False, "resources_modified": False,
        }
        build = directory / "build.json"
        build.write_text(json.dumps(receipt), encoding="utf-8")
        inventory_path = directory / "inventory.json"
        inventory_path.write_text(json.dumps(self.original), encoding="utf-8")
        bridge_path = self.local / "maintenance-bridge.json"
        sources.write_bridge(self.root, build, inventory_path, bridge_path)
        return sources.file_binding(bridge_path), receipt

    def test_tool_only_change_keeps_product_and_full_source_is_still_recorded(self):
        changed = inventory(tool="d")
        before, after = sources.fingerprints(self.original), sources.fingerprints(changed)
        self.assertEqual(before["product"], after["product"])
        self.assertNotEqual(before["test_tools"], after["test_tools"])
        self.assertNotEqual(self.original["source"], changed["source"])

    def test_product_inputs_and_unknown_paths_are_conservative(self):
        for path in ("Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml", "crates/app/src/main.rs",
                     "vendor/dep/src/lib.rs", "config/app.toml", "locales/zh.json", "catalog/access.toml",
                     "xtask/src/main.rs", "scripts/embedded.rs", "scripts/release_stage.py", "unknown/input.dat"):
            with self.subTest(path=path):
                self.assertEqual(source_inventory.source_domain(path), "product")
        self.assertEqual(source_inventory.source_domain("scripts/tests/test_copy.py"), "test_tools")
        for path in ("tests/protocol.rs", "xtask/tests/state.rs", "crates/app/tests/fixture.json"):
            with self.subTest(path=path):
                self.assertEqual(source_inventory.source_domain(path), "test_tools")

    def test_added_and_removed_test_files_change_only_test_tool_fingerprint(self):
        before = sources.fingerprints(self.original)
        added = {**self.original, "files": [*self.original["files"],
                                          {"path": "tests/new.rs", "sha256": "f" * 64}]}
        removed = {**self.original, "files": self.original["files"][:1]}
        for value in (added, removed):
            after = sources.fingerprints(value)
            self.assertEqual(before["product"], after["product"])
            self.assertNotEqual(before["test_tools"], after["test_tools"])

    def test_duplicate_unordered_or_escaping_inventory_is_rejected(self):
        for files in ([self.original["files"][0]] * 2, list(reversed(self.original["files"])),
                      [{"path": "../outside", "sha256": "a" * 64}], [{"path": "scripts\\a.py", "sha256": "a" * 64}]):
            with self.subTest(files=files), self.assertRaises(ValueError):
                sources.fingerprints({"files": files})

    def test_explicit_bridge_preserves_original_receipts_and_context_does_not_leak(self):
        before = self.build_path.read_bytes()
        binding = self.bridge()
        self.assertIsNone(sources.reusable_artifact_source(self.root, self.receipt))
        with sources.artifact_sources(self.root, [binding]) as actual:
            self.assertEqual(actual, self.current)
            self.assertEqual(sources.reusable_artifact_source(self.root, self.receipt), self.original["source"])
            self.assertEqual(devex_provenance.verify_source(self.root, self.receipt,
                             self.original["source"]["worktree_fingerprint"]), self.receipt["source"])
            with self.assertRaisesRegex(ValueError, "构建时"):
                devex_provenance.verify_source(self.root, self.receipt, "sha256:" + "f" * 64)
            with self.assertRaisesRegex(ValueError, "嵌套"), sources.artifact_sources(self.root, []):
                pass
        self.assertIsNone(sources.reusable_artifact_source(self.root, self.receipt))
        self.assertEqual(self.build_path.read_bytes(), before)

    def test_bridge_rejects_changed_product_or_forged_original_snapshot(self):
        self.set_current(inventory(product="e"))
        with self.assertRaisesRegex(ValueError, "重新编译"):
            self.bridge()
        self.set_current(inventory(tool="d"))
        self.write("inventory.json", inventory(head="e"))
        with self.assertRaisesRegex(ValueError, "构建时"):
            self.bridge()

    def test_historical_product_requires_explicit_inherited_bridge(self):
        binding = self.bridge()
        self.set_current(inventory(tool="e", product="e"))

        with self.assertRaisesRegex(ValueError, "产品指纹"), \
                sources.artifact_sources(self.root, [binding]):
            pass
        with sources.artifact_sources(
                self.root, [], inherited_bridge_bindings=[binding]) as current:
            self.assertEqual(current, self.current)
            self.assertEqual(
                sources.reusable_artifact_source(self.root, self.receipt),
                self.original["source"],
            )

    def test_inherited_bridge_rejects_forged_audited_product(self):
        binding = self.bridge()
        bridge = json.loads(Path(binding["path"]).read_text(encoding="utf-8"))
        bridge["audited_source"]["fingerprints"]["product"] = {
            "sha256": "f" * 64,
            "files": bridge["original_fingerprints"]["product"]["files"],
        }
        self.write("bridge.json", bridge)
        binding = sources.file_binding(self.bridge_path)
        self.set_current(inventory(tool="e", product="e"))

        with self.assertRaisesRegex(ValueError, "产品指纹"), sources.artifact_sources(
                self.root, [], inherited_bridge_bindings=[binding]):
            pass

    def test_bridge_registration_rejects_duplicates_within_and_across_kinds(self):
        binding = self.bridge()
        cases = (
            ([binding, binding], []),
            ([], [binding, binding]),
            ([binding], [binding]),
        )
        for ordinary, inherited in cases:
            with self.subTest(ordinary=ordinary, inherited=inherited), \
                    self.assertRaisesRegex(ValueError, "重复登记"), \
                    sources.artifact_sources(
                        self.root, ordinary, inherited_bridge_bindings=inherited):
                pass

    def test_bridge_registration_uses_stable_build_identity_across_receipt_shells(self):
        first = self.bridge()
        second_receipt = copy.deepcopy(self.receipt)
        second_receipt["source_inventory"] = self.original
        for artifact in second_receipt["artifacts"].values():
            artifact["cargo_executable"] = artifact["executable"]
        self.assertNotEqual(sources.canonical_digest(self.receipt),
                            sources.canonical_digest(second_receipt))
        second_build = self.write("build-copy.json", second_receipt)
        second_bridge_path = self.local / "bridge-copy.json"
        sources.write_bridge(self.root, second_build, self.inventory_path, second_bridge_path)
        second = sources.file_binding(second_bridge_path)

        for ordinary, inherited in (([first, second], []), ([first], [second])):
            with self.subTest(inherited=bool(inherited)), \
                    self.assertRaisesRegex(ValueError, "同一构建.*重复登记"), \
                    sources.artifact_sources(
                        self.root, ordinary, inherited_bridge_bindings=inherited):
                pass

    def test_bridge_checks_artifact_bytes_and_every_bound_evidence_file(self):
        binary = Path(self.receipt["artifacts"]["api"]["executable"])
        binary.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "构建收据不一致"):
            self.bridge()
        binary.write_bytes(b"fixed api")
        binding = self.bridge()
        self.inventory_path.write_text("{}")
        with self.assertRaisesRegex(ValueError, "摘要变化"), sources.artifact_sources(self.root, [binding]):
            pass

    def test_published_backend_bridge_rechecks_artifact_bytes_on_entry(self):
        binding = self.bridge()
        binary = Path(self.receipt["artifacts"]["worker"]["executable"])
        binary.write_bytes(b"changed after bridge publication")
        with self.assertRaisesRegex(ValueError, "构建收据不一致"), \
                sources.artifact_sources(self.root, [binding]):
            pass

    def test_published_maintenance_bridge_rechecks_artifact_bytes_on_entry(self):
        binding, receipt = self.maintenance_bridge()
        binary = Path(receipt["artifacts"]["migrate"]["executable"])
        binary.write_bytes(b"changed after bridge publication")
        with self.assertRaisesRegex(ValueError, "实际二进制与收据不一致"), \
                sources.artifact_sources(self.root, [binding]):
            pass

    def test_tool_change_during_stage_requires_new_stage_verification(self):
        binding = self.bridge()
        with self.assertRaisesRegex(ValueError, "阶段结束"), sources.artifact_sources(self.root, [binding]):
            self.set_current(inventory(tool="e"))
            with self.assertRaisesRegex(ValueError, "受影响阶段"):
                sources.reusable_artifact_source(self.root, self.receipt)

    def test_inherited_bridge_keeps_current_stage_drift_checks(self):
        binding = self.bridge()
        self.set_current(inventory(tool="e", product="e"))

        with self.assertRaisesRegex(ValueError, "阶段结束"), sources.artifact_sources(
                self.root, [], inherited_bridge_bindings=[binding]):
            self.set_current(inventory(tool="f", product="f"))
            with self.assertRaisesRegex(ValueError, "受影响阶段"):
                sources.reusable_artifact_source(self.root, self.receipt)

    def test_bridge_is_not_available_for_other_repository_or_unregistered_build(self):
        binding = self.bridge()
        with sources.artifact_sources(self.root, [binding]):
            self.assertIsNone(sources.reusable_artifact_source(self.root / "other", self.receipt))
            self.assertIsNone(sources.reusable_artifact_source(self.root, {**self.receipt, "other": True}))

    def test_new_build_inventory_can_reuse_without_rebuilding_but_rejects_product_change(self):
        receipt = {**self.receipt, "source_inventory": self.original}
        self.assertEqual(sources.reusable_artifact_source(self.root, receipt), self.original["source"])
        self.set_current(inventory(product="e"))
        with self.assertRaisesRegex(ValueError, "重新编译"):
            sources.reusable_artifact_source(self.root, receipt)

    def test_formal_restore_still_requires_exact_clean_source_inside_bridge(self):
        binding = self.bridge()
        with sources.artifact_sources(self.root, [binding]), self.assertRaisesRegex(ValueError, "干净 SHA"):
            restore_build.verify_build(self.root, self.receipt, "c" * 40)

    def test_inventory_reads_current_deleted_and_added_files_and_detects_source_race(self):
        source = self.original["source"]["snapshot"]
        (self.root / "Cargo.toml").write_bytes(b"manifest")
        (self.root / "scripts").mkdir()
        (self.root / "scripts/check.py").write_bytes(b"tool")
        paths = b"scripts/check.py\0deleted.rs\0Cargo.toml\0"
        def git(_root, *args):
            return source["head"].encode() if args == ("rev-parse", "HEAD") else paths
        with patch.object(source_inventory, "git", side_effect=git), \
                patch.object(source_inventory, "source_snapshot", return_value=source) as snapshot, \
                patch.object(source_inventory, "worktree_fingerprint", return_value="sha256:" + "a" * 64):
            value = CAPTURE(self.root)
            self.assertEqual([item["path"] for item in value["files"]], ["Cargo.toml", "scripts/check.py"])
            self.assertEqual(value["files"][0]["sha256"], hashlib.sha256(b"manifest").hexdigest())
            snapshot.side_effect = [source, {**source, "clean": True}]
            with self.assertRaisesRegex(ValueError, "完整源码发生变化"):
                CAPTURE(self.root)


if __name__ == "__main__":
    unittest.main()
