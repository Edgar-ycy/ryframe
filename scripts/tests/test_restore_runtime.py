import contextlib
import copy
import functools
import http.server
import io
import json
import subprocess
import sys
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import restore_build
import restore_runtime
import restore_runtime_evidence
import source_inventory
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
SOURCE = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": [], "clean": True}
INVENTORY = {
    "source": {"snapshot": SOURCE, "worktree_fingerprint": "sha256:" + "c" * 64},
    "files": [],
    "guard": {"head": SOURCE["head"], "index_sha256": "d" * 64, "modes_sha256": "e" * 64},
}
READY = {"status": "ready", "mysql": "up", "redis": "optional_degraded", "object_storage": "not_required"}


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass


def response_handler(routes):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            status, content_type, body, headers = routes.get(self.path, (404, "text/plain", b"missing", {}))
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            for key, value in headers.items():
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass

    return Handler


class RestoreRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.enterContext(
            patch.object(
                restore_build,
                "capture_inventory",
                side_effect=lambda *_args: copy.deepcopy(INVENTORY),
            )
        )
        self.enterContext(
            patch.object(
                restore_runtime,
                "capture_inventory",
                side_effect=lambda *_args: copy.deepcopy(INVENTORY),
            )
        )
        local = ROOT / ".local-tests/python-unit"
        self.directory = WorkspaceDirectory(local)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.backend, self.frontend = self.root / "backend", self.root / "frontend"
        self.backend.mkdir()
        (self.frontend / "dist/.vite").mkdir(parents=True)
        (self.frontend / "dist/index.html").write_text("<script src='/app.js'></script>", encoding="utf-8")
        (self.frontend / "dist/app.js").write_text("console.log(42)", encoding="utf-8")
        (self.frontend / "dist/.vite/manifest.json").write_text("{}", encoding="utf-8")
        self.identities = {}

    def write_json(self, path, value):
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def cargo_run(self, command, **_kwargs):
        if command in (["rustc", "-vV"], ["cargo", "-V"]):
            return subprocess.run(command, **_kwargs)
        name = command[command.index("--bin") + 1]
        executable = self.backend / name
        executable.write_bytes(name.encode())
        event = {
            "reason": "compiler-artifact",
            "manifest_path": str(self.backend / "crates/ryframe/Cargo.toml"),
            "target": {"name": name, "kind": ["bin"]},
            "executable": str(executable),
        }
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(event))

    def authority(self, **changes):
        value = {
            "format_version": 1,
            "kind": "restore-runtime-authority",
            "restore_id": "restore-one",
            "backup_id": "backup-one",
            "plan_hash": "c" * 64,
            "scope_id": "restore-test",
            "data_verified_at": "2026-09-06T08:00:00+08:00",
            "backend_sha": SOURCE["head"],
            "frontend_sha": SOURCE["head"],
            "api_endpoint": "http://127.0.0.1:18080/readyz",
            "worker_endpoint": "http://127.0.0.1:19091/readyz",
            "frontend_endpoint": "http://127.0.0.1:14174",
        }
        value.update(changes)
        return value

    def prepare(self, authority=None):
        authority = authority or self.authority()
        with patch.object(restore_build, "source_snapshot", return_value=SOURCE):
            backend_build = restore_build.build(self.backend, self.cargo_run)
        build_path = self.write_json(self.root / "backend-build.json", backend_build)
        frontend_build = {
            "format_version": 2,
            "kind": "restore-frontend-build",
            "sources": source_inventory.build_source_domains(INVENTORY, "frontend"),
            "build": {
                "command": ["vite", "build"], "mode": "production", "target": "vite-default",
                "toolchain": {"node": "v24.0.0",
                              "pnpm": {"pinned": "11.20.0", "observed": "11.20.0"},
                              "vite": "7.1.7"},
                "environment": {"variables": [],
                                "sha256": source_inventory.canonical_digest([])},
                "environment_files": [],
            },
            "files": restore_runtime.frontend_files(self.frontend),
        }
        frontend_path = self.write_json(
            self.frontend / "dist" / restore_runtime.FRONTEND_RECEIPT,
            frontend_build,
        )
        record = {
            "status": "data_verified",
            "data_verified_at": authority["data_verified_at"],
            "plan_hash": authority["plan_hash"],
            "plan": {
                "id": authority["restore_id"],
                "backup_id": authority["backup_id"],
                "scope_id": authority["scope_id"],
                "frontend_sha": authority["frontend_sha"],
                "api_ready_url": authority["api_endpoint"],
                "worker_ready_url": authority["worker_endpoint"],
            },
        }
        bindings_path = self.write_json(
            self.root / "bindings.json",
            {"record": record, "manifest": {"id": authority["backup_id"], "source_sha": authority["backend_sha"]}},
        )
        for index, (role, artifact) in enumerate(backend_build["artifacts"].items()):
            identity = {
                "pid": index + 42,
                "started": f"created-{role}",
                "executable": artifact["executable"],
            }
            self.identities[identity["pid"]] = identity
            self.write_json(
                self.root / f"{role}.json",
                {"format_version": 1, "role": role, "scope_id": authority["scope_id"], "identity": identity},
            )
        return authority, build_path, frontend_path, bindings_path

    @contextlib.contextmanager
    def runtime_patches(self, *, fake_probes):
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(restore_build, "source_snapshot", return_value=SOURCE))
            stack.enter_context(
                patch.object(
                    restore_runtime,
                    "process_identity",
                    side_effect=lambda pid: copy.deepcopy(self.identities.get(pid)),
                )
            )
            stack.enter_context(patch.object(restore_runtime, "verify_listener"))
            if fake_probes:
                stack.enter_context(patch.object(restore_runtime, "_probe_api", return_value=READY))
                stack.enter_context(patch.object(restore_runtime, "_probe_worker"))

                def local_frontend(root, receipt, _url):
                    files, snapshots = restore_runtime._frontend_snapshots(root)
                    if files != receipt["files"]:
                        raise ValueError("前端生产构建与文件收据不一致")
                    return files, snapshots

                stack.enter_context(
                    patch.object(
                        restore_runtime,
                        "_verify_frontend_artifacts",
                        side_effect=local_frontend,
                    )
                )
            yield

    def bind_receipt(self, authority=None, *, fake_probes=True):
        authority, build_path, _frontend_path, bindings_path = self.prepare(authority)
        with self.runtime_patches(fake_probes=fake_probes):
            receipt = restore_runtime.bind(
                self.backend,
                self.frontend,
                build_path,
                self.root,
                bindings_path,
                authority["frontend_endpoint"],
            )
        return authority, receipt, bindings_path

    def start_server(self, handler):
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(lambda: (server.shutdown(), thread.join(timeout=5)))
        return server

    def test_json_documents_are_regular_bounded_utf8_strict_and_stable(self):
        document = self.root / "document.json"
        document.write_text('{"value": 1}', encoding="utf-8")
        observed = restore_runtime.read_json_document(document)
        self.assertEqual(observed.value, {"value": 1})
        self.assertEqual(observed.sha256, restore_build.file_digest(document)["sha256"])
        invalid = {
            "empty": b"",
            "utf8": b'{"value":"\xff"}',
            "duplicate": b'{"value":1,"value":2}',
            "constant": b'{"value":NaN}',
            "array": b"[]",
        }
        for name, raw in invalid.items():
            with self.subTest(name=name):
                path = self.root / f"{name}.json"
                path.write_bytes(raw)
                with self.assertRaises(ValueError):
                    restore_runtime.read_json_document(path)
        oversized = self.root / "oversized.json"
        with oversized.open("wb") as stream:
            stream.truncate(restore_runtime_evidence.MAX_JSON_BYTES + 1)
        with self.assertRaisesRegex(ValueError, "超过 16 MiB"):
            restore_runtime.read_json_document(oversized)
        with self.assertRaisesRegex(ValueError, "普通文件"):
            restore_runtime.read_json_document(self.root)
        document.write_text('{"value": 2}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "被替换或修改"):
            observed.assert_unchanged()

    def test_json_documents_reject_links_and_reparse_points(self):
        document = self.root / "document.json"
        document.write_text("{}", encoding="utf-8")
        with patch.object(restore_runtime_evidence, "is_reparse", return_value=True), self.assertRaisesRegex(
            ValueError, "重解析点"
        ):
            restore_runtime.read_json_document(document)
        link = self.root / "link.json"
        try:
            link.symlink_to(document)
        except OSError:
            return
        with self.assertRaisesRegex(ValueError, "符号链接"):
            restore_runtime.read_json_document(link)

    def test_authoritative_context_has_exact_fields_and_strict_endpoints(self):
        expected = self.authority()
        self.assertEqual(restore_runtime.read_authority(io.StringIO(json.dumps(expected))), expected)
        valid = [
            {**expected, "restore_id": "a" + "_" * 63},
            {**expected, "backup_id": "b" + "-" * 63},
            {**expected, "scope_id": "a1"},
            {**expected, "scope_id": "a" + "_" * 46 + "z"},
        ]
        for value in valid:
            with self.subTest(valid=value):
                self.assertEqual(restore_runtime.read_authority(io.StringIO(json.dumps(value))), value)
        invalid = [
            {**expected, "extra": True},
            {**expected, "restore_id": "_restore"},
            {**expected, "restore_id": "Restore"},
            {**expected, "restore_id": "restore.one"},
            {**expected, "restore_id": "a" * 65},
            {**expected, "backup_id": "-backup"},
            {**expected, "scope_id": "a"},
            {**expected, "scope_id": "_scope"},
            {**expected, "scope_id": "scope_"},
            {**expected, "scope_id": "Scope"},
            {**expected, "scope_id": "scope.one"},
            {**expected, "scope_id": "a" + "_" * 47 + "z"},
            {**expected, "data_verified_at": "2026-09-06T08:00:00"},
            {**expected, "api_endpoint": "http://localhost:18080/readyz"},
            {**expected, "api_endpoint": "http://user@127.0.0.1:18080/readyz"},
            {**expected, "api_endpoint": "http://127.0.0.1:18080/readyz?ok=1"},
            {**expected, "api_endpoint": "http://127.0.0.1:18080/"},
            {**expected, "api_endpoint": "http://127.0.0.1/readyz"},
            {**expected, "worker_endpoint": expected["api_endpoint"]},
            {**expected, "frontend_endpoint": "http://127.0.0.1:14174/"},
        ]
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(ValueError):
                restore_runtime.read_authority(io.StringIO(json.dumps(value)))
        with self.assertRaisesRegex(ValueError, "重复字段"):
            restore_runtime.read_authority(io.StringIO('{"format_version":1,"format_version":1}'))

    def test_bindings_use_the_same_identifier_boundaries_as_authority(self):
        authority, _build, _frontend, bindings_path = self.prepare()
        bindings = restore_runtime.read_json(bindings_path)
        valid = [
            ("id", "a" + "_" * 63),
            ("backup_id", "b" + "-" * 63),
            ("scope_id", "a" + "_" * 46 + "z"),
        ]
        for field, value in valid:
            candidate = copy.deepcopy(bindings)
            candidate["record"]["plan"][field] = value
            if field == "backup_id":
                candidate["manifest"]["id"] = value
            with self.subTest(valid_field=field):
                restore_runtime.require_bindings(candidate)
        invalid = [
            ("id", "_restore"),
            ("backup_id", "backup.one"),
            ("scope_id", "a"),
            ("scope_id", "scope-"),
            ("scope_id", "a" * 49),
        ]
        for field, value in invalid:
            candidate = copy.deepcopy(bindings)
            candidate["record"]["plan"][field] = value
            if field == "backup_id":
                candidate["manifest"]["id"] = value
            with self.subTest(invalid_field=field), self.assertRaises(ValueError):
                restore_runtime.require_bindings(candidate)

    def test_bind_records_reviewable_paths_and_every_authoritative_field(self):
        authority, receipt, bindings = self.bind_receipt()
        self.assertEqual(receipt["paths"]["bindings"], str(bindings))
        self.assertEqual(receipt["restore"]["data_verified_at"], authority["data_verified_at"])
        self.assertEqual(receipt["source"], {"backend_sha": authority["backend_sha"], "frontend_sha": authority["frontend_sha"]})
        self.assertEqual(
            receipt["endpoints"],
            {
                "api": authority["api_endpoint"],
                "worker": authority["worker_endpoint"],
                "frontend": authority["frontend_endpoint"],
            },
        )
        for role in ("api", "worker"):
            self.assertEqual(receipt["processes"][role]["receipt_path"], str(self.root / f"{role}.json"))

    def test_runtime_and_process_receipts_reject_extra_fields_before_probes(self):
        authority, receipt, bindings = self.bind_receipt()
        changed = {**receipt, "extra": True}
        with patch.object(restore_runtime, "_probe_api") as probe, self.assertRaisesRegex(ValueError, "字段必须精确"):
            restore_runtime.verify(changed, self.backend, self.frontend, bindings, authority, "f" * 64)
        probe.assert_not_called()
        process_path = self.root / "api.json"
        process = restore_runtime.read_json(process_path)
        self.write_json(process_path, {**process, "extra": True})
        with self.runtime_patches(fake_probes=True), self.assertRaises(ValueError):
            restore_runtime.verify(receipt, self.backend, self.frontend, bindings, authority, "f" * 64)

    def test_tampered_bindings_build_frontend_process_and_dist_are_rejected(self):
        authority, receipt, bindings = self.bind_receipt()
        paths = [
            bindings,
            Path(receipt["paths"]["backend_build"]),
            Path(receipt["paths"]["frontend_build"]),
            Path(receipt["processes"]["api"]["receipt_path"]),
            self.frontend / "dist/app.js",
            Path(receipt["backend"]["artifacts"]["worker"]["executable"]),
        ]
        for path in paths:
            with self.subTest(path=path):
                original = path.read_bytes()
                path.write_bytes(original + b"tampered")
                with self.runtime_patches(fake_probes=True), self.assertRaises(ValueError):
                    restore_runtime.verify(receipt, self.backend, self.frontend, bindings, authority, "f" * 64)
                path.write_bytes(original)

    def test_full_verification_checks_api_worker_and_all_frontend_bytes(self):
        api = self.start_server(
            response_handler({"/readyz": (200, "application/json", json.dumps(READY).encode(), {})})
        )
        worker = self.start_server(response_handler({"/readyz": (200, "text/plain", b"", {})}))
        frontend = self.start_server(functools.partial(QuietHandler, directory=str(self.frontend / "dist")))
        authority = self.authority(
            api_endpoint=f"http://127.0.0.1:{api.server_port}/readyz",
            worker_endpoint=f"http://127.0.0.1:{worker.server_port}/readyz",
            frontend_endpoint=f"http://127.0.0.1:{frontend.server_port}",
        )
        authority, build_path, _frontend_path, bindings = self.prepare(authority)
        with self.runtime_patches(fake_probes=False):
            receipt = restore_runtime.bind(
                self.backend,
                self.frontend,
                build_path,
                self.root,
                bindings,
                authority["frontend_endpoint"],
            )
            result = restore_runtime.verify(receipt, self.backend, self.frontend, bindings, authority, "f" * 64)
        self.assertEqual(result["status"], "verified")
        self.assertEqual(result["runtime_receipt_sha256"], "f" * 64)
        self.assertEqual(result["api_readiness"], READY)
        self.assertEqual(set(result["processes"]), {"api", "worker"})
        self.assertEqual({item["path"] for item in result["frontend"]["files"]}, {"index.html", "app.js", ".vite/manifest.json"})

    def test_arbitrary_200_and_invalid_readiness_payloads_are_rejected(self):
        payloads = [
            ("text/plain", b"ok"),
            ("application/json", b"{}"),
            ("application/json", json.dumps({**READY, "extra": True}).encode()),
            ("application/json", json.dumps({**READY, "mysql": "down"}).encode()),
        ]
        for content_type, body in payloads:
            with self.subTest(content_type=content_type, body=body):
                server = self.start_server(response_handler({"/readyz": (200, content_type, body, {})}))
                with self.assertRaises(ValueError):
                    restore_runtime._probe_api(f"http://127.0.0.1:{server.server_port}/readyz")
        server = self.start_server(response_handler({"/readyz": (200, "text/plain", b"unexpected", {})}))
        with self.assertRaises(ValueError):
            restore_runtime._probe_worker(f"http://127.0.0.1:{server.server_port}/readyz")
        redirect = self.start_server(
            response_handler(
                {
                    "/readyz": (
                        302,
                        "text/plain",
                        b"",
                        {"Location": "http://127.0.0.1:1/readyz"},
                    )
                }
            )
        )
        with self.assertRaisesRegex(ValueError, "重定向"):
            restore_runtime._probe_worker(f"http://127.0.0.1:{redirect.server_port}/readyz")

    def test_process_receipt_change_during_probe_is_detected(self):
        authority, receipt, bindings = self.bind_receipt()
        process_path = self.root / "api.json"

        def mutate(_url):
            process_path.write_bytes(process_path.read_bytes() + b" ")
            return READY

        with self.runtime_patches(fake_probes=True), patch.object(restore_runtime, "_probe_api", side_effect=mutate), \
                self.assertRaisesRegex(ValueError, "被替换或修改"):
            restore_runtime.verify(receipt, self.backend, self.frontend, bindings, authority, "f" * 64)

    def test_source_or_process_identity_change_after_probe_is_detected(self):
        authority, receipt, bindings = self.bind_receipt()
        with self.runtime_patches(fake_probes=True), patch.object(
            restore_runtime,
            "process_identity",
            side_effect=[
                self.identities[42],
                self.identities[43],
                {**self.identities[42], "started": "restarted"},
            ],
        ), self.assertRaisesRegex(ValueError, "进程已退出、重启"):
            restore_runtime.verify(receipt, self.backend, self.frontend, bindings, authority, "f" * 64)

    def test_cli_requires_explicit_write_before_build_or_bind(self):
        common = ["--backend-dir", str(self.backend), "--output", str(self.backend / ".local-tests/out.json")]
        binding = [
            "--frontend-dir",
            str(self.frontend),
            "--bindings",
            str(self.root / "bindings.json"),
            "--frontend-url",
            "http://127.0.0.1:4174",
            "--build-receipt",
            str(self.root / "build.json"),
            "--runtime-dir",
            str(self.root),
        ]
        for operation, arguments in (("build", common), ("bind", common + binding)):
            with self.subTest(operation=operation), patch.object(
                sys, "argv", ["restore_runtime.py", operation, *arguments]
            ), patch.object(sys, "stderr", io.StringIO()), patch.object(restore_runtime, "build") as build_call, \
                    patch.object(restore_runtime, "bind") as bind_call, patch.object(restore_runtime, "write_new") as write:
                with self.assertRaises(SystemExit) as error:
                    restore_runtime.main()
                self.assertEqual(error.exception.code, 2)
                build_call.assert_not_called()
                bind_call.assert_not_called()
                write.assert_not_called()

    def test_verify_cli_reads_authority_from_stdin_and_outputs_only_detailed_result(self):
        runtime_path = self.write_json(self.root / "runtime.json", {"fixture": True})
        bindings = self.root / "bindings.json"
        authority = self.authority()
        result = {
            "format_version": 1,
            "kind": "restore-runtime-verification",
            "status": "verified",
            "runtime_receipt_sha256": restore_build.file_digest(runtime_path)["sha256"],
        }
        arguments = [
            "restore_runtime.py",
            "verify",
            "--backend-dir",
            str(self.backend),
            "--frontend-dir",
            str(self.frontend),
            "--bindings",
            str(bindings),
            "--receipt",
            str(runtime_path),
        ]
        with patch.object(sys, "argv", arguments), patch.object(sys, "stdin", io.StringIO(json.dumps(authority))), \
                patch.object(sys, "stdout", io.StringIO()) as output, patch.object(
                    restore_runtime, "verify", return_value=result
                ) as verify_call:
            restore_runtime.main()
        self.assertEqual(json.loads(output.getvalue()), result)
        self.assertEqual(verify_call.call_args.args[4], authority)

        def mutate(*_args):
            runtime_path.write_text('{"fixture":"changed"}', encoding="utf-8")
            return result

        self.write_json(runtime_path, {"fixture": True})
        with patch.object(sys, "argv", arguments), patch.object(sys, "stdin", io.StringIO(json.dumps(authority))), \
                patch.object(sys, "stdout", io.StringIO()), patch.object(restore_runtime, "verify", side_effect=mutate), \
                self.assertRaisesRegex(ValueError, "被替换或修改"):
            restore_runtime.main()


if __name__ == "__main__":
    unittest.main()
