"""来源清单临时适配的协议、身份入口和返回类型测试；算法覆盖由 Rust 承担。"""
from contextlib import contextmanager
import hashlib
import io
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
import source_domain_contract as contract


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


class SourceDomainContractBindingTests(unittest.TestCase):
    def setUp(self):
        self.active = False
        self.program = Path("D:/verified/xtask.exe")
        self.digest = {"bytes": 123, "sha256": "a" * 64}
        self.binding = {"format_version": 1, "kind": "ryframe-native-rate-limit-tool",
                        "path": str(self.program), **self.digest}
        self.origin = self.enterContext(patch.object(contract, "_verified_tool", side_effect=self.authority))
        self.enterContext(patch.object(contract, "file_digest", side_effect=self.read_digest))
        self.existing = self.enterContext(patch.object(contract, "tool_binding", return_value=None))
        self.install = self.enterContext(patch.object(contract, "install_binding"))

    @contextmanager
    def authority(self):
        self.active = True
        try:
            yield self.program
        finally:
            self.active = False

    def read_digest(self, path):
        self.assertTrue(self.active)
        self.assertEqual(path, self.program)
        return dict(self.digest)

    def test_missing_binding_installs_only_exact_program_while_full_authority_is_held(self):
        with contract._source_tool():
            self.assertTrue(self.active)
            self.assertEqual(json.loads(self.install.call_args.args[0]), self.binding)
        self.assertFalse(self.active)
        self.origin.assert_called_once_with()
        self.install.assert_called_once()

    def test_existing_same_authority_is_reused_without_second_installation(self):
        self.existing.return_value = self.binding
        with contract._source_tool():
            self.assertTrue(self.active)
        self.install.assert_not_called()

    def test_conflicting_path_size_or_digest_cannot_overwrite_existing_binding(self):
        for field, value in [("path", "D:/other/xtask.exe"), ("bytes", 124), ("sha256", "b" * 64)]:
            self.existing.return_value = self.binding | {field: value}
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "authority 不一致"):
                with contract._source_tool():
                    self.fail("不能执行契约")
        self.install.assert_not_called()

    def test_full_source_rejection_occurs_before_program_digest_or_installation(self):
        self.origin.side_effect = ValueError("完整来源文件集合漂移")
        with self.assertRaisesRegex(ValueError, "完整来源"):
            with contract._source_tool():
                self.fail("不能执行契约")
        self.install.assert_not_called()

    def invoke_main(self):
        request = {"frontend_inventories": [{"fixture": "frontend"}],
                   "backend_inventories": [{"fixture": "backend"}]}
        output = io.BytesIO()
        self.enterContext(patch.object(sys, "stdin", SimpleNamespace(buffer=io.BytesIO(json.dumps(request).encode()))))
        self.enterContext(patch.object(sys, "stdout", SimpleNamespace(buffer=output)))
        self.enterContext(patch.object(sys, "argv", ["source_domain_contract.py", "--frontend-dir", "D:/frontend"]))
        self.enterContext(patch.object(contract, "ordinary", return_value=Path("D:/frontend")))
        return output

    def test_complete_contract_domains_and_real_validator_remain_inside_authority(self):
        output = self.invoke_main()
        def domains(value, repository):
            self.assertTrue(self.active)
            return {"repository": repository, "full": value}
        def validate(root):
            self.assertTrue(self.active)
            self.assertEqual(root, Path("D:/frontend"))
            return {}, SimpleNamespace(value={"build": {"configuration": {"selected": "test"},
                                                         "identity": {"frontend_commit": "a" * 40}},
                                              "files": [{"path": "index.html"}]})
        with patch.object(contract, "build_source_domains", side_effect=domains) as classify, \
                patch.object(contract, "validate_frontend_build", side_effect=validate) as validator:
            contract.main()
        self.assertFalse(self.active)
        self.assertEqual(classify.call_count, 2)
        validator.assert_called_once()
        self.assertEqual(json.loads(output.getvalue())["frontend_build"]["files"], [{"path": "index.html"}])

    def test_authority_exit_failure_prevents_publishing_contract_response(self):
        output = self.invoke_main()
        @contextmanager
        def changed():
            yield self.program
            raise ValueError("调用期间完整来源漂移")
        with patch.object(contract, "_verified_tool", side_effect=changed), \
                patch.object(contract, "file_digest", return_value=self.digest), \
                patch.object(contract, "build_source_domains", return_value={}), \
                patch.object(contract, "validate_frontend_build", return_value=({}, SimpleNamespace(
                    value={"build": {"configuration": {}, "identity": {}}, "files": []}))), \
                self.assertRaisesRegex(ValueError, "完整来源漂移"):
            contract.main()
        self.assertEqual(output.getvalue(), b"")

class SourceDomainContractNativeTests(unittest.TestCase):
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
        import subprocess
        from source_inventory import capture_inventory
        request = {"frontend_inventories": [capture_inventory(self.frontend)], "backend_inventories": []}
        return subprocess.run([sys.executable, "-B", str(self.backend / "tools/python/source_domain_contract.py"),
                               "--frontend-dir", str(self.frontend)], input=json.dumps(request).encode(),
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=1810, check=False)

    def test_fresh_frontend_contract_derives_binding_from_real_full_origin_authority(self):
        result = self.execute_contract()
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))
        value = json.loads(result.stdout)
        self.assertEqual(len(value["frontend_domains"]), 1)
        self.assertEqual(value["backend_domains"], [])
        self.assertTrue(value["frontend_build"]["files"])

    def test_real_source_digest_mismatch_rejects_even_when_program_digest_is_unchanged(self):
        import frontend_origin_tool as origin
        binding = self.backend / origin.BINDING
        raw = binding.read_bytes()
        value = json.loads(raw)
        value["sources"][0]["sha256"] = "0" * 64
        try:
            binding.write_text(json.dumps(value), encoding="utf-8")
            result = self.execute_contract()
        finally:
            binding.write_bytes(raw)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"")
        self.assertIn("编译来源", result.stderr.decode("utf-8", errors="replace"))


if __name__ == "__main__":
    unittest.main()
