"""身份和新运行登记的离线边界；不连接实际资源。"""
import copy
from contextlib import nullcontext
import hashlib
import json
from pathlib import Path
import sys
import subprocess
from types import SimpleNamespace
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch, Mock

import devex_clone_seed as seed
import devex_clone_seed_runtime as runtime
from devex_clone_capture import read_json, write_json
from devex_clone_post import FIELDS as POST_FIELDS
from devex_clone_run_state import binding, initialize_state, begin, finish, load_state
from restore_reference_plan import plan_hash


class SeedTests(unittest.TestCase):
    def setUp(self):
        repository = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = WorkspaceDirectory(dir=repository / ".local-tests/tmp", prefix="seed-evidence-")
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve() / "backend"
        self.directory = self.backend / ".local-tests/run"
        self.directory.mkdir(parents=True)
        self.old = self.directory / "post-copy/runtime"
        self.old.mkdir(parents=True)
        self.scope = "seed-scope"
        self.api = {"APP_ENV": "test"}
        self.identity = {"pid": 991, "started": "222", "executable": str(self.backend / "api.exe")}
        self.file("manifest", {"copy_directory": str(self.directory / "copy")})
        initialize_state(self.directory)
        copied = self.stage("copy", "resume", {"status": "data_steps_verified"})
        generic = self.file("generic", {"safe": True})
        credentials = self.backend / ".local-tests/administrator.toml"
        credentials.write_text('[administrator]\ntenant_id="system"\nusername="admin"\npassword="Current!Changed123"\n', encoding="utf-8")
        self.post = {field: generic for field in POST_FIELDS}
        self.post.update(format_version=1, kind="devex-clone-post-copy", run_manifest=binding(self.directory / "manifest.json"),
            copy_stage_receipt=copied, copy_result=self.file("copy-result", {"status": "data_steps_verified"}),
            ledger_head=self.file("copy-head", {"sequence": 8}), api_environment=self.file("api-environment", {"environment": self.api}),
            frontend_root=str(self.backend.parent / "ryframe-vue3"), node={"path": str(self.backend / "node.exe"), "sha256": "1" * 64},
            source_admin={"subject_id": "101", "tenant_id": "system", "username": "admin", "client_address": "198.19.20.1"},
            administrator_credentials=binding(credentials))
        self.file("post-copy", self.post)
        self.stage("post-copy", "register", {"status": "post_copy_registered", "registration": binding(self.directory / "post-copy.json")})
        post_verified = self.stage("post-copy", "verify", {"status": "post_copy_existing_data_verified",
            "registration": binding(self.directory / "post-copy.json"), "worker_must_remain_stopped": True})
        write_json(self.old / "runtime.json", {"scope_id": self.scope, "api_config_dir": str(self.old / "config")})
        write_json(self.old / "api.json", {"format_version": 1, "scope_id": self.scope, "role": "api", "identity": self.identity})
        write_json(self.old / "handoff.json", {"api_url": "http://127.0.0.1:18210"})
        document = self.file("environment-document", {"machine": "fixture"})
        groups = []
        for index in range(11):
            slot = "system" if index == 0 else f"tenant-{index:02d}"
            users = [{"user_slot": f"{slot}-{n:03d}", "username": f"user-{n:03d}",
                      "credential_slot": "system" if index == 0 else "tenant", "client_address": f"198.18.{index + 40}.{n + 1}"}
                     for n in range(100 if index == 0 else 10)]
            permissions = (["monitor:schedule:add", "monitor:schedule:run", "system:message:publish",
                            "system:post:export"] if index == 0 else
                           ["system:post:add", "system:post:edit", "system:post:export", "system:post:list",
                            "system:post:remove", "system:user-import:add", "system:user-import:list"])
            groups.append({"slot": slot, "kind": "system" if index == 0 else "tenant",
                "tenant_id": slot, "role_code": "devex-system" if index == 0 else "devex-tenant",
                "permissions": permissions,
                "users": users, "admin": {"tenant_id": slot, "username": "admin", "client_address": f"198.18.30.{index+1}"}})
        self.plan = {"format_version": 1, "kind": "devex-identities", "groups": groups, "environment": {
            "scope_id": self.scope, "backend_dir": str(self.backend), "frontend_dir": self.post["frontend_root"],
            "api_url": "http://127.0.0.1:18210", "frontend_url": "http://127.0.0.1:4190",
            "runtime": {"directory": str(self.old), "receipt_sha256": binding(self.old / "runtime.json")["sha256"],
                        "api_process_sha256": binding(self.old / "api.json")["sha256"]},
            "environment_document": {key: document[key] for key in ("path", "sha256")},
            "system_admin": groups[0]["admin"], "tenants": [{"slot": g["slot"], "tenant_id": g["tenant_id"], "target_key": "shared", "admin": g["admin"]} for g in groups[1:]], "database": {"control": {"host": "127.0.0.1", "port": 3306, "database": "identity_control", "username": "fixture", "tls_mode": "required"}, "targets": []}}}
        self.private = copy.deepcopy(self.plan["environment"])
        self.private["passwords"] = {"system": "private-users", "tenant": "private-users"}
        self.private["system_admin"]["password"] = "private-admin"
        for tenant in self.private["tenants"]:
            tenant["admin"]["password"] = "private-admin"
        self.private["database"]["control"]["password"] = "private-database"
        self.plan["configuration"] = self.file("identity-private", self.private)
        self.plan["plan_sha256"] = plan_hash(self.plan)
        self.api_table = {"scope_id": self.scope, "cors": {"allow_origins": ["http://127.0.0.1:4190"]}, "database": {"primary": self.private["database"]["control"]}}
        self.addCleanup(patch.stopall)
        patch("rate_limit_client.load_app_table", return_value=self.api_table).start()
        self.request = {"format_version": 1, "kind": "devex-clone-seed-runtime",
            "run_manifest": binding(self.directory / "manifest.json"), "post_copy": binding(self.directory / "post-copy.json"),
            "post_verify": post_verified, "node": self.post["node"], "identity_plan": self.file("identity-plan", self.plan),
            "identity_state": str(self.directory / "seed-runtime/identities"),
            "identity_tool": {"path": str(Path(sys.executable).resolve()), "sha256": hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest()},
            "api_process": binding(self.old / "api.json")}

    def file(self, name, value):
        path = self.directory / (name + ".json")
        path.write_text(json.dumps(value), encoding="utf-8")
        return binding(path)

    def stage(self, stage, mode, result=None, error=None):
        number = begin(self.directory, stage, mode, {})
        finish(self.directory, number, result=result, error=error)
        return load_state(self.directory)["attempts"][-1]["result"]

    def register(self):
        self.file("seed-request", self.request)
        number = begin(self.directory, "seed-runtime", "register", {})
        # 其余部门/close/source 测试复用已发布登记夹具；真实登记写入在 Rust publication_tests 覆盖。
        write_json(self.directory / "seed-runtime.json", self.request)
        result = {"status": "seed_runtime_registered", "registration": binding(self.directory / "seed-runtime.json"),
                  "worker_authorized": False, "remote_writes": 0, "restore_qualified": False}
        finish(self.directory, number, result=result)
        return result

    def evidence(self):
        # 已完成身份阶段留下此受控父目录；完整身份数据由 Rust verifier 测试覆盖。
        (self.directory / "seed-runtime").mkdir(exist_ok=True)
        value = {"ledger": {"fixture": "ledger"}, "publication_intent": {"fixture": "intent"},
                 "verified": {"fixture": "receipt"}, "templates": []}
        for module in (seed, runtime):
            mocked = patch.object(module, "identity_evidence", return_value=value)
            mocked.start()
            self.addCleanup(mocked.stop)
        return value


    def test_history_allows_legitimate_identity_and_outbox_data_evolution(self):
        self.register()
        self.file("current-business-image", {"identity_count": 200, "outbox_delivered": 123})
        self.assertEqual(seed.history(self.backend, self.directory, self.request), self.post)

    def test_wrong_run_scope_runtime_and_old_post_evidence_rejected(self):
        for key in ("post_copy", "post_verify", "run_manifest"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                seed.history(self.backend, self.directory, {**self.request, key: self.file("wrong", {})})
        for key, value in (("scope_id", "other-scope"), ("api_url", "http://127.0.0.1:18211"),
                           ("frontend_url", "http://127.0.0.1:4191")):
            plan = copy.deepcopy(self.plan)
            plan["environment"][key] = value
            plan["plan_sha256"] = plan_hash({k: v for k, v in plan.items() if k != "plan_sha256"})
            request = {**self.request, "identity_plan": self.file("wrong-plan", plan)}
            with self.subTest(key=key), self.assertRaises(ValueError):
                seed.identity_inputs(self.backend, self.directory, request, self.post)

    def test_identity_inputs_use_the_bound_post_amendment_runtime(self):
        runtime = self.directory / "post-copy/runtime-0001"
        runtime.mkdir(parents=True)
        write_json(runtime / "runtime.json", {"scope_id": self.scope, "api_config_dir": str(runtime / "config")})
        write_json(runtime / "api.json", {"format_version": 1, "scope_id": self.scope,
                                           "role": "api", "identity": self.identity})
        write_json(runtime / "handoff.json", {"api_url": "http://127.0.0.1:18210"})
        amendment = self.file("post-amendment", {"sequence": 1})
        plan = copy.deepcopy(self.plan)
        plan["environment"]["runtime"] = {"directory": str(runtime),
            "receipt_sha256": binding(runtime / "runtime.json")["sha256"],
            "api_process_sha256": binding(runtime / "api.json")["sha256"]}
        private_config = copy.deepcopy(self.private)
        private_config["runtime"] = plan["environment"]["runtime"]
        plan["configuration"] = self.file("amended-private", private_config)
        plan["plan_sha256"] = plan_hash({key: value for key, value in plan.items() if key != "plan_sha256"})
        request = {**self.request, "post_copy": amendment, "identity_plan": self.file("amended-plan", plan),
                   "api_process": binding(runtime / "api.json")}
        active = SimpleNamespace(descriptor=amendment, request=self.post, runtime=runtime)
        with patch.object(seed, "post_registration", return_value=active):
            observed, private = seed.identity_inputs(self.backend, self.directory, request, self.post)
        self.assertEqual(observed, plan)
        self.assertEqual(private, {"APP_ENV": "test"})

    def test_later_failed_copy_or_post_does_not_select_old_success(self):
        for stage, mode in (("copy", "reconcile"), ("post-copy", "verify")):
            with self.subTest(stage=stage):
                self.stage(stage, mode, error=ValueError("fixture"))
                with self.assertRaises(ValueError):
                    seed.history(self.backend, self.directory, self.request)

    def test_identity_private_file_drift_is_rejected_without_leaking_secrets(self):
        original = Path(self.plan["configuration"]["path"]).read_bytes()
        for key in ("scope_id", "passwords", "database"):
            changed = copy.deepcopy(self.private)
            changed[key] = "untrusted-secret"
            Path(self.plan["configuration"]["path"]).write_text(json.dumps(changed), encoding="utf-8")
            with self.subTest(key=key), self.assertRaises(ValueError) as failure:
                seed.private_configuration(self.backend, self.plan)
            self.assertNotIn("untrusted-secret", str(failure.exception))
            Path(self.plan["configuration"]["path"]).write_bytes(original)


    def test_identity_outer_failure_requires_explicit_verify_same_ledger(self):
        self.register()
        evidence = {"ledger": {"fixture": "ledger"}, "publication_intent": {"fixture": "intent"},
                    "verified": {"fixture": "receipt"}, "templates": []}
        departments = {"fixture_departments": True}
        producer = self.file("producer", {"identity": self.identity})
        result = {"status": "seed_identities_prepared", "registration": binding(self.directory / "seed-runtime.json"),
                  "identity": {"state": "prepared"}, "producer": producer}
        with patch.object(seed, "identity_evidence", return_value=evidence), \
                patch("devex_clone_department_model.department_evidence", return_value=departments):
            self.stage("seed-runtime", "identities-apply", result, ValueError("outer failure"))
            with self.assertRaises(ValueError):
                seed.verified_identity_stage(self.backend, self.directory, self.request)
            result = {**result, "status": "seed_identities_verified", "identity": evidence,
                      "departments": departments}
            self.stage("seed-runtime", "identities-verify", result)
            self.assertEqual(seed.verified_identity_stage(self.backend, self.directory, self.request)["identity"], evidence)
            self.stage("seed-runtime", "identities-verify", result, ValueError("new failure"))
            with self.assertRaises(ValueError):
                seed.verified_identity_stage(self.backend, self.directory, self.request)

    def test_current_schedules_unknown_or_enabled_rejected_before_control(self):
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, target={}, output=self.directory,
            selected={}, review={}, run=Mock(), target_config={"databases": [{"key": "shared-control"}]},
            environments=runtime.Environments({}, {}), resolved_storage_run=lambda: self.directory)
        resources = SimpleNamespace(tools=SimpleNamespace(mysql=Mock(return_value="0")))
        with patch.object(runtime, "verify_confirmations", return_value=["confirmed"]), patch.object(runtime, "require_schedule_stage"), patch.object(runtime, "Resources", return_value=resources):
            self.assertEqual(runtime.schedules_guard(context), ["confirmed"])
            for raw in ("", "1", "0\n0", "NULL"):
                resources.tools.mysql.return_value = raw
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    runtime.schedules_guard(context)

    def test_bound_runtime_inputs_survive_later_failure_and_tool_changes(self):
        self.register()
        root = self.directory / "seed-runtime/runtime"
        root.mkdir(parents=True)
        write_json(root / "runtime.json", {"scope_id": self.scope})
        write_json(root / "binaries.json", {"ryframe": "fixture.exe"})
        handoff = {"registration": binding(self.directory / "seed-runtime.json"), "api_url": "http://127.0.0.1:18210",
                   "runtime": binding(root / "runtime.json"), "binaries": binding(root / "binaries.json")}
        write_json(root / "handoff.json", handoff)
        self.stage("seed-runtime", "prepare", {"status": "seed_runtime_prepared", "handoff": binding(root / "handoff.json")})
        self.stage("seed-runtime", "start", error=ValueError("outer failed after start"))
        self.file("identity-plan", {"later_tool_unavailable": True})
        selected, observed, _ = runtime.runtime_inputs(self.backend, self.directory)
        self.assertEqual(selected, root)
        self.assertEqual(observed, handoff)
        self.assertEqual(binding(self.old / "api.json"), self.request["api_process"])

    def test_failed_start_cannot_be_masked_by_successful_stop(self):
        self.stage("seed-runtime", "start", error=ValueError("failed"))
        self.stage("seed-runtime", "stop", {"status": "stopped"})
        context = SimpleNamespace(directory_root=self.directory, backend=self.backend)
        with patch("devex_clone_runtime.control") as control, self.assertRaises(ValueError):
            runtime.start(context, self.request, 999)
        control.assert_not_called()


