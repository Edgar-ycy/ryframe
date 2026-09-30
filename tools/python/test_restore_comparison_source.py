from pathlib import Path
import copy
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_comparison_source as comparison
from source_inventory import build_source_domains, canonical_digest


def inventory(head, product="1", tool="2", *, frontend=False):
    files = ([{"path": "package.json", "sha256": product * 64},
              {"path": "scripts/build.mjs", "sha256": tool * 64}]
             if frontend else
             [{"path": "Cargo.toml", "sha256": product * 64},
              {"path": "xtask/src/cli.rs", "sha256": tool * 64}])
    return {
        "source": {
            "snapshot": {"head": head, "patch_sha256": "0" * 64,
                         "files": [], "clean": True},
            "worktree_fingerprint": "sha256:" + "3" * 64,
        },
        "files": files,
        "guard": {"head": head, "index_sha256": "4" * 64, "modes_sha256": "5" * 64},
    }


def binding(name):
    return {"path": str((Path.cwd() / name).resolve()), "bytes": 1, "sha256": "6" * 64}


def export_identity():
    value = {
        "result": binding("export-result.json"), "origin_attempt": 7,
        "source_registration": binding("source-registration.json"),
        "source_rebind": binding("source-rebind.json"),
        "source_generation": binding("source-generation.json"),
        "review_successor": binding("review-successor.json"),
        "source_request": binding("source-request.json"), "export": binding("export.json"),
        "export_sha256": "7" * 64, "generation_sha256": "8" * 64,
        "logical_inventory_sha256": "9" * 64,
    }
    value["identity_sha256"] = canonical_digest(value)
    return value


def adapter():
    return {
        "contract": "legacy-stable-readiness-b0-v1",
        "base_backend_sha": comparison.B0_BACKEND_COMMIT,
        "base_frontend_sha": comparison.B0_FRONTEND_COMMIT,
        "reference_adapter_sha": comparison.B0_ADAPTER_COMMIT,
        "adapter_tree": comparison.B0_ADAPTER_TREE,
        "reconstructed_tree": comparison.B0_ADAPTER_TREE,
        "adapter_paths": comparison.B0_ADAPTER_PATHS,
        "patch": {"path": comparison.B0_ADAPTER_PATCH.as_posix(), "bytes": 1,
                  "sha256": comparison.B0_ADAPTER_PATCH_SHA256},
    }


class B0AdapterEvidenceTests(unittest.TestCase):
    def test_patch_ancestor_reparse_is_rejected_before_reading(self):
        root = Path(__file__).resolve().parents[2]
        original = Path.lstat
        def replaced(path, *args, **kwargs):
            value = original(path, *args, **kwargs)
            return SimpleNamespace(st_mode=value.st_mode, st_file_attributes=0x400) if path == root / "xtask/assets" else value
        with patch.object(Path, "lstat", replaced), patch.object(Path, "read_bytes") as read, self.assertRaises(ValueError):
            comparison._patch_bytes(root)
        read.assert_not_called()

    def test_reconstruction_rejects_scratch_reparse_before_git_writes(self):
        temporary = WorkspaceDirectory(Path(__file__).resolve().parents[2] / ".local-tests/python-unit")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "target").mkdir()
        original = Path.lstat
        def replaced(path, *args, **kwargs):
            value = original(path, *args, **kwargs)
            return SimpleNamespace(st_mode=value.st_mode, st_file_attributes=0x400) if path == root / "target" else value
        with patch.object(Path, "lstat", replaced), patch.object(comparison, "_git") as git, self.assertRaises(ValueError):
            comparison._reconstructed_tree(root, root / comparison.B0_ADAPTER_PATCH)
        git.assert_not_called()

    def test_readonly_adapter_verifies_embedded_reference_without_writing(self):
        root = Path(__file__).resolve().parents[2]
        directory = root / "target"
        before = {path.name: path.stat().st_mtime_ns for path in directory.iterdir()} if directory.exists() else None
        original = comparison._git
        calls = []
        def readonly_git(location, *arguments, **kwargs):
            calls.append(arguments)
            self.assertNotIn(arguments[0], {"read-tree", "write-tree", "apply", "hash-object", "update-index"})
            self.assertIsNone(kwargs.get("index"))
            return original(location, *arguments, **kwargs)
        with patch.object(comparison, "_git", side_effect=readonly_git), \
                patch.object(comparison, "_reconstructed_tree", side_effect=AssertionError("只读不能重建")):
            observed = comparison.b0_adapter_evidence(root, reconstruct=False)
        self.assertEqual(observed["reconstructed_tree"], comparison.B0_ADAPTER_TREE)
        self.assertEqual(before, {path.name: path.stat().st_mtime_ns for path in directory.iterdir()} if directory.exists() else None)
        self.assertTrue(any(arguments[:2] == ("rev-parse", "--show-toplevel") for arguments in calls))
        self.assertTrue(any(arguments[0] == "rev-parse" for arguments in calls))

    def test_readonly_adapter_rejects_wrong_base_tree_or_patch(self):
        root = Path(__file__).resolve().parents[2]
        original = comparison._text
        def changed(location, *arguments, **kwargs):
            return "f" * 40 if f"{comparison.B0_BACKEND_COMMIT}^{{tree}}" in arguments else original(location, *arguments, **kwargs)
        with patch.object(comparison, "_text", side_effect=changed), self.assertRaises(ValueError):
            comparison.b0_adapter_evidence(root, reconstruct=False)
        with patch.object(comparison, "_patch_bytes", return_value=b"changed"), self.assertRaises(ValueError):
            comparison.b0_adapter_evidence(root, reconstruct=False)

    def test_registered_adapter_is_reconstructed_from_embedded_patch(self):
        root = Path(__file__).resolve().parents[2]
        value = comparison.b0_adapter_evidence(root)
        self.assertEqual(value["base_backend_sha"], comparison.B0_BACKEND_COMMIT)
        self.assertEqual(value["base_frontend_sha"], comparison.B0_FRONTEND_COMMIT)
        self.assertEqual(value["reference_adapter_sha"], comparison.B0_ADAPTER_COMMIT)
        self.assertEqual(value["adapter_tree"], comparison.B0_ADAPTER_TREE)
        self.assertEqual(value["adapter_paths"], ["xtask/src/cli.rs"])
        self.assertEqual(value["patch"]["sha256"], comparison.B0_ADAPTER_PATCH_SHA256)

    def test_changed_embedded_patch_is_rejected(self):
        root = Path(__file__).resolve().parents[2]
        with patch.object(comparison, "_patch_bytes", return_value=b"changed"), \
                self.assertRaisesRegex(ValueError, "不匹配"):
            comparison.b0_adapter_evidence(root)


