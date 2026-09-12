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
    setUp = fixtures.SeedTests.setUp
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
            runtime=self.directory / "runtime", target={"target": True}, output=self.directory,
            review={"review": True}, run=MagicMock(), environments=environments,
            api_guard=MagicMock())
        confirmation_check = patch.object(runtime, "verify_confirmations")
        with patch.object(runtime, "registered", return_value=request), \
                patch.object(runtime, "history", return_value=post), \
                patch.object(runtime, "identity_inputs"), \
                confirmation_check as verify_confirmations, \
                patch.object(runtime, "require_schedule_stage"), \
                patch.object(runtime, "source_binding", return_value={"scope": "target"}), \
                patch.object(runtime, "verify_api_address"), \
                patch.object(runtime, "worker_ready_url", return_value=context.selected["worker_ready_url"]), \
                patch.object(runtime, "absent"), patch.object(runtime, "require_closed_port"), \
                patch.object(runtime, "Resources", return_value=resources), \
                patch("devex_clone_department_model.require_prerequisite"):
            self.assertEqual(runtime.current_mutation_guard(
                context, request, prerequisite, ["confirmed"]), ["confirmed"])
        verify_confirmations.assert_not_called()
        context.api_guard.assert_called_once_with(resources)
        resources.storage_identity.assert_not_called()
        resources.tools.verify_databases.assert_not_called()
        resources.tools.verify_objects.assert_not_called()

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

        def register_runtime(backend, directory):
            write_json(directory / "runtime.json", old)
            return old

        number = begin(self.directory, "seed-runtime", "prepare", {})
        with patch.object(runtime, "current_guard", return_value=["schedule-proof"]), patch.object(runtime, "require_closed_port"), patch.object(runtime, "verify_runtime", return_value=old), patch.object(runtime, "register_runtime", side_effect=register_runtime):
            result = runtime.prepare(self.context, self.request)
        finish(self.directory, number, result=result)
        self.assertEqual(old_api, binding(self.old / "api.json"))
        self.new = self.directory / "seed-runtime/runtime"
        return read_json(self.new / "handoff.json")

    def history(self, *, incomplete=False):
        events = []
        for index, role in enumerate(("api", "worker")):
            identity = {**self.identity, "pid": self.identity["pid"] + index + 1}
            write_json(self.new / (role + ".json"), {"format_version": 1, "scope_id": self.scope, "role": role, "identity": identity})
            events.append({"sequence": len(events), "event": "start-intent", "role": role, "token": role})
            if not incomplete:
                events.append({"sequence": len(events), "event": "started", "role": role, "token": role, "identity": identity})
        write_json(self.new / "producer-history.json", {"format_version": 1, "kind": "devex-clone-producer-history",
            "scope_id": self.scope, "runtime_directory": str(self.new), "events": events})

    def stop_result(self, *, fail=False):
        value = {"format_version": 1, "kind": "devex-clone-runtime-control", "operation": "stop", "scope_id": self.scope,
                 "runtime_directory": str(self.new), "processes": {role: {"state": "stopped", "identity": None, "ready": False}
                                                                   for role in ("api", "worker")}}
        number = begin(self.directory, "seed-runtime", "stop", {})
        with patch("devex_clone_runtime.control", return_value=value), patch.object(runtime, "process_identity", return_value=None):
            result = runtime.execute_seed(self.backend, self.directory, None, "stop", number)
        finish(self.directory, number, result=result, error=ValueError("stop outer failure") if fail else None)
        return result

    def test_prepare_new_runtime_preserves_historical_api_and_identity_plan(self):
        old_plan = self.request["identity_plan"]
        handoff = self.prepared()
        self.assertEqual(handoff["original_api"], self.request["api_process"])
        self.assertEqual(binding(Path(old_plan["path"])), old_plan)
        self.assertNotEqual(self.new, self.old)
        self.assertEqual(runtime.runtime_inputs(self.backend, self.directory)[0], self.new)

    def test_start_checks_current_state_before_and_after_control_without_rechecking_old_rows(self):
        self.prepared()
        number = begin(self.directory, "seed-runtime", "start", {})
        order = []
        with patch.object(runtime, "current_guard", side_effect=lambda *_args: order.append("guard") or ["schedule-proof"]), patch.object(runtime, "old_api_stopped", side_effect=lambda *_args: order.append("old-stopped")), patch("devex_clone_runtime.control", side_effect=lambda *_args: order.append("start") or {"running": True}):
            result = runtime.start(self.context, self.request, number)
        self.assertEqual(order, ["guard", "old-stopped", "start", "guard"])
        self.assertFalse(result["outbox_drained"])

    def test_failed_start_then_precise_stop_can_explicitly_restart_same_handoff(self):
        handoff = self.prepared()
        self.history()
        failed = begin(self.directory, "seed-runtime", "start", {})
        finish(self.directory, failed, error=ValueError("outer failure"))
        stopped = self.stop_result()
        self.assertTrue(stopped["restart_ready"])
        with patch.object(runtime, "process_identity", return_value=None):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff,
                                      {"number": failed, "stage": "seed-runtime", "mode": "start"}, 999)

    def test_failed_stop_or_unknown_start_intent_cannot_allow_restart(self):
        handoff = self.prepared()
        self.history(incomplete=True)
        failed = begin(self.directory, "seed-runtime", "start", {})
        finish(self.directory, failed, error=ValueError("start failure"))
        result = self.stop_result()
        self.assertFalse(result["restart_ready"])
        with patch.object(runtime, "process_identity", return_value=None), self.assertRaises(ValueError):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff,
                                      {"number": failed, "stage": "seed-runtime", "mode": "start"}, 999)
        self.stop_result(fail=True)
        with self.assertRaises(ValueError):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff,
                                      {"number": failed, "stage": "seed-runtime", "mode": "start"}, 999)

    def test_stale_stop_receipt_changed_handoff_or_later_identity_failure_rejected(self):
        handoff = self.prepared()
        self.history()
        failed = begin(self.directory, "seed-runtime", "start", {})
        finish(self.directory, failed, error=ValueError("start failure"))
        self.stop_result()
        (self.new / "api.json").write_text('{"changed":true}', encoding="utf-8")
        with patch.object(runtime, "process_identity", return_value=None), self.assertRaises(ValueError):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff,
                                      {"number": failed, "stage": "seed-runtime", "mode": "start"}, 999)
        with self.assertRaises(ValueError):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff,
                                      {"number": failed, "stage": "seed-runtime", "mode": "identities-apply"}, 999)

    def test_other_handoff_or_stop_older_than_failure_does_not_release_start(self):
        handoff = self.prepared()
        self.history()
        self.stop_result()
        failed = begin(self.directory, "seed-runtime", "start", {})
        finish(self.directory, failed, error=ValueError("new failure"))
        prior = {"number": failed, "stage": "seed-runtime", "mode": "start"}
        with patch.object(runtime, "process_identity", return_value=None), self.assertRaises(ValueError):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff, prior, 999)
        self.stop_result()
        (self.new / "handoff.json").write_text('{"other_generation":true}', encoding="utf-8")
        with patch.object(runtime, "process_identity", return_value=None), self.assertRaises(ValueError):
            runtime.restart_after_stop(self.backend, self.directory, self.new, handoff, prior, 999)

    def test_outer_cleanup_skips_current_source_failures_and_uses_registered_manifest(self):
        import devex_clone_run as run
        from contextlib import nullcontext

        self.register()
        owner = {"identity": self.identity}
        expected = {"status": "seed_runtime_stop"}
        with patch.object(run, "process_guard", return_value=nullcontext()), patch.object(run, "claim_run_lock", return_value=nullcontext(owner)), patch.object(run, "bind_controller_attempt"), patch("source_fingerprints.current_execution_source", side_effect=ValueError("later source changed")), patch.object(runtime, "execute_seed", return_value=expected) as execute, patch.object(run, "environments", side_effect=AssertionError("must not load source environment")), patch.object(run, "read_manifest", side_effect=AssertionError("must not full-validate manifest")):
            # 最小清理仍要求原固定 manifest 的字段完整；此处只替代读取，保留状态机真实发布。
            with patch.object(run, "registered_manifest", return_value={"fixed": True}):
                result = run.execute(self.backend, self.directory, "seed-runtime", "stop")
        self.assertEqual(result["result"], expected)
        self.assertEqual(execute.call_args.args[3], "stop")

    def test_cli_requires_exact_operation_inputs_and_status_is_readonly(self):
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
        with patch.object(runtime, "execute_seed", return_value={"status": "observed"}) as observe:
            self.assertEqual(cli.dispatch(args("status"), self.backend), {"status": "observed"})
            self.assertEqual(observe.call_args.args[2:], (None, "status", None))

        request = self.directory / "arm-request.json"
        write_json(request, {"kind": "fixture-arm-input"})
        completed = {"status": "stage_finished", "stage": "seed-runtime", "mode": "arm-input",
                     "attempt": 53, "restore_qualified": False}
        with patch.object(cli, "execute", return_value=completed) as execute:
            self.assertEqual(cli.dispatch(args("arm-input", request=request, write=True),
                                          self.backend), completed)
        self.assertEqual(execute.call_args.kwargs["seed_request"], request)

    def test_status_uses_zero_write_runtime_observation(self):
        runtime_dir = self.directory / "seed-runtime/runtime"
        handoff = {"api_url": "http://127.0.0.1:18210"}
        expected = {"kind": "devex-clone-runtime-observation", "operation": "status"}
        with patch.object(runtime, "runtime_inputs", return_value=(runtime_dir, handoff, {})), \
                patch("devex_clone_runtime.observe", return_value=expected) as observe, \
                patch("devex_clone_runtime.control", side_effect=AssertionError("status 不能取得控制锁")):
            result = runtime.execute_seed(
                self.backend, self.directory, None, "status", None,
            )
        self.assertEqual(result, {
            "status": "seed_runtime_status",
            "runtime": expected,
            "restore_qualified": False,
        })
        observe.assert_called_once_with(
            self.backend, runtime_dir, ("api", "worker"), handoff["api_url"],
        )

    def test_arm_input_dispatches_evidence_only_publisher_without_target_lock(self):
        request = self.directory / "arm-request.json"
        write_json(request, {"kind": "fixture-arm-input"})
        expected = {"status": "seed_arm_input_published", "remote_writes": 0}
        with patch.object(runtime, "require_quiet") as quiet, \
                patch("devex_clone_seed_arm.publish_arm_input", return_value=expected) as publish, \
                patch.object(runtime, "target_lock", side_effect=AssertionError("unexpected target lock")):
            result = runtime.execute_seed(self.backend, self.directory, {}, "arm-input", 53, request)
        self.assertEqual(result, expected)
        quiet.assert_called_once_with(self.backend, self.directory, 53)
        publish.assert_called_once_with(self.backend, self.directory, request, 53)

    def test_start_rejects_capacity_proof_changed_after_prepare(self):
        self.prepared()
        number = begin(self.directory, "seed-runtime", "start", {})
        with patch.object(runtime, "capacity_evidence", return_value={"changed": True}), patch("devex_clone_runtime.control") as control:
            with self.assertRaises(ValueError):
                runtime.start(self.context, self.request, number)
        control.assert_not_called()

    def test_registered_identity_source_cannot_restart_and_overwrite_old_api(self):
        import devex_clone_run as run

        self.register()
        old = binding(self.old / "api.json")
        with patch("devex_clone_post.runtime_inputs", return_value=({"runtime_dir": str(self.old), "api_url": "http://127.0.0.1:18210"}, self.api)), patch("devex_clone_runtime.control") as control, self.assertRaises(ValueError):
            run.run_runtime(self.backend, {}, None, "target", "start", ("api",), run_directory=self.directory)
        control.assert_not_called()
        self.assertEqual(binding(self.old / "api.json"), old)


if __name__ == "__main__":
    unittest.main()
