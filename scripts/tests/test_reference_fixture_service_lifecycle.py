"""服务生命周期的本地账本与进程边界回归，不访问共享服务。"""
from __future__ import annotations

from contextlib import ExitStack
import copy
import io
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import test_reference_fixture_request as request_fixture
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding, begin, finish, load_state
import reference_fixture_service_context as context
import reference_fixture_service_lifecycle as lifecycle
from reference_fixture_service_history import validate_history
import reference_fixture_services as cli
import devex_clone_target_cli as target


class ServiceLifecycleTests(unittest.TestCase):
    def setUp(self):
        fixture = request_fixture.ReferenceFixtureRequestTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture = fixture
        self.backend, self.root, self.run = fixture.backend, fixture.root, fixture.service_run
        self.review, self.bootstrap = fixture.review_path, fixture.environments["seed"]
        identity = {"pid": 2147481000, "started": "1", "executable": str(Path(sys.executable).resolve())}
        tree = {"format_version": 2, "runtime_directory": str(self.run / "rustfs"), "role": "rustfs",
                "scope_id": "fixture-service-test", "operation_id": "b" * 32, "supervisor": identity,
                "monitor": {**identity, "pid": 2147481001}, "process": {**identity, "pid": 2147481002}}
        self.services = {"requests": {"redis": {}}, "runtime": {}, "tree": tree}

    def snapshot(self):
        return {str(path.relative_to(self.root)): path.read_bytes() for path in self.root.rglob("*") if path.is_file()}

    def mocks(self, *, failure=False):
        stack = ExitStack()
        stack.enter_context(patch.object(lifecycle, "registered_services", return_value=self.services))
        stack.enter_context(patch.object(lifecycle, "observe_services", return_value={"redis": "running", "rustfs": "running"}))
        def redis_stop(_request, _environment, _runtime, output):
            self.events.append("redis")
            if failure:
                raise RuntimeError("Redis 关闭结果未知")
            value = {"status": "redis_process_stopped", "alive": False, "resources_deleted": False}
            write_json(output / "stopped.json", value)
            return value
        stack.enter_context(patch.object(lifecycle, "stop_cache", side_effect=redis_stop))
        stack.enter_context(patch.object(lifecycle, "terminate_owned_process_tree", side_effect=lambda tree: self.events.append("rustfs") or True))
        stack.enter_context(patch.object(lifecycle, "_closed_services", return_value={"redis": "stopped", "rustfs": "stopped"}))
        def completed(_tree):
            from full_stack_process_monitor import receipt_path
            tree = self.services["tree"]
            path = receipt_path(self.run / "rustfs", "rustfs", tree["operation_id"], "stopped")
            write_json(path, {"directory": tree["runtime_directory"], "status": "stopped", "error_type": None,
                              "members": [tree["supervisor"]],
                              **{key: tree[key] for key in ("operation_id", "role", "scope_id", "monitor", "supervisor")}})
            return binding(path)
        stack.enter_context(patch.object(lifecycle, "completion_binding", side_effect=completed))
        self.events = []
        return stack

    def test_status_has_no_writes_and_reports_unique_next_operation(self):
        before = self.snapshot()
        with patch.object(context, "registered_services", return_value=self.services), \
                patch.object(context, "observe_services", return_value={"redis": "running", "rustfs": "running"}):
            result = context.status(self.backend, self.review, self.bootstrap)
        self.assertEqual(result["next_operation"], "close")
        self.assertEqual(before, self.snapshot())
        self.assertFalse((self.run / "run-control.guard").exists())

    def test_close_orders_services_and_preserves_registered_history_prefix(self):
        descriptor = {"path": str(self.run), "manifest": binding(self.run / "manifest.json"), "state": binding(self.run / "state.json")}
        original = copy.deepcopy(load_state(self.run)["attempts"])
        with self.mocks():
            result = lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        self.assertEqual(self.events, ["redis", "rustfs"])
        self.assertEqual(result["status"], "services_closed")
        self.assertEqual(load_state(self.run)["attempts"][:3], original)
        self.assertTrue(validate_history(self.run, load_state(self.run))["closed"])
        self.assertEqual(target._storage_run(self.backend, descriptor), self.run)
        with self.mocks():
            again = lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        self.assertEqual(again["status"], "services_already_closed")
        self.assertEqual(self.events, [])

    def test_unknown_close_result_is_not_replayed_and_rustfs_is_not_stopped(self):
        with self.mocks(failure=True), self.assertRaisesRegex(RuntimeError, "未知"):
            lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        self.assertEqual(self.events, ["redis"])
        with self.mocks(), self.assertRaisesRegex(ValueError, "未成功收尾"):
            lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        self.assertEqual(self.events, [])

    def test_unknown_stage_or_tampered_close_evidence_is_rejected(self):
        with self.mocks():
            lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        evidence = self.run / "lifecycle-0004/stopped.json"
        evidence.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "关闭收据"):
            validate_history(self.run, load_state(self.run))
        state = load_state(self.run)
        state["attempts"][3]["mode"] = "restart"
        with self.assertRaisesRegex(ValueError, "仅允许"):
            validate_history(self.run, state)

    def test_new_requests_reject_closed_service_generations(self):
        with self.mocks():
            lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        from reference_fixture_request import build
        with self.assertRaisesRegex(ValueError, "关闭"):
            build(self.backend, self.bootstrap, self.run, "fresh-seed", "seed")

    def test_status_pid_reuse_fails_without_any_file_changes(self):
        before = self.snapshot()
        observation = {"owner": {}, "process_matches": False, "process_missing": False}
        with patch.object(context, "controller_observation", return_value=observation), \
                self.assertRaisesRegex(ValueError, "PID 已复用"):
            context.status(self.backend, self.review, self.bootstrap)
        self.assertEqual(before, self.snapshot())

    def dead_lock(self):
        owner = {"format_version": 1, "directory": str(self.run),
                 "manifest_sha256": binding(self.run / "manifest.json")["sha256"],
                 "identity": {"pid": 2147483000, "started": "1", "executable": str(Path(sys.executable).resolve())}}
        lock = self.run / "run.lock"
        lock.mkdir()
        write_json(lock / "owner.json", owner)
        owner_file = self.root / "owner-binding.json"
        write_json(owner_file, binding(lock / "owner.json"))
        return owner_file

    def test_recover_only_removes_dead_controller_and_appends_auditable_receipt(self):
        owner_file = self.dead_lock()
        with patch("devex_clone_run_state.process_identity", side_effect=lambda pid: None if pid == 2147483000 else __import__("full_stack_process").process_identity(pid)), \
                patch.object(lifecycle, "stop_cache") as cache_stop, patch.object(lifecycle, "terminate_owned_process_tree") as tree_stop:
            result = lifecycle.recover(self.backend, self.review, self.bootstrap, owner_file, write=True)
        self.assertEqual(result["next_operation"], "status")
        self.assertFalse((self.run / "run.lock").exists())
        cache_stop.assert_not_called()
        tree_stop.assert_not_called()
        self.assertFalse(validate_history(self.run, load_state(self.run))["closed"])

    def test_recover_rejects_live_or_reused_controller_without_writes(self):
        owner_file = self.dead_lock()
        before = self.snapshot()
        with patch("devex_clone_run_state.process_identity", return_value={"pid": 2147483000, "started": "changed", "executable": "other"}), \
                self.assertRaisesRegex(ValueError, "PID 复用"):
            lifecycle.recover(self.backend, self.review, self.bootstrap, owner_file, write=True)
        self.assertEqual(before, self.snapshot())

    def test_partial_startup_recovery_never_suggests_an_illegal_next_stage(self):
        state = load_state(self.run)
        for item in state["attempts"][1:]:
            Path(item["result"]["path"]).unlink()
        state["attempts"] = state["attempts"][:1]
        self.fixture.write(self.run / "state.json", state)
        owner_file = self.dead_lock()
        with patch("devex_clone_run_state.process_identity", side_effect=lambda pid: None if pid == 2147483000 else __import__("full_stack_process").process_identity(pid)):
            lifecycle.recover(self.backend, self.review, self.bootstrap, owner_file, write=True)
        before = self.snapshot()
        result = context.status(self.backend, self.review, self.bootstrap)
        self.assertEqual(result["next_operation"], "reconcile-evidence")
        self.assertEqual(before, self.snapshot())

    def test_interrupted_close_recovery_keeps_unknown_service_write_unreplayed(self):
        value = context.context(self.backend, self.review, self.bootstrap)
        begin(self.run, "fixture-services", "close", value["sources"])
        owner_file = self.dead_lock()
        with patch("devex_clone_run_state.process_identity", side_effect=lambda pid: None if pid == 2147483000 else __import__("full_stack_process").process_identity(pid)), \
                patch.object(lifecycle, "stop_cache") as stop:
            result = lifecycle.recover(self.backend, self.review, self.bootstrap, owner_file, write=True)
        self.assertEqual(result["next_operation"], "reconcile-evidence")
        stop.assert_not_called()
        state = load_state(self.run)
        self.assertEqual(state["attempts"][3]["error_type"], "ControllerInterrupted")
        with self.assertRaisesRegex(ValueError, "未成功收尾"):
            validate_history(self.run, state)

    def test_mid_close_input_drift_stops_before_rustfs_and_preserves_failed_stage(self):
        with self.mocks(), patch.object(lifecycle, "stop_cache") as stop:
            def changed(_request, _environment, _runtime, output):
                self.review.write_text("{}", encoding="utf-8")
                return {"status": "redis_process_stopped"}
            stop.side_effect = changed
            with self.assertRaises(ValueError):
                lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        self.assertEqual(self.events, [])
        self.assertEqual(load_state(self.run)["attempts"][-1]["status"], "failed")

    def test_pid_reuse_during_close_wait_fails_immediately(self):
        expected = {"pid": 12345, "started": "1", "executable": "original"}
        services = {"tree": {"supervisor": expected, "process": expected}}
        with patch.object(lifecycle, "process_identity", return_value={**expected, "started": "2"}), \
                patch.object(lifecycle.time, "sleep") as sleep, self.assertRaisesRegex(ValueError, "PID 已复用"):
            lifecycle._closed_services(services)
        sleep.assert_not_called()

    def test_unknown_recovery_intent_blocks_close_before_service_operations(self):
        write_json(self.run / "recovery-unknown.intent.json", {"unknown": True})
        with self.mocks(), self.assertRaisesRegex(ValueError, "未知控制恢复"):
            lifecycle.close(self.backend, self.review, self.bootstrap, write=True)
        self.assertEqual(self.events, [])

    def test_legacy_receipts_do_not_gain_invented_tree_ownership(self):
        value = context.context(self.backend, self.review, self.bootstrap)
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, "历史首代"):
            context.registered_services(value)
        self.assertEqual(before, self.snapshot())

    def test_frozen_service_requests_tree_and_results_are_bound_together(self):
        from full_stack_process_tree import record_process_tree
        state = load_state(self.run)
        executable = str(self.fixture.tool)
        storage_identity = {"pid": 12345, "started": "1", "executable": executable}
        supervisor = {"pid": 12346, "started": "2", "executable": str(Path(sys.executable).resolve())}
        request = {"scope_id": "services-fixture-seed", "executable": {"sha256": "1" * 64}}
        for role, body, index in (("rustfs", request, 0), ("redis", {}, 1)):
            path = self.run / role / "request.json"
            write_json(path, body)
            state["attempts"][index]["sources"]["request"] = binding(path)
        tree = record_process_tree(self.run / "rustfs", "rustfs", request["scope_id"], supervisor, storage_identity, "a" * 32,
                                   {**supervisor, "pid": 12347})
        write_json(self.run / "controller-0001.json", {"fixture": True})
        observed = {"state": "recorded", "identity": storage_identity,
                    "process_receipt": binding(self.run / "rustfs/process.json"),
                    "launch_receipt": binding(self.run / "rustfs/launch.json")}
        results = [{**observed, "sha256": "1" * 64, "tree": binding(self.run / "rustfs/rustfs-tree.json")},
                   {"service": "redis", "runtime": read_json(self.run / "redis/runtime.json")}]
        for index, result in enumerate(results):
            path = self.run / "results" / f"{index + 1:04d}.json"
            self.fixture.write(path, result)
            state["attempts"][index]["result"] = binding(path)
        self.fixture.write(self.run / "state.json", state)
        value = context.context(self.backend, self.review, self.bootstrap)
        with patch.object(context, "inspect_attempt", return_value=observed) as inspect:
            services = context.registered_services(value)
        self.assertEqual(services["tree"], tree)
        self.assertEqual(inspect.call_args.kwargs["request_binding"], state["attempts"][0]["sources"]["request"])
        self.fixture.write(self.run / "redis/request.json", {"changed": True})
        with self.assertRaises(ValueError):
            context.registered_services(value)

    def test_close_requires_write_before_reading_resources(self):
        with patch.object(lifecycle, "context") as read, self.assertRaisesRegex(ValueError, "--write"):
            lifecycle.close(self.backend, self.review, self.bootstrap, write=False)
        read.assert_not_called()

    def test_cli_rejects_ambiguous_or_write_status_arguments_with_usage_exit(self):
        common = ["--backend-dir", str(self.backend), "--review", str(self.review), "--environment", str(self.bootstrap)]
        for args in (("status", "--write"), ("close",), ("recover", "--write"), ("close", "--write", "--owner-binding", "owner.json")):
            with self.subTest(args=args), patch.object(sys, "argv", ["services", *args, *common]), \
                    patch("sys.stderr", new_callable=io.StringIO), self.assertRaises(SystemExit) as error:
                cli.main()
            self.assertEqual(error.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
