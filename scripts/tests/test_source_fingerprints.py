"""产品与验收工具指纹及显式产物复用的离线边界测试。"""
import copy
import hashlib
import json
from pathlib import Path
import sys
import unittest
from workspace_directory import WorkspaceDirectory
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


def inventory(tool="a", product="b", head="c", xtask="3"):
    files = [{"path": "Cargo.toml", "sha256": product * 64},
             {"path": "scripts/check.py", "sha256": tool * 64},
             {"path": "xtask/src/main.rs", "sha256": xtask * 64}]
    return {"source": {"snapshot": {"head": head * 40, "patch_sha256": tool * 64,
                                      "files": [], "clean": False},
                       "worktree_fingerprint": "sha256:" + tool * 64}, "files": files,
            "guard": {"head": head * 40, "index_sha256": "1" * 64, "modes_sha256": "2" * 64}}


def execution(value):
    return {**value["source"], "fingerprints": sources.fingerprints(value)}


def build_context():
    return {
        "commands": {role: restore_build.build_command(role) for role in restore_build.ROLES},
        "profile": "dev", "target": "x86_64-pc-windows-msvc", "jobs": "cargo-default",
        "toolchain": {"cargo": "cargo fixture", "rustc": "rustc fixture"},
        "environment": {"variables": [], "sha256": source_inventory.canonical_digest([])},
    }


def build_receipt(value, artifacts=None):
    return {"format_version": 2, "kind": "restore-backend-build",
            "sources": source_inventory.build_source_domains(value, "backend"),
            "build": build_context(), "artifacts": artifacts or {}}


