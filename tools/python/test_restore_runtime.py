import contextlib
import copy
import io
import json
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_build
import restore_frontend_build
import restore_runtime
import restore_runtime_launch
import restore_runtime_evidence
import source_inventory
from artifact_digests import filesystem_path
from workspace_directory import WorkspaceDirectory
from frontend_receipt_fixture import prepare_frontend, frontend_receipt as frontend_receipt_value

ROOT = Path(__file__).resolve().parents[2]
SOURCE = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": [], "clean": True}
INVENTORY = {
    "source": {"snapshot": SOURCE, "worktree_fingerprint": "sha256:" + "c" * 64},
    "files": [],
    "guard": {"head": SOURCE["head"], "index_sha256": "d" * 64, "modes_sha256": "e" * 64},
}
READY = {"status": "ready", "mysql": "up", "redis": "optional_degraded", "object_storage": "not_required"}


def private_request(operation: str, **values) -> restore_runtime.RuntimeRequest:
    protocol = {
        "backend_dir": str(values.pop("backend_dir")),
        "format_version": 1,
        "kind": restore_runtime.PROTOCOL_KIND,
        "operation": operation,
        "write": operation in restore_runtime.WRITING_OPERATIONS,
        **values,
    }
    return restore_runtime.decode_request(json.dumps(protocol))


