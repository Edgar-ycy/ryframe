"""离线证明性能来源严格绑定实际产物，且不会放宽正式恢复。"""

import copy
import hashlib
import io
import json
import struct
import subprocess
import sys
import unittest
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch
from urllib.parse import unquote, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_provenance as provenance
import restore_build
import restore_runtime
from tests.workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
SOURCE = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": [], "clean": False}
FINGERPRINT = "sha256:" + "c" * 64
DRIVER_SOURCE = {
    "snapshot": SOURCE,
    "worktree_fingerprint": FINGERPRINT,
    "fingerprints": {
        name: {"sha256": character * 64, "files": 1}
        for name, character in (("product", "d"), ("test_tools", "e"), ("support", "f"))
    },
}


class Response(io.BytesIO):
    status = 200


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(restore_build, "capture_inventory", side_effect=lambda _root, source: {
            "source": {"snapshot": source, "worktree_fingerprint": FINGERPRINT}, "files": []}))
        self.enterContext(patch.object(provenance, "reusable_artifact_source", return_value=None))
        self.driver_source = self.enterContext(
            patch.object(provenance, "current_execution_source", return_value=DRIVER_SOURCE)
        )
        local = ROOT / ".local-tests/python-unit"
        temporary = WorkspaceDirectory(local)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.backend, self.frontend, self.runtime = [self.root / name for name in ("backend", "frontend", "runtime")]
        self.backend.mkdir()
        self.runtime.mkdir()
        (self.frontend / "dist/.vite").mkdir(parents=True)
        for name, value in (("index.html", "current index"), ("app.js", "current JS"), (".vite/manifest.json", "{}")):
            (self.frontend / "dist" / name).write_text(value)
        with patch.object(restore_build, "source_snapshot", return_value=SOURCE):
            build = restore_build.build(self.backend, self.cargo_run)
        backend_binding = self.write(self.root / "build.json", build)
        frontend_binding = self.write(self.frontend / "dist" / restore_runtime.FRONTEND_RECEIPT,
                                      {"format_version": 1, "kind": "restore-frontend-build", "source": SOURCE,
                                       "files": restore_runtime.frontend_files(self.frontend)})
        self.identities = {role: {"pid": 42 + index, "started": "original", "executable": artifact["executable"]}
                           for index, (role, artifact) in enumerate(build["artifacts"].items())}
        process_hashes = {role: self.write(self.runtime / f"{role}.json", {
            "format_version": 1, "role": role, "scope_id": "perf-isolated", "identity": identity})["sha256"]
            for role, identity in self.identities.items()}
        self.runtime_receipt = {"format_version": 1, "scope_id": "perf-isolated", "backend_root": str(self.backend),
                                "configuration_sha256": "f" * 64, "worker_ready_url": "http://127.0.0.1:19093/readyz",
                                "artifacts": {role: {"path": value["executable"], "sha256": value["sha256"]}
                                              for role, value in build["artifacts"].items()}}
        runtime_binding = self.write(self.runtime / "runtime.json", self.runtime_receipt)
        environment = self.root / "environment.md"
        environment.write_text("# 已登记测试环境\n硬件、存储、网络与初始数据集由操作人员核实。", encoding="utf-8")
        self.request = {"backend": str(self.backend), "frontend": str(self.frontend), "driver": str(self.backend),
                        "driver_fingerprint": FINGERPRINT, "scope_id": "perf-isolated",
                        "source_fingerprints": {"backend": FINGERPRINT, "frontend": FINGERPRINT},
                        "api_url": "http://127.0.0.1:18082", "frontend_url": "http://127.0.0.1:4176",
                        "metrics_urls": {"api": "http://127.0.0.1:18082/metrics", "worker": "http://127.0.0.1:19093/metrics"},
                        "environment_sha256": restore_build.file_digest(environment)["sha256"],
                        "provenance": {"backend_build": backend_binding, "runtime": runtime_binding,
                                       "processes": process_hashes, "frontend_build_sha256": frontend_binding["sha256"],
                                       "environment_document": str(environment)}}
        stack = self.enterContext(ExitStack())
        self.source = stack.enter_context(patch.object(provenance, "source_snapshot", return_value=SOURCE))
        stack.enter_context(patch.object(provenance, "worktree_fingerprint", return_value=FINGERPRINT))
        self.runtime_check = stack.enter_context(patch.object(provenance, "verify_runtime", return_value=self.runtime_receipt))
        self.process = stack.enter_context(patch.object(provenance, "process_identity", side_effect=lambda pid: next(
            identity for identity in self.identities.values() if identity["pid"] == pid)))
        self.listener = stack.enter_context(patch.object(provenance, "verify_listener", side_effect=self.listen))
        opener = stack.enter_context(patch.object(restore_runtime.urllib.request, "build_opener"))
        self.http = opener.return_value.open
        self.http.side_effect = lambda request, **_kwargs: Response(
            (self.frontend / "dist" / unquote(urlsplit(request.full_url).path).lstrip("/")).read_bytes())

    def write(self, path, value):
        path.write_text(json.dumps(value), encoding="utf-8")
        return {"path": str(path), "sha256": restore_build.file_digest(path)["sha256"]}

    def cargo_run(self, command, **_kwargs):
        name = command[command.index("--bin") + 1]
        binary = self.backend / name
        binary.write_bytes(name.encode())
        event = {"reason": "compiler-artifact", "manifest_path": str(self.backend / "crates/ryframe/Cargo.toml"),
                 "target": {"name": name, "kind": ["bin"]}, "executable": str(binary)}
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(event))

    def listen(self, pid, url):
        if urlsplit(url).port != {42: 18082, 43: 19093}[pid]:
            raise ValueError("wrong listener")

    def test_exact_dirty_development_snapshot_is_read_only_and_not_formal_restore(self):
        before = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        result = provenance.verify(self.request)
        self.assertFalse(result["sources"]["backend"]["clean"])
        self.assertEqual(result["sources"]["driver"], SOURCE)
        self.assertEqual(result["execution_source"], DRIVER_SOURCE)
        self.assertEqual(result["environment_evidence"], "operator_declared_document")
        self.assertEqual(before, {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()})
        backend = json.loads((self.root / "build.json").read_text())
        frontend = json.loads((self.frontend / "dist" / restore_runtime.FRONTEND_RECEIPT).read_text())
        with patch.object(restore_build, "source_snapshot", return_value=SOURCE), self.assertRaisesRegex(ValueError, "干净 SHA"):
            restore_build.verify_build(self.backend, backend, SOURCE["head"])
        with patch.object(restore_runtime, "source_snapshot", return_value=SOURCE), self.assertRaisesRegex(ValueError, "干净 SHA"):
            restore_runtime.verify_frontend(self.frontend, frontend, SOURCE["head"], self.request["frontend_url"])

    def test_old_binary_and_old_runtime_artifact_are_rejected(self):
        (self.backend / "ryframe").write_bytes(b"old binary")
        with self.assertRaisesRegex(provenance.ProvenanceError, "backend_artifacts"):
            provenance.verify(self.request)
        self.cargo_run(["--bin", "ryframe"])
        changed = copy.deepcopy(self.runtime_receipt)
        changed["artifacts"]["worker"]["sha256"] = "0" * 64
        self.request["provenance"]["runtime"] = self.write(self.runtime / "runtime.json", changed)
        self.runtime_check.return_value = changed
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_configuration"):
            provenance.verify(self.request)

    def test_same_head_dirty_change_and_wrong_xtask_fingerprint_are_rejected(self):
        for field, value in (("patch_sha256", "d" * 64), ("files", [{"path": "new.rs", "sha256": "e" * 64}])):
            self.source.return_value = {**SOURCE, field: value}
            with self.assertRaisesRegex(provenance.ProvenanceError, "backend_source"):
                provenance.verify(self.request)
        self.source.return_value = SOURCE
        self.request["source_fingerprints"]["backend"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(provenance.ProvenanceError, "backend_source"):
            provenance.verify(self.request)

    def test_driver_source_fingerprint_and_verification_window_are_exact(self):
        self.request["driver_fingerprint"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(provenance.ProvenanceError, "driver_source"):
            provenance.verify(self.request)
        self.request["driver_fingerprint"] = FINGERPRINT
        self.driver_source.side_effect = [
            DRIVER_SOURCE,
            {**DRIVER_SOURCE, "worktree_fingerprint": "sha256:" + "1" * 64},
        ]
        with self.assertRaisesRegex(provenance.ProvenanceError, "driver_source_stable"):
            provenance.verify(self.request)

    def test_unknown_or_missing_driver_binding_is_rejected_before_receipt_access(self):
        for mutate in (
            lambda value: value.pop("driver"),
            lambda value: value.update(extra="unexpected"),
            lambda value: value.update(driver_fingerprint="invalid"),
        ):
            changed = copy.deepcopy(self.request)
            mutate(changed)
            with self.assertRaisesRegex(provenance.ProvenanceError, "bindings"):
                provenance.verify(changed)

    def test_process_creation_identity_and_each_actual_endpoint_are_checked(self):
        self.process.side_effect = None
        for actual in (None, {**self.identities["api"], "started": "reused PID"}):
            self.process.return_value = actual
            with self.assertRaisesRegex(provenance.ProvenanceError, "processes"):
                provenance.verify(self.request)
        self.process.side_effect = lambda pid: self.identities["api" if pid == 42 else "worker"]
        for field in ("api_url", "api", "worker"):
            changed = copy.deepcopy(self.request)
            (changed if field == "api_url" else changed["metrics_urls"])[field] = "http://127.0.0.1:18083/metrics"
            with self.assertRaisesRegex(provenance.ProvenanceError, "processes"):
                provenance.verify(changed)

    def test_old_frontend_disk_and_old_http_response_are_rejected(self):
        self.http.side_effect = lambda *_args, **_kwargs: Response(b"old site")
        with self.assertRaisesRegex(provenance.ProvenanceError, "frontend_artifacts"):
            provenance.verify(self.request)
        (self.frontend / "dist/app.js").write_text("old disk")
        self.http.reset_mock()
        with self.assertRaisesRegex(provenance.ProvenanceError, "frontend_artifacts"):
            provenance.verify(self.request)
        self.http.assert_not_called()

    def test_frontend_old_snapshot_rejected_even_when_receipt_hash_is_rebound(self):
        filename = self.frontend / "dist" / restore_runtime.FRONTEND_RECEIPT
        receipt = json.loads(filename.read_text())
        receipt["source"]["patch_sha256"] = "0" * 64
        self.request["provenance"]["frontend_build_sha256"] = self.write(filename, receipt)["sha256"]
        with self.assertRaisesRegex(provenance.ProvenanceError, "frontend_source"):
            provenance.verify(self.request)

    def test_environment_document_and_all_receipts_are_explicitly_hashed(self):
        changed = copy.deepcopy(self.request)
        changed["environment_sha256"] = "0" * 64
        with self.assertRaisesRegex(provenance.ProvenanceError, "environment_document"):
            provenance.verify(changed)
        (self.runtime / "api.json").write_text("{}")
        with self.assertRaisesRegex(provenance.ProvenanceError, "api_receipt"):
            provenance.verify(self.request)

    def test_changes_within_one_verification_are_rejected(self):
        self.source.side_effect = [SOURCE, SOURCE, {**SOURCE, "patch_sha256": "0" * 64}]
        with self.assertRaisesRegex(provenance.ProvenanceError, "backend_source_stable"):
            provenance.verify(self.request)

    def test_environment_change_and_config_mismatch_do_not_become_current_evidence(self):
        original = self.http.side_effect

        def changed(request, **kwargs):
            Path(self.request["provenance"]["environment_document"]).write_text("changed environment")
            return original(request, **kwargs)

        self.http.side_effect = changed
        with self.assertRaisesRegex(provenance.ProvenanceError, "environment_document_stable"):
            provenance.verify(self.request)
        self.request["environment_sha256"] = restore_build.file_digest(
            Path(self.request["provenance"]["environment_document"]))["sha256"]
        self.runtime_check.side_effect = ValueError("private APP configuration differs")
        with self.assertRaisesRegex(provenance.ProvenanceError, "runtime_configuration"):
            provenance.verify(self.request)

    def test_no_clean_bypass_switch_and_cli_failure_does_not_echo_private_input(self):
        self.request["provenance"]["allow_dirty"] = True
        with self.assertRaisesRegex(provenance.ProvenanceError, "bindings"):
            provenance.verify(self.request)
        result = subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "scripts/devex_provenance.py")],
                                input=json.dumps({"backend": "private-password=secret"}), capture_output=True,
                                text=True, encoding="utf-8", timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout), {"ok": False, "stage": "bindings"})
        self.assertEqual(result.stderr, "")


class FingerprintTests(unittest.TestCase):
    def test_xtask_length_framing_and_raw_git_order_include_untracked_content(self):
        local = ROOT / ".local-tests/python-unit"
        with WorkspaceDirectory(local) as directory:
            root = Path(directory).resolve()
            (root / "z.rs").write_bytes(b"z content")
            (root / "a.rs").write_bytes(b"a content")
            chunks = [b"a" * 40, b"binary patch\0", b"z.rs", b"z content", b"a.rs", b"a content"]
            expected = hashlib.sha256(b"".join(struct.pack("<Q", len(value)) + value for value in chunks)).hexdigest()
            with patch.object(provenance, "git", side_effect=[chunks[1], b"z.rs\0a.rs\0"]) as git:
                self.assertEqual(provenance.worktree_fingerprint(root, SOURCE["head"]), "sha256:" + expected)
            self.assertEqual(git.call_args_list[0].args[1:], ("diff", "--binary", "--no-ext-diff", "HEAD", "--", "."))
            with patch.object(provenance, "git", side_effect=[b"", b"../outside\0"]), self.assertRaises(ValueError):
                provenance.worktree_fingerprint(root, SOURCE["head"])


if __name__ == "__main__":
    unittest.main()