class SourceFingerprintsTests(unittest.TestCase):
    def test_registered_execution_requires_clean_head_and_all_tool_files(self):
        value = inventory()
        value["source"]["snapshot"]["clean"] = True
        registered = execution(value)
        with patch.object(sources, "current_execution_source", return_value=registered):
            self.assertEqual(sources.require_current_execution_source(ROOT, registered), registered)
        for mutate in (
            lambda item: item["snapshot"].update(head="e" * 40),
            lambda item: item["snapshot"].update(clean=False),
            lambda item: item["fingerprints"]["test_tools"].update(sha256="e" * 64),
        ):
            current = copy.deepcopy(registered)
            mutate(current)
            with patch.object(sources, "current_execution_source", return_value=current), self.assertRaises(ValueError):
                sources.require_current_execution_source(ROOT, registered)
        registered["snapshot"]["clean"] = False
        with patch.object(sources, "current_execution_source", return_value=registered), self.assertRaises(ValueError):
            sources.require_current_execution_source(ROOT, registered)

    def test_indirect_and_dynamic_node_imports_belong_to_complete_tool_domain(self):
        value = inventory()
        helpers = ["scripts/devex/request.mjs", "scripts/restore_reference_existing.mjs"]
        value["files"] = sorted(value["files"] + [{"path": path, "sha256": "a" * 64} for path in helpers], key=lambda item: item["path"])
        original = sources.fingerprints(value)
        for path in helpers:
            changed = copy.deepcopy(value)
            next(item for item in changed["files"] if item["path"] == path)["sha256"] = "f" * 64
            self.assertNotEqual(sources.fingerprints(changed)["test_tools"], original["test_tools"])
            self.assertEqual(sources.fingerprints(changed)["product"], original["product"])

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
        value = inventory()
        self.assertEqual(
            sources.build_source(build_receipt(value)),
            value["source"],
        )
        value = inventory()["source"]
        self.assertEqual(
            sources.build_source({"kind": "devex-clone-tool-build", "source": value}),
            value,
        )

    def test_inventory_source_must_match_build_snapshot(self):
        value = inventory()
        receipt = build_receipt(value)
        sources.verify_inventory_source(value, receipt)
        changed = copy.deepcopy(value)
        changed["source"]["snapshot"]["head"] = "0" * 40
        with self.assertRaisesRegex(ValueError, "构建时"):
            sources.verify_inventory_source(
                changed,
                receipt,
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
        original = inventory()
        receipt = build_receipt(original)
        value = copy.deepcopy(original)
        value["source"]["worktree_fingerprint"] = "invalid"
        with self.assertRaisesRegex(ValueError, "构建时|格式"):
            sources.verify_inventory_source(value, receipt)

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

    def test_restore_build_source_rejects_v1_and_extra_fields(self):
        value = build_receipt(inventory())
        for changed in ({**value, "format_version": 1}, {**value, "unexpected": True}):
            with self.subTest(fields=set(changed)), self.assertRaisesRegex(ValueError, "严格 v2"):
                sources.build_source(changed)

    def test_tool_only_change_reuses_original_product_source(self):
        original = inventory(tool="d")
        current = sources.execution_source(inventory(tool="e"))
        receipt = build_receipt(original)
        with patch.object(sources, "capture_inventory", return_value=inventory(tool="e")), \
                patch.object(restore_build, "build_context", return_value=build_context()):
            result = sources.reusable_artifact_source(Path("repository"), receipt)
        self.assertEqual(result, original["source"])
        result["snapshot"]["head"] = "0" * 40
        self.assertEqual(original["source"]["snapshot"]["head"], "c" * 40)

    def test_product_change_rejects_existing_artifact(self):
        original = inventory(product="c")
        current = sources.execution_source(inventory(product="0"))
        receipt = build_receipt(original)
        with patch.object(sources, "capture_inventory", return_value=inventory(product="0")), \
                patch.object(restore_build, "build_context", return_value=build_context()), \
                self.assertRaisesRegex(ValueError, "重新编译"):
            sources.reusable_artifact_source(Path("repository"), receipt)

    def test_build_parameter_change_rejects_existing_artifact(self):
        original = inventory(tool="d")
        receipt = build_receipt(original)
        changed = build_context()
        changed["jobs"] = "8"
        with patch.object(sources, "capture_inventory", return_value=inventory(tool="e")), \
                patch.object(restore_build, "build_context", return_value=changed), \
                self.assertRaisesRegex(ValueError, "构建命令、工具链"):
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
        self.enterContext(patch.object(restore_build, "build_context", return_value=build_context()))
        artifacts = {}
        for role, (feature, name) in restore_build.ROLES.items():
            binary = self.local / f"{name}.exe"
            binary.write_bytes(("fixed " + role).encode())
            command = ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                       "--features", feature, "--bin", name, "--message-format=json"]
            artifacts[role] = {"executable": str(binary), "command": command,
                               **restore_build.file_digest(binary)}
        self.receipt = build_receipt(self.original, artifacts)
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
        added["files"].sort(key=lambda item: item["path"])
        removed = {**self.original,
                   "files": [item for item in self.original["files"] if item["path"] != "scripts/check.py"]}
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
        self.assertEqual(sources.reusable_artifact_source(self.root, self.receipt), self.original["source"])
        with sources.artifact_sources(self.root, [binding]) as actual:
            self.assertEqual(actual, self.current)
            self.assertEqual(sources.reusable_artifact_source(self.root, self.receipt), self.original["source"])
            self.assertEqual(devex_provenance.verify_source(self.root, self.receipt,
                             self.original["source"]["worktree_fingerprint"]),
                             self.original["source"]["snapshot"])
            with self.assertRaisesRegex(ValueError, "构建时"):
                devex_provenance.verify_source(self.root, self.receipt, "sha256:" + "f" * 64)
            with self.assertRaisesRegex(ValueError, "嵌套"), sources.artifact_sources(self.root, []):
                pass
        self.assertEqual(sources.reusable_artifact_source(self.root, self.receipt), self.original["source"])
        self.assertEqual(self.build_path.read_bytes(), before)

    def test_xtask_only_change_uses_explicit_audited_inventory_without_rebuilding_product(self):
        self.set_current(inventory(tool="d", xtask="4"))
        binding = self.bridge()
        bridge = json.loads(Path(binding["path"]).read_text(encoding="utf-8"))
        self.assertEqual(bridge["product_inputs"]["build"], bridge["product_inputs"]["audited"])
        self.assertNotEqual(bridge["audited_inventory"], self.original)
        with sources.artifact_sources(self.root, [binding]):
            self.assertEqual(sources.reusable_artifact_source(self.root, self.receipt),
                             self.original["source"])

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

    def test_bridge_rejects_noncanonical_v2_artifact_fields(self):
        changed = copy.deepcopy(self.receipt)
        artifact = changed["artifacts"]["api"]
        artifact["cargo_executable"] = artifact["executable"]
        build_path = self.write("build-copy.json", changed)
        with self.assertRaisesRegex(ValueError, "产物字段无效"):
            sources.write_bridge(self.root, build_path, self.inventory_path, self.bridge_path)

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

    def test_maintenance_bridge_can_verify_controlled_isolated_worktree(self):
        isolated = self.local / "isolated-backend"
        build = isolated / ".local-tests/build/build.json"
        build.parent.mkdir(parents=True)
        build.write_text("{}", encoding="utf-8")
        receipt = {"kind": "devex-clone-tool-build", "backend_root": str(isolated)}

        with patch.object(devex_clone_tools, "verify_evidence") as verify:
            sources._verify_bridge_build(self.root, build, receipt)

        verify.assert_called_once_with(isolated.resolve(), build, receipt)

    def test_maintenance_bridge_rejects_uncontrolled_or_misplaced_worktree(self):
        isolated = self.local / "isolated-backend"
        allowed = isolated / ".local-tests/build/build.json"
        allowed.parent.mkdir(parents=True)
        allowed.write_text("{}", encoding="utf-8")
        receipt = {"kind": "devex-clone-tool-build", "backend_root": str(ROOT)}

        with self.assertRaisesRegex(ValueError, "当前 .local-tests"):
            sources._verify_bridge_build(self.root, allowed, receipt)

        receipt["backend_root"] = str(isolated)
        misplaced = self.local / "misplaced-build.json"
        misplaced.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "所属工作树"):
            sources._verify_bridge_build(self.root, misplaced, receipt)

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
            with self.assertRaisesRegex(ValueError, "字段无效"):
                sources.reusable_artifact_source(self.root, {**self.receipt, "other": True})

    def test_new_build_inventory_can_reuse_without_rebuilding_but_rejects_product_change(self):
        receipt = self.receipt
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
