import contextlib
import copy
import json
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


class RestoreRuntimeTests(unittest.TestCase):
    def setUp(self):
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
        # 仅是编排夹具；完整生产来源与Cargo行为由Rust正式入口测试承担。
        context = restore_build.build_context(self.backend)
        artifacts = {}
        for role, name in (("api", "ryframe"), ("worker", "ryframe-worker")):
            executable = self.backend / name
            executable.write_bytes(name.encode())
            artifacts[role] = {"executable": str(executable), "command": context["commands"][role],
                               **restore_build.file_digest(executable)}
        backend_build = {"format_version": 2, "kind": "restore-backend-build",
                         "sources": source_inventory.build_source_domains(INVENTORY, "backend"),
                         "build": context, "artifacts": artifacts}
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
        frontend_program = self.root / "registered-frontend.exe"
        frontend_program.write_bytes(b"fixture native frontend")
        frontend_identity = {
            "pid": 44,
            "started": "created-frontend",
            "executable": str(frontend_program),
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


if __name__ == "__main__":
    unittest.main()