class RestoreRuntimeTests(unittest.TestCase):
    def test_native_probe_requires_bound_coordinator_without_compilation_fallback(self):
        with patch.object(restore_runtime, "tool_binding", return_value=None), patch.object(
            restore_runtime, "native_invoke"
        ) as invoke, self.assertRaisesRegex(ValueError, "工具绑定"):
            restore_runtime._probe_api("http://127.0.0.1:1/readyz")
        invoke.assert_not_called()

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
                side_effect=lambda root: (source_inventory.capture_inventory(root)
                                          if Path(root) == self.frontend else copy.deepcopy(INVENTORY)),
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
        (self.frontend / "dist/app.js").write_bytes(b"\x00RyFrame fixture\xff")
        (self.frontend / "dist/.vite/manifest.json").write_text("{}", encoding="utf-8")
        self.frontend_head = prepare_frontend(self.frontend)
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
            "format_version": 2,
            "kind": "restore-runtime-authority",
            "restore_id": "restore-one",
            "backup_id": "backup-one",
            "plan_hash": "c" * 64,
            "scope_id": "restore-test",
            "data_verified_at": "2026-09-06T08:00:00+08:00",
            "backup_source_sha": "f" * 40,
            "backend_product_sha": SOURCE["head"],
            "backend_execution_sha": SOURCE["head"],
            "backend_adapter_contract": None,
            "frontend_sha": self.frontend_head,
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
        frontend_build = frontend_receipt_value(self.frontend)
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
            {
                "record": record,
                "manifest": {
                    "id": authority["backup_id"],
                    "source_sha": authority["backup_source_sha"],
                },
            },
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
        frontend_identity = {
            "pid": 44,
            "started": "created-frontend",
            "executable": str(Path(sys.executable).resolve()),
        }
        self.identities[frontend_identity["pid"]] = frontend_identity
        self.write_json(
            self.root / "frontend.json",
            {
                "format_version": 1,
                "role": "frontend",
                "scope_id": authority["scope_id"],
                "identity": frontend_identity,
            },
        )
        self.launch_path = self.root / "runtime-launch.json"
        self.write_json(
            self.launch_path,
            {
                "format_version": 1,
                "kind": "restore-runtime-launch",
                "runtime_directory": str(self.root),
                "request": {
                    "authority": authority,
                    "roots": {
                        "backend_product": str(self.backend),
                        "backend_execution": str(self.backend),
                        "frontend": str(self.frontend),
                    },
                    "paths": {
                        "bindings": str(bindings_path),
                        "backend_build": str(build_path),
                        "frontend_build": str(frontend_path),
                    },
                    "digests": {
                        "bindings": restore_build.file_digest(bindings_path)["sha256"],
                        "backend_build": restore_build.file_digest(build_path)["sha256"],
                        "frontend_build": restore_build.file_digest(frontend_path)["sha256"],
                    },
                },
                "processes": {
                    role: {
                        "command": [
                            backend_build["artifacts"][role]["executable"]
                            if role != "frontend"
                            else frontend_identity["executable"]
                        ]
                    }
                    for role in ("api", "worker", "frontend")
                },
            },
        )
        return authority, build_path, frontend_path, bindings_path

    def resolved_sources(self, authority):
        return {
            "roots": {
                "backend_product": str(self.backend),
                "backend_execution": str(self.backend),
                "frontend": str(self.frontend),
            },
            "source": {
                "backend_product_sha": authority["backend_product_sha"],
                "backend_execution_sha": authority["backend_execution_sha"],
                "backend_adapter_contract": authority["backend_adapter_contract"],
                "frontend_sha": authority["frontend_sha"],
            },
        }

    @contextlib.contextmanager
    def runtime_patches(self, *, fake_probes):
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(restore_runtime_launch, "verify_configuration"))
            stack.enter_context(patch.object(restore_build, "source_snapshot", return_value=SOURCE))
            stack.enter_context(
                patch.object(
                    restore_runtime,
                    "process_identity",
                    side_effect=lambda pid: copy.deepcopy(self.identities.get(pid)),
                )
            )
            stack.enter_context(patch.object(restore_runtime, "verify_listener"))
            stack.enter_context(
                patch.object(restore_runtime, "validate_launch", side_effect=lambda value, _path: value)
            )
            stack.enter_context(
                patch.object(
                    restore_runtime,
                    "resolve_runtime_sources",
                    side_effect=lambda *_args, **_kwargs: self.resolved_sources(self.authority()),
                )
            )
            if fake_probes:
                stack.enter_context(patch.object(restore_runtime, "_probe_api", return_value=READY))
                stack.enter_context(patch.object(restore_runtime, "_probe_worker"))

                def local_frontend(root, receipt, _url):
                    files, snapshots = restore_frontend_build.frontend_snapshots(root)
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
                self.backend,
                self.frontend,
                build_path,
                self.launch_path,
                bindings_path,
            )
        return authority, receipt, bindings_path

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
        self.addCleanup(link.unlink)
        with self.assertRaisesRegex(ValueError, "符号链接"):
            restore_runtime.read_json_document(link)

    def test_json_documents_support_paths_beyond_max_path(self):
        deep = self.root
        while len(str(deep)) < 250:
            deep = deep / ("d" * 40)
        os.makedirs(filesystem_path(deep), exist_ok=True)
        self.addCleanup(lambda: shutil.rmtree(filesystem_path(deep), ignore_errors=True))
        document = deep / "document.json"
        with open(filesystem_path(document), "wb") as stream:
            stream.write(b'{"value": 7}')
        self.assertGreater(len(str(document)), 260)
        observed = restore_runtime.read_json_document(document)
        self.assertEqual(observed.value, {"value": 7})
        self.assertEqual(restore_runtime_evidence.artifact_snapshot(document).bytes, 12)

    def test_authoritative_context_has_exact_fields_and_strict_endpoints(self):
        expected = self.authority()
        self.assertEqual(restore_runtime.read_authority(io.StringIO(json.dumps(expected))), expected)
        valid = [
            {**expected, "restore_id": "a" + "_" * 63},
            {**expected, "backup_id": "b" + "-" * 63},
            {**expected, "scope_id": "a1"},
            {**expected, "scope_id": "a" + "_" * 46 + "z"},
            {
                **expected,
                "backend_execution_sha": "e" * 40,
                "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
            },
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
            {**expected, "backend_execution_sha": "e" * 40},
            {**expected, "backend_adapter_contract": "legacy-stable-readiness-b0-v1"},
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
        self.assertEqual(receipt["paths"]["launch"], str(self.launch_path))
        self.assertEqual(receipt["format_version"], 3)
        self.assertEqual(receipt["restore"]["data_verified_at"], authority["data_verified_at"])
        self.assertEqual(
            receipt["source"],
            {
                "backup_source_sha": authority["backup_source_sha"],
                "backend_product_sha": authority["backend_product_sha"],
                "backend_execution_sha": authority["backend_execution_sha"],
                "backend_adapter_contract": authority["backend_adapter_contract"],
                "frontend_sha": authority["frontend_sha"],
            },
        )
        self.assertEqual(
            receipt["endpoints"],
            {
                "api": authority["api_endpoint"],
                "worker": authority["worker_endpoint"],
                "frontend": authority["frontend_endpoint"],
            },
        )
        for role in ("api", "worker", "frontend"):
            self.assertEqual(receipt["processes"][role]["receipt_path"], str(self.root / f"{role}.json"))

    def test_runtime_and_process_receipts_reject_extra_fields_before_probes(self):
        authority, receipt, bindings = self.bind_receipt()
        changed = {**receipt, "extra": True}
        with patch.object(restore_runtime, "_probe_api") as probe, self.assertRaisesRegex(ValueError, "字段必须精确"):
            restore_runtime.verify(changed, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64)
        probe.assert_not_called()
        process_path = self.root / "api.json"
        process = restore_runtime.read_json(process_path)
        self.write_json(process_path, {**process, "extra": True})
        with self.runtime_patches(fake_probes=True), self.assertRaises(ValueError):
            restore_runtime.verify(receipt, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64)

    def test_tampered_bindings_build_frontend_process_and_dist_are_rejected(self):
        authority, receipt, bindings = self.bind_receipt()
        paths = [
            bindings,
            Path(receipt["paths"]["backend_build"]),
            Path(receipt["paths"]["frontend_build"]),
            Path(receipt["processes"]["api"]["receipt_path"]),
            Path(receipt["processes"]["frontend"]["receipt_path"]),
            Path(receipt["paths"]["launch"]),
            self.frontend / "dist/app.js",
            Path(receipt["backend"]["artifacts"]["worker"]["executable"]),
        ]
        for path in paths:
            with self.subTest(path=path):
                original = path.read_bytes()
                path.write_bytes(original + b"tampered")
                with self.runtime_patches(fake_probes=True), self.assertRaises(ValueError):
                    restore_runtime.verify(receipt, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64)
                path.write_bytes(original)

    def test_full_verification_binds_native_probe_results_to_the_runtime_receipt(self):
        # 真实 HTTP 与全部公开产物字节由 Rust runtime::probes 测试覆盖；
        # 此处仍覆盖尚由 Python 执行的绑定、编排及最终结果投影。
        authority, build_path, _frontend_path, bindings = self.prepare()

        def native_observation(command, payload, _runner, *, may_write):
            self.assertEqual(command, "__source-evidence")
            self.assertEqual(set(payload), {"operation", "write", "arguments"})
            self.assertEqual(payload["operation"], "runtime-probe")
            self.assertIs(payload["write"], False)
            self.assertIs(may_write, False)
            value = READY if payload["arguments"]["role"] == "api" else {"status": "verified"}
            return subprocess.CompletedProcess([], 0), {"ok": True, "result": {"value": value}}

        with self.runtime_patches(fake_probes=False), patch.object(
            restore_runtime, "tool_binding", return_value={"bound": True}
        ), patch.object(restore_runtime, "native_invoke", side_effect=native_observation) as native:
            receipt = restore_runtime.bind(
                self.backend,
                self.backend,
                self.frontend,
                build_path,
                self.launch_path,
                bindings,
            )
            result = restore_runtime.verify(receipt, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64)
        self.assertEqual([call.args[1]["arguments"] for call in native.call_args_list], [
            {"role": "api", "url": authority["api_endpoint"]},
            {"role": "worker", "url": authority["worker_endpoint"]},
            {"role": "frontend", "url": authority["frontend_endpoint"], "files": receipt["frontend"]["files"]},
        ] * 2)
        self.assertEqual(result["status"], "verified")
        self.assertEqual(result["runtime_receipt_sha256"], "f" * 64)
        self.assertEqual(result["api_readiness"], READY)
        self.assertEqual(set(result["processes"]), {"api", "worker", "frontend"})
        self.assertEqual({item["path"] for item in result["frontend"]["files"]},
                         {"index.html", "app.js", ".vite/manifest.json", "build-identity.json"})

    def test_process_receipt_change_during_probe_is_detected(self):
        authority, receipt, bindings = self.bind_receipt()
        process_path = self.root / "api.json"

        def mutate(_url):
            process_path.write_bytes(process_path.read_bytes() + b" ")
            return READY

        with self.runtime_patches(fake_probes=True), patch.object(restore_runtime, "_probe_api", side_effect=mutate), \
                self.assertRaisesRegex(ValueError, "被替换或修改"):
            restore_runtime.verify(receipt, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64)

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
            restore_runtime.verify(receipt, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64)

    def test_configuration_is_rechecked_before_probes_and_after_verification(self):
        authority, receipt, bindings = self.bind_receipt()
        for outcome, calls in ((ValueError("配置已漂移"), 1),
                               ([None, ValueError("核验期间配置漂移")], 2)):
            with self.runtime_patches(fake_probes=True), patch.object(
                restore_runtime_launch, "verify_configuration", side_effect=outcome,
            ) as verify_config, patch.object(restore_runtime, "_probe_api", return_value=READY) as probe:
                with self.assertRaisesRegex(ValueError, "配置.*漂移"):
                    restore_runtime.verify(
                        receipt, self.backend, self.backend, self.frontend, bindings, authority, "f" * 64,
                    )
                self.assertEqual(verify_config.call_count, calls)
                self.assertEqual(probe.call_count, calls - 1)

    def test_private_protocol_requires_exact_write_and_rejects_direct_argv(self):
        common = {
            "backend_dir": str(self.backend),
            "output": str(self.backend / ".local-tests/out.json"),
            "source_backend": str(self.backend),
            "source_frontend": str(self.frontend),
        }
        cases = {
            "build": {
                **common,
                "frontend_dir": str(self.frontend),
                "expected_head": SOURCE["head"],
                "expected_frontend_head": self.frontend_head,
            },
            "bind": {
                **common,
                "bindings": str(self.root / "bindings.json"),
                "build_receipt": str(self.root / "build.json"),
                "launch_receipt": str(self.root / "runtime-launch.json"),
            },
        }
        for operation, values in cases.items():
            protocol = {
                "format_version": 1,
                "kind": restore_runtime.PROTOCOL_KIND,
                "operation": operation,
                "write": False,
                **values,
            }
            with self.subTest(operation=operation), patch.object(
                restore_runtime, "build_registered"
            ) as build_call, \
                    patch.object(restore_runtime, "bind") as bind_call, patch.object(restore_runtime, "write_new") as write:
                with self.assertRaises(restore_runtime.RuntimeProtocolError):
                    restore_runtime.decode_request(json.dumps(protocol))
                build_call.assert_not_called()
                bind_call.assert_not_called()
                write.assert_not_called()
        with self.assertRaises(restore_runtime.RuntimeProtocolError):
            restore_runtime.private_protocol_request(["build"])

    def test_bind_request_delegates_receipt_write_to_the_locked_operation(self):
        output = self.backend / ".local-tests/runtime.json"
        output.parent.mkdir()
        request = private_request(
            "bind", backend_dir=self.backend, source_backend=str(self.backend),
            source_frontend=str(self.frontend), bindings=str(self.root / "bindings.json"),
            build_receipt=str(self.root / "build.json"),
            launch_receipt=str(self.root / "generation-0001/runtime-launch.json"),
            output=str(output),
        )
        with patch.object(sys, "stdout", io.StringIO()) as stdout, patch.object(
            restore_runtime, "bind_and_write"
        ) as bind_write, patch.object(
            restore_runtime, "write_new"
        ) as raw_write:
            restore_runtime.main(request)
        bind_write.assert_called_once_with(
            self.backend.resolve(),
            self.backend,
            self.frontend,
            (self.root / "build.json").resolve(),
            (self.root / "generation-0001/runtime-launch.json").resolve(),
            (self.root / "bindings.json").resolve(),
            output,
            adapter_contract=None,
            product_backend=None,
        )
        raw_write.assert_not_called()
        self.assertEqual(json.loads(stdout.getvalue()), {"output": str(output.resolve())})

    def test_build_request_separates_coordinator_from_source_worktree(self):
        output = self.backend / ".local-tests/build.json"
        receipt = {"format_version": 2, "kind": "restore-backend-build"}
        frontend_receipt = restore_runtime.read_json_document(
            self.write_json(
                self.frontend / "dist" / restore_runtime.FRONTEND_RECEIPT,
                frontend_receipt_value(self.frontend),
            )
        )
        request = private_request(
            "build", backend_dir=self.backend, source_backend=str(self.root),
            expected_head=SOURCE["head"], frontend_dir=str(self.frontend),
            source_frontend=str(self.frontend), expected_frontend_head=self.frontend_head,
            output=str(output),
        )
        with patch.object(sys, "stdout", io.StringIO()) as stdout, \
                patch.object(restore_runtime, "repository", return_value=self.root), patch.object(
            restore_runtime,
            "build_registered",
            return_value=(self.root, receipt),
        ) as build_call, patch.object(
            restore_runtime,
            "build_registered_frontend",
            return_value=(self.frontend, frontend_receipt, "verified"),
        ) as frontend_build, patch.object(
            restore_runtime, "validate_new_output", return_value=output
        ) as validate, patch.object(restore_runtime, "write_new") as write:
            restore_runtime.main(request)
        build_call.assert_called_once_with(
            self.backend.resolve(),
            self.root,
            SOURCE["head"],
            adapter_contract=None,
            product_backend=None,
        )
        frontend_build.assert_called_once_with(
            self.frontend.resolve(), self.frontend, self.frontend_head
        )
        validate.assert_called_once_with(output, self.root)
        write.assert_called_once_with(output, receipt, self.root)
        self.assertEqual(json.loads(stdout.getvalue())["frontend_action"], "verified")

    def test_registered_frontend_reuses_only_an_exact_existing_receipt(self):
        self.prepare()
        run = unittest.mock.Mock()
        with patch.object(
            restore_frontend_build,
            "repository",
            side_effect=lambda path, _label: Path(path).resolve(),
        ):
            source, receipt, action = restore_frontend_build.build_registered_frontend(
                self.frontend, self.frontend, self.frontend_head, run
            )
        self.assertEqual(source, self.frontend)
        self.assertEqual(receipt.value["kind"], "restore-frontend-build")
        self.assertEqual(action, "verified")
        run.assert_not_called()

    def test_registered_frontend_builds_with_the_target_vite_when_receipt_is_absent(self):
        _authority, _backend, frontend_path, _bindings = self.prepare()
        value = json.loads(frontend_path.read_text(encoding="utf-8"))
        frontend_path.unlink()

        def run(command, **options):
            self.assertEqual(command[0], "node")
            self.assertEqual(command[2:4], ["external-build", "--source-root"])
            self.assertEqual(command[-3:], ["--expected-head", self.frontend_head, "--write"])
            self.assertEqual(options["cwd"], self.frontend)
            self.assertTrue(options["check"])
            self.write_json(frontend_path, value)
            return subprocess.CompletedProcess(command, 0)

        with patch.object(
            restore_frontend_build,
            "repository",
            side_effect=lambda path, _label: Path(path).resolve(),
        ):
            source, receipt, action = restore_frontend_build.build_registered_frontend(
                self.frontend, self.frontend, self.frontend_head, run
            )
        self.assertEqual(source, self.frontend)
        self.assertEqual(receipt.path, frontend_path)
        self.assertEqual(action, "built")

    def test_verify_request_reads_stdin_authority_and_outputs_detailed_result(self):
        runtime_path = self.write_json(self.root / "runtime.json", {"fixture": True})
        bindings = self.root / "bindings.json"
        authority = self.authority()
        result = {
            "format_version": 1,
            "kind": "restore-runtime-verification",
            "status": "verified",
            "runtime_receipt_sha256": restore_build.file_digest(runtime_path)["sha256"],
        }
        request = private_request(
            "verify", backend_dir=self.backend, source_backend=str(self.backend),
            source_frontend=str(self.frontend), bindings=str(bindings), receipt=str(runtime_path),
        )
        with patch.object(sys, "stdin", io.StringIO(json.dumps(authority))), \
                patch.object(sys, "stdout", io.StringIO()) as output, patch.object(
                    restore_runtime, "verify", return_value=result
                ) as verify_call:
            restore_runtime.main(request)
        self.assertEqual(json.loads(output.getvalue()), result)
        self.assertEqual(verify_call.call_args.args[5], authority)

        def mutate(*_args, **_kwargs):
            runtime_path.write_text('{"fixture":"changed"}', encoding="utf-8")
            return result

        self.write_json(runtime_path, {"fixture": True})
        with patch.object(sys, "stdin", io.StringIO(json.dumps(authority))), \
                patch.object(sys, "stdout", io.StringIO()), patch.object(restore_runtime, "verify", side_effect=mutate), \
                self.assertRaisesRegex(ValueError, "被替换或修改"):
            restore_runtime.main(request)


if __name__ == "__main__":
    unittest.main()