class ComparisonSourceTests(unittest.TestCase):
    def setUp(self):
        self.root = Path.cwd().resolve()
        self.base = inventory(comparison.B0_BACKEND_COMMIT, tool="1")
        self.adapter_source = inventory(comparison.B0_ADAPTER_COMMIT, tool="2")
        self.frontend = inventory(comparison.B0_FRONTEND_COMMIT, frontend=True)
        self.export = export_identity()

    def test_b0_separates_original_product_source_from_adapter_execution(self):
        receipt = {"sources": build_source_domains(self.adapter_source, "backend")}
        with patch.object(comparison, "_inventory", side_effect=[
                (self.root / "base", self.base), (self.root / "adapter", self.adapter_source),
                (self.root / "frontend", self.frontend)]), \
                patch.object(comparison, "_backend_build", return_value=(receipt, binding("b0-backend.json"))), \
                patch.object(comparison, "_frontend_build", return_value=({}, binding("b0-frontend.json"))), \
                patch.object(comparison, "b0_adapter_evidence", return_value=adapter()):
            arm = comparison._b0_arm(self.root, *(self.root / name for name in (
                "base", "adapter", "frontend", "b0-backend.json", "b0-frontend.json")), self.export)
        self.assertEqual(arm["sources"]["backend"], self.base)
        self.assertEqual(arm["execution_sources"]["backend"], self.adapter_source)
        self.assertEqual(arm["source_export_identity_sha256"], self.export["identity_sha256"])

    def test_b0_rejects_adapter_that_changes_product_inputs(self):
        changed = inventory(comparison.B0_ADAPTER_COMMIT, product="a", tool="2")
        receipt = {"sources": build_source_domains(changed, "backend")}
        with patch.object(comparison, "_inventory", side_effect=[
                (self.root / "base", self.base), (self.root / "adapter", changed),
                (self.root / "frontend", self.frontend)]), \
                patch.object(comparison, "_backend_build", return_value=(receipt, binding("b0-backend.json"))), \
                patch.object(comparison, "_frontend_build", return_value=({}, binding("b0-frontend.json"))), \
                self.assertRaisesRegex(ValueError, "产品输入"):
            comparison._b0_arm(self.root, *(self.root / name for name in (
                "base", "adapter", "frontend", "b0-backend.json", "b0-frontend.json")), self.export)

    def test_source_export_consumer_uses_published_result_without_exposing_storage(self):
        descriptor = binding("result.json")
        value = {
            "review_successor": binding("successor.json"), "origin_attempt": 4,
            "source_registration": binding("registration.json"), "source_rebind": binding("rebind.json"),
            "source_generation": binding("generation.json"),
            "source_request": binding("request.json"), "export": binding("payload.json"),
            "summary": {"export_sha256": "a" * 64, "generation_sha256": "b" * 64,
                        "logical_inventory_sha256": "c" * 64}, "source_storage": {"secret": "hidden"},
        }
        document = unittest.mock.Mock(
            value=value, path=Path(descriptor["path"]), raw=b"x",
            sha256=descriptor["sha256"], unsafe=True,
        )
        with patch.object(comparison, "bound_file", return_value=document.path), \
                patch.object(comparison, "read_json_document", return_value=document), \
                patch.object(comparison, "published_source", return_value={"source": True}) as source, \
                patch.object(comparison, "published_export", return_value=value):
            result = comparison._source_export(self.root, descriptor)
        source.assert_called_once_with(self.root, value["review_successor"], live_storage=False)
        self.assertNotIn("source_storage", result)
        self.assertNotIn("hidden", repr(result))
        self.assertEqual(result["result"], descriptor)
        self.assertEqual(result["identity_sha256"], canonical_digest(
            {key: item for key, item in result.items() if key != "identity_sha256"}))

    def test_frontend_build_rejects_environment_change_during_verification(self):
        receipt = {
            "sources": {"full": self.frontend},
            "build": {"environment_files": []},
            "files": [{"path": "index.html", "bytes": 1, "sha256": "a" * 64}],
        }
        unchanged = unittest.mock.Mock()
        document = unittest.mock.Mock(
            value=receipt, raw=b"{}", sha256="b" * 64, assert_unchanged=unchanged,
        )
        with patch.object(comparison, "_build_document", return_value=document), \
                patch.object(comparison, "validate_frontend_receipt", return_value=receipt), \
                patch.object(comparison, "frontend_environment_files", side_effect=[[], [
                    {"path": ".env.production", "sha256": "c" * 64},
                ]]), \
                patch.object(comparison, "frontend_files", return_value=receipt["files"]), \
                patch.object(comparison, "capture_inventory", return_value=self.frontend), \
                self.assertRaisesRegex(ValueError, "环境文件"):
            comparison._frontend_build(self.root, self.root / "receipt.json", self.frontend)
        unchanged.assert_not_called()

    def test_manifest_binds_both_arms_to_one_export_and_rejects_extra_fields(self):
        b1_backend = inventory("d" * 40)
        b1_frontend = inventory("e" * 40, frontend=True)
        b0 = comparison._arm(
            {"source_backend": self.root / "a", "execution_backend": self.root / "b",
             "frontend": self.root / "c"},
            {"backend": self.base, "frontend": self.frontend},
            {"backend": self.adapter_source, "frontend": self.frontend},
            {"backend": binding("b0-backend.json"), "frontend": binding("b0-frontend.json")},
            adapter(), self.export)
        b1 = comparison._arm(
            {"source_backend": self.root / "d", "execution_backend": self.root / "d",
             "frontend": self.root / "e"},
            {"backend": b1_backend, "frontend": b1_frontend},
            {"backend": b1_backend, "frontend": b1_frontend},
            {"backend": binding("b1-backend.json"), "frontend": binding("b1-frontend.json")},
            None, self.export)
        with patch.object(comparison, "_repository", return_value=self.root), \
                patch.object(comparison, "_source_export", side_effect=[self.export, self.export]), \
                patch.object(comparison, "_b0_arm", side_effect=[b0, b0]), \
                patch.object(comparison, "_b1_arm", side_effect=[b1, b1]):
            manifest = comparison.capture_comparison_sources(
                self.root, b0_backend=self.root / "a", b0_adapter_backend=self.root / "b",
                b0_frontend=self.root / "c", b0_backend_build=self.root / "b0-backend.json",
                b0_frontend_build=self.root / "b0-frontend.json", b1_backend=self.root / "d",
                b1_frontend=self.root / "e", b1_backend_build=self.root / "b1-backend.json",
                b1_frontend_build=self.root / "b1-frontend.json", source_export_result=self.export["result"])
        self.assertEqual({arm["source_export_identity_sha256"] for arm in manifest["arms"].values()},
                         {self.export["identity_sha256"]})
        with patch.object(comparison, "capture_comparison_sources", return_value=manifest) as capture:
            self.assertIs(comparison.verify_comparison_sources(self.root, manifest), manifest)
            self.assertIs(capture.call_args.kwargs["reconstruct_adapter"], False)
            comparison.verify_comparison_sources(self.root, manifest, read_only=False)
            self.assertIs(capture.call_args.kwargs["reconstruct_adapter"], True)
        invalid = copy.deepcopy(manifest)
        invalid["legacy"] = True
        with patch.object(comparison, "capture_comparison_sources") as capture, \
                self.assertRaises(ValueError):
            comparison.verify_comparison_sources(self.root, invalid)
        capture.assert_not_called()

        invalid = copy.deepcopy(manifest)
        invalid["arms"]["b0"]["adapter"].pop("reconstructed_tree")
        with patch.object(comparison, "capture_comparison_sources") as capture, self.assertRaises(ValueError):
            comparison.verify_comparison_sources(self.root, invalid)
        capture.assert_not_called()

        invalid = copy.deepcopy(manifest)
        invalid["arms"]["b1"]["source_export_result_sha256"] = "0" * 64
        with patch.object(comparison, "capture_comparison_sources") as capture, \
                self.assertRaisesRegex(ValueError, "同一 source-export"):
            comparison.verify_comparison_sources(self.root, invalid)
        capture.assert_not_called()


if __name__ == "__main__":
    unittest.main()