class IdentityEvidenceAdapterTests(unittest.TestCase):
    def invoke(self, response, *, code=0):
        runner = Mock(return_value=SimpleNamespace(returncode=code, stdout=response))
        with patch("rate_limit_tool.tool_binding", return_value={"fixture": True}), \
                patch("rate_limit_tool.verified_tool", return_value=nullcontext("bound-tool.exe")), \
                patch("subprocess.run", runner):
            result = seed.identity_evidence(Path("D:/backend"), Path("D:/backend/.local-tests/run"),
                                            {"registered": "current"})
        return result, runner

    def test_identity_evidence_delegates_once_using_only_bound_seed_stdin(self):
        evidence = {"ledger": {"fixture": "native-verifier"}}
        result, runner = self.invoke(json.dumps({"ok": True, "result": evidence}).encode())
        self.assertEqual(result, evidence)
        runner.assert_called_once()
        self.assertEqual(runner.call_args.args[0], ["bound-tool.exe", "__seed-evidence"])
        payload = json.loads(runner.call_args.kwargs["input"])
        self.assertEqual(payload, {"format_version": 1, "kind": "ryframe-seed-evidence",
            "run_dir": str(Path("D:/backend/.local-tests/run")), "operation": "identity-evidence",
            "arguments": {"request": {"registered": "current"}}})
        self.assertFalse(runner.call_args.kwargs["check"])
        self.assertEqual(runner.call_args.kwargs["stderr"], subprocess.DEVNULL)

    def test_missing_binding_or_oversized_request_never_starts_a_process(self):
        for request in ({"request": "current"}, {"request": "界" * 22000}):
            with patch("rate_limit_tool.tool_binding", return_value=None), \
                    patch("rate_limit_tool.verified_tool", return_value=nullcontext(None)), \
                    patch("subprocess.run") as runner, self.assertRaises(ValueError):
                seed.identity_evidence(Path("D:/backend"), Path("D:/backend/.local-tests/run"), request)
            runner.assert_not_called()

    def test_rejected_malformed_or_interrupted_native_results_do_not_retry(self):
        for raw, code in [(b"not-json", 0), (b'{"ok":true,"ok":false}', 0),
                          (b'{"ok":false,"error":{"message":"fixture-secret"}}', 1),
                          (b'{"ok":true,"result":[]}', 0),
                          (b'{"ok":true,"result":{}}', 1), (b"x" * (1024 * 1024 + 1), 0)]:
            with self.subTest(code=code, size=len(raw)), self.assertRaises(ValueError) as failure:
                self.invoke(raw, code=code)
            self.assertNotIn("fixture-secret", str(failure.exception))
        for error in (subprocess.TimeoutExpired("bound-tool", 180), KeyboardInterrupt()):
            runner = Mock(side_effect=error)
            with patch("rate_limit_tool.tool_binding", return_value={"fixture": True}), \
                    patch("rate_limit_tool.verified_tool", return_value=nullcontext("bound-tool.exe")), \
                    patch("subprocess.run", runner), \
                    self.assertRaises(KeyboardInterrupt if isinstance(error, KeyboardInterrupt) else ValueError):
                seed.identity_evidence(Path("D:/backend"), Path("D:/backend/.local-tests/run"), {})
            runner.assert_called_once()


if __name__ == "__main__":
    unittest.main()
