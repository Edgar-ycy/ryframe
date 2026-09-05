import copy
import functools
import http.server
import io
import json
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_build
import restore_runtime

ROOT = Path(__file__).resolve().parents[2]
SOURCE = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": [], "clean": True}


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass


class RestoreRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(restore_build, "capture_inventory", side_effect=lambda _root, source: {
            "source": {"snapshot": source, "worktree_fingerprint": "sha256:" + "c" * 64}, "files": []}))
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=local)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.backend, self.frontend = self.root / "backend", self.root / "frontend"
        self.backend.mkdir()
        (self.frontend / "dist/.vite").mkdir(parents=True)
        (self.frontend / "dist/index.html").write_text("<script src='/app.js'></script>")
        (self.frontend / "dist/app.js").write_text("console.log(42)")
        (self.frontend / "dist/.vite/manifest.json").write_text("{}")

    def cargo_run(self, command, **_kwargs):
        name = command[command.index("--bin") + 1]
        executable = self.backend / name
        executable.write_bytes(name.encode())
        event = {"reason": "compiler-artifact", "manifest_path": str(self.backend / "crates/ryframe/Cargo.toml"),
                 "target": {"name": name, "kind": ["bin"]}, "executable": str(executable)}
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(event))

    def test_frontend_checks_served_bytes_in_addition_to_disk_and_clean_source(self):
        handler = functools.partial(QuietHandler, directory=str(self.frontend / "dist"))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(lambda: (server.shutdown(), thread.join(timeout=5)))
        url = f"http://127.0.0.1:{server.server_port}"
        receipt = {"format_version": 1, "kind": "restore-frontend-build", "source": SOURCE,
                   "files": restore_runtime.frontend_files(self.frontend)}
        with patch.object(restore_runtime, "source_snapshot", return_value=SOURCE):
            restore_runtime.verify_frontend(self.frontend, receipt, SOURCE["head"], url)
            # 真实 HTTP 服务改为旧目录；磁盘上当前 dist 不变，仍须拒绝。
            old = self.root / "old-site"
            old.mkdir()
            (old / "index.html").write_text("old site")
            (old / "app.js").write_text("console.log('old site')")
            server.RequestHandlerClass = functools.partial(QuietHandler, directory=str(old))
            with self.assertRaisesRegex(ValueError, "实际返回的资源"):
                restore_runtime.verify_frontend(self.frontend, receipt, SOURCE["head"], url)

    def test_frontend_wrong_sha_dirty_or_changed_source_fail_before_http(self):
        receipt = {"format_version": 1, "kind": "restore-frontend-build", "source": SOURCE,
                   "files": restore_runtime.frontend_files(self.frontend)}
        for source, expected in ((SOURCE, "d" * 40), ({**SOURCE, "clean": False}, SOURCE["head"]),
                                 ({**SOURCE, "patch_sha256": "e" * 64}, SOURCE["head"])):
            with self.subTest(source=source, expected=expected), \
                    patch.object(restore_runtime, "source_snapshot", return_value=SOURCE), \
                    patch.object(restore_runtime.urllib.request, "build_opener") as opener:
                with self.assertRaisesRegex(ValueError, "精确干净 SHA"):
                    restore_runtime.verify_frontend(self.frontend, {**receipt, "source": source}, expected,
                                                    "http://127.0.0.1:4174")
                opener.assert_not_called()

    def test_process_restart_replaced_executable_and_wrong_port_are_rejected(self):
        identity = {"pid": 42, "started": "original", "executable": str(self.backend / "ryframe")}
        receipt = {"runtime_dir": str(self.root), "processes": {"api": identity, "worker": identity},
                   "backend": {"artifacts": {role: {"executable": identity["executable"]}
                                               for role in ("api", "worker")}}}
        record = {"plan": {"scope_id": "restore-test", "api_ready_url": "http://127.0.0.1:8080/readyz",
                           "worker_ready_url": "http://127.0.0.1:9091/readyz"}}
        for actual in (None, {**identity, "started": "restarted"}, {**identity, "executable": "other.exe"}):
            with patch.object(restore_runtime, "read_process", return_value=identity), \
                    patch.object(restore_runtime, "process_identity", return_value=actual), \
                    self.assertRaisesRegex(ValueError, "进程退出、重启"):
                restore_runtime.verify_processes(receipt, record)
        with patch.object(restore_runtime, "read_process", return_value=identity), \
                patch.object(restore_runtime, "process_identity", return_value=identity), \
                patch.object(restore_runtime, "verify_listener", side_effect=ValueError("wrong port")), \
                self.assertRaisesRegex(ValueError, "wrong port"):
            restore_runtime.verify_processes(receipt, record)

    def test_bindings_and_browser_target_must_match_receipt_before_any_probe(self):
        record = {"status": "data_verified", "plan_hash": "c" * 64,
                  "plan": {"id": "restore", "backup_id": "backup", "scope_id": "restore-test", "frontend_sha": "d" * 40}}
        bindings = self.root / "bindings.json"
        bindings.write_text(json.dumps({"record": record, "manifest": {"id": "backup", "source_sha": SOURCE["head"]}}))
        receipt = {"format_version": 1, "kind": "restore-runtime", "restore_id": "restore",
                   "plan_hash": record["plan_hash"], "scope_id": "restore-test", "backend_root": str(self.backend),
                   "frontend_root": str(self.frontend), "frontend_url": "http://127.0.0.1:4174",
                   "bindings_sha256": restore_build.file_digest(bindings)["sha256"]}
        for key in ("scope_id", "plan_hash", "bindings_sha256", "backend_root", "frontend_url"):
            changed = copy.deepcopy(receipt)
            changed[key] = "changed"
            with patch.object(restore_runtime, "verify_build") as build, self.assertRaisesRegex(ValueError, "不匹配"):
                restore_runtime.verify(changed, self.backend, self.frontend, bindings, "http://127.0.0.1:4174")
            build.assert_not_called()

    def test_bound_receipt_roundtrip_checks_the_production_files_again(self):
        with patch.object(restore_build, "source_snapshot", return_value=SOURCE):
            backend_build = restore_build.build(self.backend, self.cargo_run)
        build_path = self.root / "build.json"
        build_path.write_text(json.dumps(backend_build))
        frontend_build = {"format_version": 1, "kind": "restore-frontend-build", "source": SOURCE,
                          "files": restore_runtime.frontend_files(self.frontend)}
        frontend_receipt = self.frontend / "dist" / restore_runtime.FRONTEND_RECEIPT
        frontend_receipt.write_text(json.dumps(frontend_build))
        record = {"status": "data_verified", "plan_hash": "c" * 64,
                  "plan": {"id": "restore", "backup_id": "backup", "scope_id": "restore-test",
                           "frontend_sha": SOURCE["head"], "api_ready_url": "http://127.0.0.1:8080/readyz",
                           "worker_ready_url": "http://127.0.0.1:9091/readyz"}}
        bindings = self.root / "bindings.json"
        bindings.write_text(json.dumps({"record": record, "manifest": {"id": "backup", "source_sha": SOURCE["head"]}}))
        identities = {role: {"pid": index + 42, "started": "original", "executable": artifact["executable"]}
                      for index, (role, artifact) in enumerate(backend_build["artifacts"].items())}
        for role, identity in identities.items():
            (self.root / f"{role}.json").write_text(json.dumps({"format_version": 1, "role": role,
                                                            "scope_id": "restore-test", "identity": identity}))
        handler = functools.partial(QuietHandler, directory=str(self.frontend / "dist"))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(lambda: (server.shutdown(), thread.join(timeout=5)))
        url = f"http://127.0.0.1:{server.server_port}"
        with patch.object(restore_build, "source_snapshot", return_value=SOURCE), \
                patch.object(restore_runtime, "source_snapshot", return_value=SOURCE), \
                patch.object(restore_runtime, "process_identity", side_effect=lambda pid: next(
                    identity for identity in identities.values() if identity["pid"] == pid)), \
                patch.object(restore_runtime, "verify_listener"):
            receipt = restore_runtime.bind(self.backend, self.frontend, build_path, self.root, bindings, url)
            restore_runtime.verify(receipt, self.backend, self.frontend, bindings, url)
            frontend_receipt.write_text("{}")
            with self.assertRaisesRegex(ValueError, "构建收据已变化"):
                restore_runtime.verify(receipt, self.backend, self.frontend, bindings, url)

    def test_cli_requires_explicit_write_before_build_bind_or_receipt_publication(self):
        common = ["--backend-dir", str(self.backend), "--output", str(self.backend / ".local-tests/out.json")]
        binding = ["--frontend-dir", str(self.frontend), "--bindings", str(self.root / "bindings.json"),
                   "--frontend-url", "http://127.0.0.1:4174", "--build-receipt", str(self.root / "build.json"),
                   "--runtime-dir", str(self.root)]
        for operation, arguments in (("build", common), ("bind", common + binding)):
            with self.subTest(operation=operation), \
                    patch.object(sys, "argv", ["restore_runtime.py", operation, *arguments]), \
                    patch.object(sys, "stderr", io.StringIO()), \
                    patch.object(restore_runtime, "build") as build, \
                    patch.object(restore_runtime, "bind") as bind, \
                    patch.object(restore_runtime, "write_new") as write:
                with self.assertRaises(SystemExit) as error:
                    restore_runtime.main()
                self.assertEqual(error.exception.code, 2)
                build.assert_not_called()
                bind.assert_not_called()
                write.assert_not_called()

    def test_invalid_output_is_rejected_before_build_bind_or_resource_probes(self):
        outside = self.root / "outside.json"
        common = ["--backend-dir", str(self.backend), "--output", str(outside), "--write"]
        binding = ["--frontend-dir", str(self.frontend), "--bindings", str(self.root / "bindings.json"),
                   "--frontend-url", "http://127.0.0.1:4174",
                   "--build-receipt", str(self.root / "build.json"), "--runtime-dir", str(self.root)]
        for operation, arguments in (("build", common), ("bind", common + binding)):
            with self.subTest(operation=operation), \
                    patch.object(sys, "argv", ["restore_runtime.py", operation, *arguments]), \
                    patch.object(restore_runtime, "build") as build, \
                    patch.object(restore_runtime, "bind") as bind, \
                    patch.object(restore_runtime, "write_new") as write:
                with self.assertRaisesRegex(ValueError, "当前仓库忽略"):
                    restore_runtime.main()
                build.assert_not_called()
                bind.assert_not_called()
                write.assert_not_called()
                self.assertFalse(outside.exists())

    def test_readonly_cli_preserves_receipt_and_rejects_change_during_verification(self):
        receipt = self.root / "runtime.json"
        receipt.write_text('{"fixture": true}', encoding="utf-8")
        original = receipt.read_bytes()
        bindings = self.root / "bindings.json"
        arguments = ["restore_runtime.py", "verify", "--backend-dir", str(self.backend),
                     "--frontend-dir", str(self.frontend), "--bindings", str(bindings),
                     "--frontend-url", "http://127.0.0.1:4174", "--receipt", str(receipt)]
        with patch.object(sys, "argv", arguments), patch.object(sys, "stdout", io.StringIO()) as output, \
                patch.object(restore_runtime, "verify") as verify, \
                patch.object(restore_runtime, "write_new") as write:
            restore_runtime.main()
            verify.assert_called_once_with({"fixture": True}, self.backend, self.frontend, bindings,
                                           "http://127.0.0.1:4174")
            self.assertEqual(receipt.read_bytes(), original)
            self.assertEqual(json.loads(output.getvalue()),
                             {"runtime_receipt_sha256": restore_build.file_digest(receipt)["sha256"]})
            write.assert_not_called()
            verify.side_effect = lambda *_: receipt.write_text('{"fixture": "changed"}', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "运行收据被替换"):
                restore_runtime.main()
            write.assert_not_called()


if __name__ == "__main__":
    unittest.main()
