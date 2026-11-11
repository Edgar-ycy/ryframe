"""新 runtime 的准备、未知启动、精确停止与外层阶段接线。"""
from pathlib import Path
from types import SimpleNamespace
import json
import unittest
from unittest.mock import MagicMock, patch

import devex_clone_seed_runtime as runtime
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding, begin, finish
import test_devex_clone_seed as fixtures


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        fixtures.SeedTests.setUp(self)
        self.config_dir = self.directory / "config"
        self.config_dir.mkdir(exist_ok=True)
        execution = patch.object(runtime, "execution_root", return_value=self.backend)
        execution.start()
        self.addCleanup(execution.stop)
        binding = patch.object(runtime, "application_config_directory", return_value=self.config_dir)
        binding.start()
        self.addCleanup(binding.stop)
    file = fixtures.SeedTests.file
    stage = fixtures.SeedTests.stage
    register = fixtures.SeedTests.register
    evidence = fixtures.SeedTests.evidence

    def test_mutation_guard_checks_live_api_and_schedule_without_full_resource_scan(self):
        request = {"fixture": "seed"}
        post = {"fixture": "post"}
        prerequisite = {"fixture": "departments"}
        resources = MagicMock()
        resources.tools.mysql.return_value = "0"
        environments = MagicMock()
        environments.use.return_value.__enter__.return_value = {}
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory,
            target_config={"scope_id": self.scope,
                "databases": [{"key": "shared-control", "database": "control"}]},
            initial={"generation": {"physical": {"scope": "target"}}},
            selected={"api_url": "http://127.0.0.1:18210",
                      "worker_ready_url": "http://127.0.0.1:18211/readyz",
                      "runtime_dir": str(self.directory / "candidate")},
            runtime=self.directory / "runtime", target={"target": True}, target_descriptor={"bound": "request"}, output=self.directory,
            review={"review": True}, run=MagicMock(), environments=environments,
            api_guard=MagicMock(), resolved_storage_run=lambda: self.directory)
        confirmation_check = patch.object(runtime, "verify_confirmations")
        with patch.object(runtime, "registered", return_value=request), \
                patch.object(runtime, "history", return_value=post), \
                patch.object(runtime, "identity_inputs"), \
                confirmation_check as verify_confirmations, \
                patch.object(runtime, "require_schedule_stage"), \
                patch.object(runtime, 'native_invoke', return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": {"scope": "target"}}})), \
                patch.object(runtime, "verify_api_address"), \
                patch.object(runtime, "worker_ready_url", return_value=context.selected["worker_ready_url"]), \
                patch.object(runtime, "absent"), patch.object(runtime, "require_closed_port"), \
                patch.object(runtime, "Resources", return_value=resources) as created, \
                patch("devex_clone_department_model.require_prerequisite"):
            self.assertEqual(runtime.current_mutation_guard(
                context, request, prerequisite, ["confirmed"]), ["confirmed"])
        verify_confirmations.assert_not_called()
        self.assertEqual(created.call_args.kwargs["storage_run"], self.directory)
        context.api_guard.assert_called_once_with(resources)
        resources.storage_identity.assert_not_called()
        resources.tools.verify_databases.assert_not_called()
        resources.tools.verify_objects.assert_not_called()

    def test_resources_guard_uses_resolved_fixture_storage_run(self):
        resources = MagicMock()
        environments = MagicMock()
        environments.use.return_value.__enter__.return_value = {}
        fixture_run = self.directory / "fixture-service-run"
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory,
            target={"target": True}, target_descriptor={"bound": "request"}, target_config={"scope_id": self.scope},
            initial={"generation": {"physical": {"scope": "target"}}},
            selected={"api_url": "http://127.0.0.1:18210",
                      "worker_ready_url": "http://127.0.0.1:18211/readyz",
                      "runtime_dir": str(self.directory / "candidate")},
            runtime=self.directory / "runtime", output=self.directory, review={"review": True},
            run=MagicMock(), environments=environments, bindings=MagicMock(),
            resolved_storage_run=lambda: fixture_run)
        with patch.object(runtime, 'native_invoke', return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": {"scope": "target"}}})), \
                patch.object(runtime, "verify_api_address"), \
                patch.object(runtime, "worker_ready_url", return_value=context.selected["worker_ready_url"]), \
                patch.object(runtime, "absent"), \
                patch.object(runtime, "Resources", return_value=resources) as created:
            self.assertIs(runtime.resources_guard(context), resources)
        self.assertEqual(created.call_args.kwargs["storage_run"], fixture_run)
        resources.storage_identity.assert_called_once_with()
        resources.tools.verify_databases.assert_called_once_with("target")
        resources.tools.verify_objects.assert_called_once_with("target")

    def test_fresh_identity_sequence_prepares_before_departments_and_verifies_after_them(self):
        self.register()
        prepared = {"state": "prepared", "projection_sha256": "a" * 64}
        verified = {"ledger": {"path": "ledger.json", "sha256": "b" * 64}}
        departments = {"stage": {"path": "departments.json", "sha256": "c" * 64}}
        producer_receipt = self.file("identity-producer", {"identity": self.identity})
        kinds = []

        class FakeProducer:
            def __init__(inner, _context, kind):
                kinds.append(kind)
                inner.kind = kind

            def communicate(inner, **_kwargs):
                if inner.kind == "identity-apply":
                    value = {"status": "prepared", "plan_sha256": self.plan["plan_sha256"],
                             "entries": 622, "roles": 11, "users": 200}
                else:
                    value = {"status": "verified", "plan_sha256": self.plan["plan_sha256"],
                             "users": 200, "tenants": 10, "message_audience": 10}
                return json.dumps(value)

            def close(inner):
                return None

        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, request=self.post)
        capacity = {"capacity": "fixed"}
        observed = {"alive": False, "receipt": producer_receipt}
        with patch.object(runtime, "capacity_evidence", return_value=capacity), \
                patch.object(runtime, "current_guard"), \
                patch.object(runtime, "identity_inputs", return_value=(self.plan, self.api)), \
                patch.object(runtime, "Producer", FakeProducer), \
                patch.object(runtime, "inspect_producer", return_value=observed), \
                patch.object(runtime, "identity_evidence", return_value=verified), \
                patch("devex_clone_department_model.identity_projection", return_value=prepared) as projection, \
                patch("devex_clone_department_model.authorize_identity", return_value=departments) as authorize:
            context.output = self.directory / "seed-runtime/attempt-identity-apply"
            context.output.mkdir(parents=True)
            applied = runtime.identity_run(context, self.request, "apply")
            self.assertEqual(applied["status"], "seed_identities_prepared")
            self.assertEqual(applied["identity"], prepared)
            self.assertNotIn("departments", applied)
            authorize.assert_not_called()
            self.assertEqual(projection.call_count, 2)

            context.output = self.directory / "seed-runtime/attempt-identity-verify"
            context.output.mkdir()
            verified_result = runtime.identity_run(context, self.request, "verify")
            self.assertEqual(verified_result["status"], "seed_identities_verified")
            self.assertEqual(verified_result["identity"], verified)
            self.assertEqual(verified_result["departments"], departments)
            self.assertEqual(authorize.call_count, 2)
        self.assertEqual(kinds, ["identity-apply", "identity-verify"])

    def prepared(self):
        self.register()
        capacity = patch.object(runtime, "capacity_evidence", return_value={"fixture_capacity": True})
        capacity.start()
        self.addCleanup(capacity.stop)
        departments = {"fixture_departments": True}
        department = patch("devex_clone_department_model.department_evidence", return_value=departments)
        department.start()
        self.addCleanup(department.stop)
        evidence = self.evidence()
        result = {"status": "seed_identities_verified", "registration": binding(self.directory / "seed-runtime.json"),
                  "identity": evidence, "producer": self.file("producer", {"identity": self.identity}),
                  "departments": departments}
        self.stage("seed-runtime", "identities-verify", result)
        self.context = SimpleNamespace(backend=self.backend, directory_root=self.directory, request=self.post,
            runtime=self.old, selected={"api_url": "http://127.0.0.1:18210", "worker_ready_url": "http://127.0.0.1:18211/readyz"},
            target_config={"scope_id": self.scope}, environments=runtime.Environments(self.api, self.api))
        write_json(self.old / "binaries.json", {"ryframe": "api.exe", "ryframe-worker": "worker.exe"})
        old_api = binding(self.old / "api.json")
        old = read_json(self.old / "runtime.json")
        old.update(api_config_dir=str(self.config_dir), worker_config_dir=str(self.directory / "worker-config"))
        (self.old / "runtime.json").write_text(json.dumps(old), encoding="utf-8")

        def register_runtime(backend, directory, *, api_config_dir, worker_config_dir):
            self.assertEqual(api_config_dir, self.config_dir)
            self.assertEqual(worker_config_dir, self.directory / "worker-config")
            write_json(directory / "runtime.json", old)
            return old

        number = begin(self.directory, "seed-runtime", "prepare", {})
        with patch.object(runtime, "current_guard", return_value=["schedule-proof"]), patch.object(runtime, "require_closed_port"), patch.object(runtime, "verify_runtime", return_value=old), patch.object(runtime, "register_runtime", side_effect=register_runtime):
            result = runtime.prepare(self.context, self.request)
        finish(self.directory, number, result=result)
        self.assertEqual(old_api, binding(self.old / "api.json"))
        self.new = self.directory / "seed-runtime/runtime"
        return read_json(self.new / "handoff.json")

    def test_prepare_new_runtime_preserves_historical_api_and_identity_plan(self):
        old_plan = self.request["identity_plan"]
        handoff = self.prepared()
        self.assertEqual(handoff["original_api"], self.request["api_process"])
        self.assertEqual(binding(Path(old_plan["path"])), old_plan)
        self.assertNotEqual(self.new, self.old)
        self.assertEqual(runtime.runtime_inputs(self.backend, self.directory)[0], self.new)

    def test_close_rejects_a_changed_native_handoff_before_control(self):
        handoff = self.prepared()
        number = begin(self.directory, "seed-runtime", "close", {})
        with patch.object(runtime, "invoke_seed_tool") as control:
            with self.assertRaises(ValueError):
                runtime.pair_control(self.backend, self.new, "status", ("api", "worker"), {**handoff, "different": True}, number)
        control.assert_not_called()

    def test_retired_start_cannot_claim_python_target_or_call_role_control(self):
        with patch.object(runtime, "target_lock") as target, patch.object(runtime, "invoke_seed_tool") as control:
            with self.assertRaisesRegex(ValueError, "Rust 正式入口"):
                runtime.execute_seed(self.backend, self.directory, {}, "start", 53)
            with self.assertRaisesRegex(ValueError, "业务关闭"):
                runtime.pair_control(self.backend, self.directory / "seed-runtime/runtime", "start", ("api", "worker"), {}, 53)
        target.assert_not_called()
        control.assert_not_called()

    def test_cli_requires_exact_inputs_and_rejects_native_runtime_operations(self):
        import devex_clone_run_cli as cli

        def args(mode, *, request=None, producer_binding=None, write=False):
            return SimpleNamespace(command="seed-runtime", run_dir=self.directory,
                                   operation=mode, request=request,
                                   producer_binding=producer_binding, write=write)
        for mode in ("register", "arm-input", "source-rebind", "recover-session", "start", "identities-apply", "quotas-plan", "quotas-apply",
                     "quotas-reconcile", "departments-plan", "departments-apply", "departments-reconcile",
                     "departments-verify"):
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                cli.dispatch(args(mode), self.backend)
        with self.assertRaises(ValueError):
            cli.dispatch(args("start", request=self.directory / "unused", write=True), self.backend)
        for mode in ("status", "start", "stop", "recover"):
            with self.subTest(mode=mode), self.assertRaisesRegex(ValueError, "Rust 正式入口"):
                cli.dispatch(args(mode, write=mode != "status"), self.backend)

        request = self.directory / "arm-request.json"
        write_json(request, {"kind": "fixture-arm-input"})
        with patch.object(cli, "execute") as execute, self.assertRaisesRegex(ValueError, "Rust 正式入口"):
            cli.dispatch(args("arm-input", request=request, write=True), self.backend)
        execute.assert_not_called()

    def test_register_calls_bound_native_writer_with_current_controller_and_lock(self):
        from contextlib import nullcontext
        import subprocess

        number = begin(self.directory, "seed-runtime", "register", {})
        write_json(self.directory / f"controller-{number:04d}.json", {"fixture": True})
        request = self.directory / "seed-native-request.json"
        write_json(request, self.request)
        expected = {"status": "seed_runtime_registered", "remote_writes": 0, "worker_authorized": False}
        response = subprocess.CompletedProcess([], 0, json.dumps({"ok": True, "result": expected}).encode())
        with patch.object(runtime, "require_quiet"), \
                patch.object(runtime, "target_lock", return_value=nullcontext(123)), \
                patch("rate_limit_tool.verified_tool", return_value=nullcontext("D:/native/xtask.exe")), \
                patch.object(runtime.subprocess, "run", return_value=response) as execute:
            self.assertEqual(runtime.execute_seed(self.backend, self.directory, {}, "register", number, request), expected)
        call = execute.call_args
        self.assertEqual(call.args[0], ["D:/native/xtask.exe", "__seed-evidence"])
        payload = json.loads(call.kwargs["input"])
        self.assertEqual(payload["arguments"]["attempt"], number)
        self.assertEqual(payload["arguments"]["target_lock_inode"], 123)
        self.assertEqual(payload["arguments"]["controller"], binding(self.directory / f"controller-{number:04d}.json"))
        self.assertFalse(any(key.startswith(("APP_", "RYFRAME_")) for key in call.kwargs["env"]))
        self.assertNotIn("private-admin", str(call))

    def test_register_never_retries_unknown_native_result_or_runs_without_tool_binding(self):
        from contextlib import nullcontext
        import subprocess

        number = begin(self.directory, "seed-runtime", "register", {})
        write_json(self.directory / f"controller-{number:04d}.json", {"fixture": True})
        request = self.directory / "seed-native-request.json"
        write_json(request, self.request)
        for executable, output in [(None, b"{}"), ("D:/native/xtask.exe", b"broken"),
                                   ("D:/native/xtask.exe", b'{"ok": false, "error": {}}')]:
            with self.subTest(executable=executable, output=output), \
                    patch.object(runtime, "require_quiet"), \
                    patch.object(runtime, "target_lock", return_value=nullcontext(123)), \
                    patch("rate_limit_tool.verified_tool", return_value=nullcontext(executable)), \
                    patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, output)) as execute:
                with self.assertRaises(ValueError):
                    runtime.execute_seed(self.backend, self.directory, {}, "register", number, request)
                self.assertEqual(execute.call_count, 0 if executable is None else 1)

    def test_registered_identity_source_rejects_a_replaced_original_api_receipt(self):
        self.register()
        api = self.old / "api.json"
        active = SimpleNamespace(request=self.post, runtime=self.old)
        with patch.object(fixtures.seed, "post_registration", return_value=active):
            plan, _ = runtime.identity_inputs(self.backend, self.directory, self.request, self.post)
            self.assertEqual(plan, self.plan)
            receipt = read_json(api)
            receipt["identity"]["started"] = "223"
            api.write_text(json.dumps(receipt), encoding="utf-8")
            replacement = binding(api)
            self.assertNotEqual(replacement, self.request["api_process"])
            with self.assertRaisesRegex(ValueError, "身份计划必须保留原 API"):
                runtime.identity_inputs(self.backend, self.directory, self.request, self.post)
            self.assertEqual(binding(api), replacement)


if __name__ == "__main__":
    unittest.main()
