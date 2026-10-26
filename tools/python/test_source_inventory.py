"""来源清单临时适配的协议、身份入口和返回类型测试；算法覆盖由 Rust 承担。"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import source_inventory as sources
import native_s3_client


class SourceInventoryAdapterTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(sources, "tool_binding", return_value={"fixture": True}))

    def test_missing_binding_rejects_before_transport_or_cargo_fallback(self):
        with patch.object(sources, "tool_binding", return_value=None), \
                patch.object(native_s3_client, "_invoke") as invoke, self.assertRaisesRegex(ValueError, "工具绑定"):
            sources.source_domain("Cargo.toml")
        invoke.assert_not_called()

    def test_every_inventory_operation_delegates_exact_read_only_protocol(self):
        root = Path("D:/source")
        inventory = {"files": []}
        domains = {"full": inventory}
        snapshot = {"head": "a" * 40}
        cases = [
            (lambda: sources.source_domain("Cargo.toml"), {"action": "domain", "path": "Cargo.toml"}, "product", "product"),
            (lambda: sources.fingerprints(inventory), {"action": "fingerprints", "files": []}, {}, {}),
            (lambda: sources.validate_inventory(inventory), {"action": "validate", "inventory": inventory}, {}, inventory),
            (lambda: sources.build_source_domains(inventory, "backend"),
             {"action": "domains", "inventory": inventory, "repository": "backend"}, domains, domains),
            (lambda: sources.validate_build_source_domains(domains, "frontend"),
             {"action": "validate-domains", "value": domains, "repository": "frontend"}, {}, domains),
            (lambda: sources.source_snapshot(root), {"action": "source-snapshot", "root": str(root)}, snapshot, snapshot),
            (lambda: sources.snapshot(root), {"action": "snapshot", "root": str(root)},
             {"snapshot": snapshot, "patch": [0, 255, 10]}, (snapshot, b"\x00\xff\n")),
            (lambda: sources.worktree_fingerprint(root, "a" * 40),
             {"action": "worktree-fingerprint", "root": str(root), "commit": "a" * 40}, "sha256:x", "sha256:x"),
            (lambda: sources.file_inventory(root), {"action": "file-inventory", "root": str(root)}, inventory, inventory),
            (lambda: sources.capture_inventory(root),
             {"action": "capture", "root": str(root), "source": None}, inventory, inventory),
            (lambda: sources.capture_inventory(root, snapshot),
             {"action": "capture", "root": str(root), "source": snapshot}, inventory, inventory),
        ]
        for call, arguments, result, expected in cases:
            response = {"ok": True, "result": {"value": result}}
            with self.subTest(arguments=arguments), patch.object(native_s3_client, "_invoke",
                    return_value=(SimpleNamespace(returncode=0), response)) as invoke:
                self.assertEqual(call(), expected)
                invoke.assert_called_once_with("__source-evidence", {
                    "operation": "source-inventory", "write": False, "arguments": arguments,
                }, subprocess.run, may_write=False)

    def test_native_rejection_is_propagated_without_python_fallback(self):
        response = {"ok": False, "error": {"category": "SourceEvidenceRejected",
            "message": "源码路径不能包含符号链接或 junction", "remote_write_may_have_occurred": False,
            "automatic_retry_permitted": False}}
        with patch.object(native_s3_client, "_invoke", return_value=(SimpleNamespace(returncode=1), response)), \
                self.assertRaisesRegex(ValueError, "junction"):
            sources.source_domain("nested/file")

    def test_transport_failure_propagates_without_git_or_algorithm_fallback(self):
        with patch.object(native_s3_client, "_invoke", side_effect=OSError("fixture unavailable")), \
                patch.object(sources, "git") as git, self.assertRaises(OSError):
            sources.capture_inventory(Path("D:/source"))
        git.assert_not_called()

    def test_shared_canonical_digest_preserves_utf8_compact_sorted_keys(self):
        value = [{"sha256": "a" * 64, "path": "中文 来源.rs"}]
        raw = ('[{"path":"中文 来源.rs","sha256":"' + "a" * 64 + '"}]').encode("utf-8")
        self.assertEqual(sources.canonical_digest(value), hashlib.sha256(raw).hexdigest())

    def test_shared_git_helper_retains_arguments_and_disables_optional_locks(self):
        with patch.object(subprocess, "check_output", return_value=b"raw\x00") as run:
            self.assertEqual(sources.git(Path("D:/source"), "ls-files", "-z"), b"raw\x00")
        self.assertEqual(run.call_args.args[0], ["git", "-C", "D:\\source", "ls-files", "-z"])
        self.assertEqual(run.call_args.kwargs["env"]["GIT_OPTIONAL_LOCKS"], "0")


class FrontendBuildContractNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from frontend_receipt_fixture import prepare_frontend, save_receipt
        from workspace_directory import WorkspaceDirectory
        cls.backend = Path(__file__).resolve().parents[2]
        cls.directory = WorkspaceDirectory(cls.backend / ".local-tests/python-unit", "source-contract-")
        cls.addClassCleanup(cls.directory.cleanup)
        cls.frontend = cls.directory.path
        prepare_frontend(cls.frontend)
        save_receipt(cls.frontend)

    def execute_contract(self):
        from rate_limit_tool import tool_binding, verified_tool
        from source_inventory import capture_inventory

        request = {"frontend_inventories": [capture_inventory(self.frontend)], "backend_inventories": []}
        with verified_tool(tool_binding()) as executable:
            return subprocess.run([executable, "__frontend-build-contract",
                                   "--frontend-dir", str(self.frontend)], input=json.dumps(request).encode(),
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1810, check=False)

    def test_frontend_contract_uses_existing_full_origin_authority(self):
        result = self.execute_contract()
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))
        value = json.loads(result.stdout)
        self.assertEqual(len(value["frontend_domains"]), 1)
        self.assertEqual(value["backend_domains"], [])
        self.assertTrue(value["frontend_build"]["files"])


if __name__ == "__main__":
    unittest.main()
