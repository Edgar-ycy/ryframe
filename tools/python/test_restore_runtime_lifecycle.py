import contextlib
import json
import os
import shutil
import subprocess
import sys
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import restore_runtime_state as generation_model
import restore_runtime_evidence as evidence_model
import restore_runtime_lifecycle as lifecycle
import restore_runtime_launch as launch_model
import restore_runtime_registration as registration_model
import runtime_control_lock
from full_stack_process import process_identity, write_receipt
from restore_runtime_evidence import artifact_snapshot, read_json_document
from restore_runtime_registration import REGISTRATION_LOCK
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


class RestoreRuntimeLifecycleTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=local)
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()
        self.runtime = self.base / "runtime"
        self.coordinator = ROOT
        self.binding = {
            "registration": self.descriptor(self.base / "registration.json"),
            "target_plan": self.descriptor(self.base / "target-plan.json"),
        }
        self.authority = {
            "format_version": 2,
            "kind": "restore-runtime-authority",
            "restore_id": "restore-one",
            "backup_id": "backup-one",
            "plan_hash": "1" * 64,
            "scope_id": "runtime-test",
            "data_verified_at": "2026-09-11T08:00:00+08:00",
            "backup_source_sha": "2" * 40,
            "backend_product_sha": "3" * 40,
            "backend_execution_sha": "3" * 40,
            "backend_adapter_contract": None,
            "frontend_sha": "4" * 40,
            "api_endpoint": "http://127.0.0.1:18180/readyz",
            "worker_endpoint": "http://127.0.0.1:19191/readyz",
            "frontend_endpoint": "http://127.0.0.1:15174",
        }
        executable = str(Path(sys.executable).resolve())
        server = self.base / "registered-frontend.exe"
        shutil.copyfile(executable, server)
        tool = artifact_snapshot(server).descriptor()
        self.native_tool = {"format_version": 1, "kind": "ryframe-native-rate-limit-tool", **tool}
        patcher = patch.object(launch_model, "tool_binding", return_value=self.native_tool)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.request = {
            "format_version": 2,
            "kind": "restore-runtime-launch-request",
            "runtime_directory": str(self.runtime),
            "authority": self.authority,
            "registration": self.binding,
            "roots": {
                "backend_product": str(self.base),
                "backend_execution": str(self.base),
                "frontend": str(self.base),
            },
            "paths": {
                "bindings": str(self.base / "bindings.json"),
                "backend_build": str(self.base / "backend-build.json"),
                "frontend_build": str(self.base / "frontend-build.json"),
            },
            "digests": {key: "5" * 64 for key in ("bindings", "backend_build", "frontend_build")},
            "artifacts": {
                role: {
                    "executable": executable,
                    "command": ["cargo", "build"],
                    "bytes": 1,
                    "sha256": "6" * 64,
                }
                for role in ("api", "worker")
            },
            "tools": {
                "coordinator_root": str(ROOT),
                "coordinator_head": "7" * 40,
                "inventory_sha256": "8" * 64,
                "frontend_server": tool,
            },
            "environment": {
                "document": {
                    "path": str(self.base / "environment.json"),
                    "bytes": 1,
                    "sha256": "a" * 64,
                },
                "configurations": {
                    role: {"directory": str(self.base / role), "files": {
                        "app.test.toml": {"bytes": 1, "sha256": "b" * 64},
                    }, "environment_sha256": "c" * 64} for role in ("api", "worker")
                },
            },
        }
        self.documents = tuple(Mock(assert_unchanged=Mock()) for _ in range(3))
        self.facts = {
            "runtime_directory": str(self.runtime),
            "endpoints": {
                "api": self.authority["api_endpoint"],
                "worker": self.authority["worker_endpoint"],
                "frontend": self.authority["frontend_endpoint"],
            },
        }

    @staticmethod
    def descriptor(path: Path) -> dict:
        path.write_text("{}", encoding="utf-8")
        document = read_json_document(path)
        return {"path": str(path), "bytes": len(document.raw), "sha256": document.sha256}

    def args(self, **changes):
        values = {
            "runtime_registration": Path(self.binding["registration"]["path"]),
            "target_plan": Path(self.binding["target_plan"]["path"]),
            "source_backend": self.base,
            "source_frontend": self.base,
            "build_receipt": self.base / "backend-build.json",
            "bindings": self.base / "bindings.json",
            "adapter_contract": None,
            "product_backend": None,
            "timeout": 5,
            "generation": 1,
        }
        values.update(changes)
        return SimpleNamespace(**values)

    def commands(self, seconds: float = 30) -> dict:
        return {
            role: [str(Path(sys.executable).resolve()), "-c", f"import time; time.sleep({seconds})"]
            for role in ("api", "worker", "frontend")
        }

    @contextlib.contextmanager
    def patched(self, command_map):
        binding_result = (self.runtime, self.binding, {}, self.facts, self.documents)
        with contextlib.ExitStack() as stack:
            stack.enter_context(
                patch.object(lifecycle, "prepare_request", return_value=(self.runtime, self.request))
            )
            stack.enter_context(patch.object(lifecycle, "_binding", return_value=binding_result))
            stack.enter_context(
                patch.object(
                    lifecycle,
                    "verify_registration",
                    return_value=(
                        {"observation": {"runtime": {"exists": False}}},
                        self.facts,
                        self.documents,
                    ),
                )
            )
            stack.enter_context(patch.object(generation_model, "commands", return_value=command_map))
            stack.enter_context(patch.object(lifecycle, "launch_environment", return_value=launch_model.configured({"APP_ENV": "test"})))
            stack.enter_context(patch.object(lifecycle, "_wait_ready"))
            stack.enter_context(patch.object(lifecycle, "verify_listener"))
            stack.enter_context(patch.object(lifecycle, "require_closed_port"))
            yield

    def stop_under_recover_lock(self):
        # 旧 Recover 仍以相同锁调用此内部回收实现。
        with runtime_control_lock.controller_lock(
            self.coordinator / ".local-tests",
            lifecycle._operation("recover", self.binding),
            REGISTRATION_LOCK,
        ):
            return lifecycle._stop_locked(self.args(), self.coordinator)

    def observed_generation(self):
        *_, state = lifecycle._load_locked(self.args(), self.coordinator)
        generation = state["generations"][-1]
        return generation, lifecycle._observe_generation(generation)

    def test_stop_after_natural_exit_retains_completion_evidence(self):
        with self.patched(self.commands(0.5)):
            lifecycle.start(self.args(), self.coordinator, Mock(), Mock(), Mock())
            time.sleep(1)
            _generation, processes = self.observed_generation()
            self.assertEqual({item["state"] for item in processes.values()}, {"stopped"})
            self.assertTrue(all(item["completion"] is not None for item in processes.values()))
            self.assertEqual(self.stop_under_recover_lock()["status"], "stopped")

    def test_generation_without_tool_binding_fails_before_process_or_file_write(self):
        with patch.object(generation_model, "tool_binding", return_value=None), patch.object(
            generation_model.subprocess, "run"
        ) as runner, self.assertRaisesRegex(ValueError, "缺少协调器工具绑定"):
            generation_model.create_intent(self.runtime, self.binding, self.request)
        runner.assert_not_called()
        self.assertFalse(generation_model.creation_path(self.runtime).exists())
        self.assertFalse(self.runtime.exists())

    def test_recover_completes_first_directory_crash_cut(self):
        with self.patched(self.commands()):
            generation_model.create_intent(self.runtime, self.binding, self.request)
            control = self.coordinator / ".local-tests"
            lock = control / REGISTRATION_LOCK.lock_name
            lock.mkdir()
            process = subprocess.Popen(
                [sys.executable, "-c", "import time; time.sleep(.2)"],
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
            identity = process_identity(process.pid)
            process.wait(timeout=5)
            owner = {
                "format_version": 1,
                "kind": REGISTRATION_LOCK.owner_kind,
                "identity": identity,
                "runtime_directory": str(control.resolve()),
                "operation": lifecycle._operation("start", self.binding),
                "token": "c" * 32,
            }
            owner_path = lock / "owner.json"
            write_receipt(owner_path, owner)

            def reconcile(*_args, **_kwargs):
                shutil.rmtree(lock)
                return {"kind": "test-reconciliation", "owner": owner}

            with patch.object(lifecycle, "reconcile_lock", side_effect=reconcile):
                result = lifecycle.recover(
                    self.args(owner=owner_path), self.coordinator
                )
            self.assertEqual(result["status"], "stopped")
            state = read_json_document(self.runtime / "lifecycle.json").value
            self.assertEqual(state["generations"][0]["status"], "stopped")
            self.assertTrue((self.runtime / "generation-0001").is_dir())

    def test_launch_request_keeps_product_execution_and_frontend_identities_distinct(self):
        self.assertEqual(launch_model.validate_request(self.request), self.request)
        invalid = json.loads(json.dumps(self.request))
        invalid["authority"]["backend_execution_sha"] = "d" * 40
        invalid["authority"]["backend_adapter_contract"] = launch_model.B0_BACKEND_ADAPTER
        with self.assertRaisesRegex(ValueError, "必须绑定精确历史产品、执行树和适配合同"):
            launch_model.validate_request(invalid)

        invalid["authority"].update(
            frontend_sha=launch_model.B0_HEAD,
            backend_product_sha="815c5eafb09d4b493319d255fb7ab88ddd02c8b6",
            backend_execution_sha="c05114bcdf5c369cd74087db6317ce3c8f89bee8",
        )
        with self.assertRaisesRegex(ValueError, "适配关系"):
            launch_model.validate_request(invalid)

        invalid["roots"]["backend_execution"] = str(self.base / "adapter")
        invalid["roots"]["frontend"] = str(self.base / "frontend")
        self.assertEqual(launch_model.validate_request(invalid), invalid)

    def test_lifecycle_documents_reject_boolean_format_versions(self):
        request = json.loads(json.dumps(self.request))
        request["format_version"] = True
        cases = (
            (launch_model.validate_request, request, ()),
            (
                generation_model.validate_creation,
                {
                    "format_version": True,
                    "kind": "restore-runtime-create-intent",
                    "runtime_directory": str(self.runtime),
                    "generation": 1,
                    "registration": self.binding,
                    "request": self.request,
                },
                (self.runtime, self.binding),
            ),
            (
                generation_model.validate_state,
                {
                    "format_version": True,
                    "kind": "restore-runtime-lifecycle",
                    "runtime_directory": str(self.runtime),
                    "registration": self.binding,
                    "creation_intent": {},
                    "generations": [{}],
                },
                (self.runtime, self.binding),
            ),
            (
                generation_model.validate_launch,
                {
                    "format_version": True,
                    "kind": "restore-runtime-launch",
                    "generation": 1,
                    "runtime_directory": str(self.runtime / "generation-0001"),
                    "request": self.request,
                    "processes": {},
                },
                (self.runtime / "generation-0001",),
            ),
            (
                registration_model.validate_registration,
                {
                    "format_version": True,
                    "kind": "restore-runtime-registration",
                    "reference_plan": {},
                    "target_plan": {},
                    "observation": {},
                    "remote_writes": 0,
                },
                (),
            ),
        )
        for validator, value, arguments in cases:
            with self.subTest(validator=validator.__name__), self.assertRaises(ValueError):
                validator(value, *arguments)
        creation = json.loads(json.dumps(cases[1][1]))
        creation["format_version"] = 1
        creation["generation"] = True
        with self.assertRaises(ValueError):
            generation_model.validate_creation(creation, self.runtime, self.binding)
        invalid_generation = {
            "number": True,
            "directory": str(self.runtime / "generation-0001"),
            "status": "starting",
            "request": self.request,
            "roles": {},
            "launch": None,
            "error_type": None,
        }
        with self.assertRaises(ValueError):
            generation_model.validate_generation(invalid_generation, self.runtime, 1, self.binding)
        registration = json.loads(json.dumps(cases[-1][1]))
        registration["format_version"] = 1
        registration["remote_writes"] = False
        with self.assertRaises(ValueError):
            registration_model.validate_registration(registration)
        process = self.base / "process.json"
        write_receipt(
            process,
            {
                "format_version": True,
                "role": "api",
                "scope_id": self.authority["scope_id"],
                "identity": {},
            },
        )
        with self.assertRaises(ValueError):
            evidence_model.process_document(process, "api", self.authority["scope_id"])

    def test_target_and_environment_descriptors_fail_closed(self):
        backend_path = self.base / "product-backend-build.json"
        frontend_path = self.base / "product-frontend-build.json"
        backend_path.write_text("{}", encoding="utf-8")
        frontend_path.write_text("{}", encoding="utf-8")
        backend_build = read_json_document(backend_path)
        frontend_build = read_json_document(frontend_path)
        resolved = {
            "roots": self.request["roots"],
            "source": {
                "backend_product_sha": self.authority["backend_product_sha"],
                "backend_execution_sha": self.authority["backend_execution_sha"],
                "backend_adapter_contract": None,
                "frontend_sha": self.authority["frontend_sha"],
            },
        }
        facts = {
            "product_plan": {
                "id": self.authority["restore_id"],
                "backup_id": self.authority["backup_id"],
                "scope_id": self.authority["scope_id"],
                "frontend_sha": self.authority["frontend_sha"],
            },
            "endpoints": {
                "api": self.authority["api_endpoint"],
                "worker": self.authority["worker_endpoint"],
                "frontend": self.authority["frontend_endpoint"],
            },
            "product_execution": {
                "roots": {
                    "source_backend": self.request["roots"]["backend_product"],
                    "execution_backend": self.request["roots"]["backend_execution"],
                    "frontend": self.request["roots"]["frontend"],
                },
                "backend_product_sha": self.authority["backend_product_sha"],
                "backend_execution_sha": self.authority["backend_execution_sha"],
                "frontend_sha": self.authority["frontend_sha"],
                "builds": {
                    "backend": launch_model._document_descriptor(backend_build),
                    "frontend": launch_model._document_descriptor(frontend_build),
                },
                "adapter": None,
            },
        }
        launch_model._target_matches(
            self.authority, facts, resolved, backend_build, frontend_build
        )
        with self.assertRaisesRegex(ValueError, "目标计划"):
            launch_model._target_matches(
                {**self.authority, "frontend_sha": "e" * 40},
                facts,
                resolved,
                backend_build,
                frontend_build,
            )
        changed = json.loads(json.dumps(facts))
        changed["product_execution"]["backend_execution_sha"] = "e" * 40
        with self.assertRaisesRegex(ValueError, "product execution"):
            launch_model._target_matches(
                self.authority, changed, resolved, backend_build, frontend_build
            )
        environment_path = self.base / "launch-environment.json"
        private = {"scope_id": "runtime-test", "metrics_bearer_token": "private-token",
                   "api_config_dir": str(self.base / "api"), "worker_config_dir": str(self.base / "worker")}
        environment_path.write_text(json.dumps(private), encoding="utf-8")
        environment = read_json_document(environment_path)
        facts["fresh_target"] = {
            "environment": {
                "path": str(environment.path),
                "bytes": len(environment.raw),
                "sha256": environment.sha256,
            }
        }
        document, private = launch_model._environment_document(self.coordinator, facts)
        self.assertEqual(private["scope_id"], "runtime-test")
        environment_path.write_text('{"environment":{}}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "描述不一致"):
            launch_model._environment_document(self.coordinator, facts)
        self.assertEqual(document.sha256, facts["fresh_target"]["environment"]["sha256"])

if __name__ == "__main__":
    unittest.main()
