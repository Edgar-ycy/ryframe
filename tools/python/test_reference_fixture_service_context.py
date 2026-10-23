"""保留服务上下文、状态观察和私有 CLI 的有效场景，不访问共享服务。"""
from __future__ import annotations

import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import test_reference_fixture_request as request_fixture
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding, begin, finish, load_state
import reference_fixture_service_context as context
from fixture_service_client import validate_history
import reference_fixture_services as cli


class ServiceContextTests(unittest.TestCase):
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
        self.services = {"requests": {"redis": {}},
                         "runtime": read_json(self.run / "redis/runtime.json"), "tree": tree}


    def snapshot(self, exclude=None):
        return {str(path.relative_to(self.root)): path.read_bytes()
                for path in self.root.rglob("*") if path.is_file() and path != exclude}


    def test_native_recovery_preserves_partial_startup_and_interrupted_close_without_replay(self):
        from fixture_service_client import lifecycle
        for mode in ("partial", "interrupted-close", "complete"):
            with self.subTest(mode=mode):
                # 每个场景由独立夹具提供，原生控制器和 Python 底层恢复共同验证文件证据。
                fixture = ServiceContextTests()
                fixture.setUp()
                self.addCleanup(fixture.doCleanups)
                if mode == "partial":
                    state = load_state(fixture.run)
                    for item in state["attempts"][1:]:
                        Path(item["result"]["path"]).unlink()
                    state["attempts"] = state["attempts"][:1]
                    fixture.fixture.write(fixture.run / "state.json", state)
                if mode == "interrupted-close":
                    value = context.context(fixture.backend, fixture.review, fixture.bootstrap)
                    begin(fixture.run, "fixture-services", "close", value["sources"])
                owner = {"format_version": 1, "directory": str(fixture.run),
                         "manifest_sha256": binding(fixture.run / "manifest.json")["sha256"],
                         "identity": {"pid": 2147483000, "started": "1",
                                      "executable": str(Path(sys.executable).resolve())}}
                lock = fixture.run / "run.lock"
                lock.mkdir()
                write_json(lock / "owner.json", owner)
                selected = fixture.root / "owner-binding.json"
                write_json(selected, binding(lock / "owner.json"))
                if mode == "complete":
                    from process_guard import process_guard
                    with process_guard(fixture.run, "run-control.guard"):
                        # Windows 的独占字节锁禁止读取自身标记；其他全部文件仍逐字节核对。
                        guard_file = fixture.run / "run-control.guard"
                        before = fixture.snapshot(exclude=guard_file)
                        with self.assertRaises(ValueError):
                            lifecycle(fixture.backend, "recover", fixture.review, fixture.bootstrap,
                                      selected, write=True)
                        self.assertEqual(before, fixture.snapshot(exclude=guard_file))
                        self.assertEqual(guard_file.stat().st_size, 1)
                result = lifecycle(fixture.backend, "recover", fixture.review, fixture.bootstrap,
                                   selected, write=True)
                self.assertEqual(result["status"], "controller_recovered")
                self.assertFalse(lock.exists())
                self.assertEqual(len(list(fixture.run.glob("recovered-*.json"))), 1)
                if mode == "interrupted-close":
                    self.assertEqual(result["next_operation"], "reconcile-evidence")
                    self.assertEqual(load_state(fixture.run)["attempts"][3]["error_type"], "ControllerInterrupted")
                if mode == "partial":
                    before = fixture.snapshot()
                    self.assertEqual(context.status(fixture.backend, fixture.review, fixture.bootstrap)["next_operation"],
                                     "reconcile-evidence")
                    self.assertEqual(before, fixture.snapshot())

    def closed_history(self):
        value = context.context(self.backend, self.review, self.bootstrap)
        number = begin(self.run, "fixture-services", "close", value["sources"])
        output = self.run / f"lifecycle-{number:04d}"
        output.mkdir()
        tree = self.services["tree"]
        members = self.run / "rustfs" / ("rustfs-members-" + tree["operation_id"] + "-stopped.json")
        write_json(members, {"status": "stopped", "error_type": None, "operation_id": tree["operation_id"],
                             "role": tree["role"], "scope_id": tree["scope_id"], "monitor": tree["monitor"],
                             "supervisor": tree["supervisor"], "directory": tree["runtime_directory"],
                             "members": [tree["supervisor"], tree["monitor"], tree["process"]]})
        write_json(output / "stopped.json", {"status": "redis_process_stopped", "alive": False,
                                            "resources_deleted": False, "runtime": self.services["runtime"]})
        write_json(output / "rustfs-stopped.json", {"state": "stopped", "terminated": True,
                                                   "tree": tree, "completion": binding(members)})
        finish(self.run, number, result={"format_version": 1, "kind": "reference-fixture-service-lifecycle",
               "operation": "close", "status": "services_closed", "run": str(self.run),
               "remote_writes": 0, "resources_deleted": False,
               "services": {"redis": "stopped", "rustfs": "stopped"},
               "evidence": {"redis": binding(output / "stopped.json"),
                            "rustfs": binding(output / "rustfs-stopped.json")}})
        self.assertTrue(validate_history(self.run, load_state(self.run))["closed"])


    def test_new_requests_reject_closed_service_generations(self):
        self.closed_history()
        from reference_fixture_request import build
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, "关闭"):
            build(self.backend, self.bootstrap, self.run, "fresh-seed", "seed")
        self.assertEqual(before, self.snapshot())



    def test_context_and_transition_bridges_only_pass_native_protocol(self):
        value = {"backend": str(self.backend), "run": str(self.run), "private": {"APP_ENV": "test"}}
        with patch.object(context, "_invoke", return_value=value) as invoke:
            result = context.context(self.backend, self.review, self.bootstrap)
        self.assertEqual(result["run"], self.run)
        invoke.assert_called_once_with("service-context", {"review": self.review, "environment": self.bootstrap}, backend=self.backend)
        with patch.object(context, "_invoke", return_value=[{"generation": "bound"}, None]) as invoke:
            result = context.runtime_transition(self.backend, self.run, {"storage": "expected"})
        self.assertEqual(result, ({"generation": "bound"}, None))
        invoke.assert_called_once_with("service-transition", {"run": self.run, "expected": {"storage": "expected"}}, backend=self.backend)

    def test_status_bridge_uses_read_only_native_execution(self):
        with patch.object(context, "lifecycle", return_value={"status": "service_status"}) as invoke:
            self.assertEqual(context.status(self.backend, self.review, self.bootstrap), {"status": "service_status"})
        invoke.assert_called_once_with(self.backend, "status", self.review, self.bootstrap, None, write=False)

    def test_successor_tools_observe_predecessor(self):
        original = {"wsl": {"path": "old"}, "python": {"path": "python"}}
        effective = {"wsl": {"path": "current"}, "python": {"path": "python"}}
        services = {**self.services, "requests": {"redis": original, "rustfs": {
            "api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201"}},
            "cache_effective": effective}
        services["runtime"] = {**services["runtime"],
            "linux_identity": {"pid": 9001, "started": "1", "executable": "/usr/bin/redis-server",
                               "boot_id": "11111111-1111-1111-1111-111111111111"},
            "boot_id": "11111111-1111-1111-1111-111111111111",
            "redis": {"run_id": "a" * 40}}
        with patch("devex_clone_cache_process.status") as status, \
                patch("devex_clone_cache_process.predecessor_status",
                      return_value={"state": "stopped"}) as predecessor, \
                patch.object(context, "_identity_observations",
                             return_value={"supervisor": "running", "monitor": "running", "process": "running"}), \
                patch.object(context, "arguments", return_value=[]), \
                patch.object(context, "running", return_value=True):
            observed = context.observe_services(services)
        status.assert_not_called()
        predecessor.assert_called_once_with(original, services["runtime"], effective)
        self.assertEqual(observed["redis"], "stopped")










    def test_external_termination_requires_every_identity_missing_and_redis_stopped(self):
        request = {"api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201"}
        services = {**self.services, "requests": {"rustfs": request, "redis": {}}, "evidence": {}}
        identities = {role: "missing" for role in ("supervisor", "monitor", "process")}
        ready = {"format_version": 1, "operation_id": self.services["tree"]["operation_id"],
                 "directory": self.services["tree"]["runtime_directory"], "role": "rustfs",
                 "scope_id": self.services["tree"]["scope_id"],
                 "supervisor": self.services["tree"]["supervisor"], "monitor": self.services["tree"]["monitor"]}
        with patch.object(context, "read_json", return_value=ready), \
                patch.object(context, "binding", return_value={"path": "ready"}), \
                patch.object(context, "require_closed_port") as closed, \
                patch.object(context, "_identity_observations", return_value=identities):
            result = context._external_termination(services, identities)
        self.assertEqual(result["status"], "external-termination-unreconciled")
        self.assertEqual(closed.call_count, 2)

        with patch("devex_clone_cache_process.status", return_value={"state": "running"}), \
                patch.object(context, "_identity_observations", return_value=identities), \
                patch("full_stack_process_monitor.wait_members", side_effect=ValueError(
                    "成员监督器退出但没有完整成员关闭证明")), \
                patch.object(context, "_external_termination", return_value=result), \
                self.assertRaisesRegex(ValueError, "Redis 仍存活"):
            context.observe_services(services)


    def test_service_observation_treats_reused_identity_as_recorded_exit(self):
        reused = {**self.services["tree"]["monitor"], "started": "2"}
        with patch.object(context, "process_identity", side_effect=lambda pid: reused
                          if pid == self.services["tree"]["monitor"]["pid"] else None):
            observations = context._identity_observations(self.services["tree"])
        self.assertEqual(observations, {role: "missing" for role in ("supervisor", "monitor", "process")})
        with patch("devex_clone_cache_process.status", return_value={"state": "stopped"}), \
                patch.object(context, "_identity_observations", return_value={
                    "supervisor": "missing", "monitor": "missing", "process": "running"}), \
                self.assertRaisesRegex(ValueError, "部分存活"):
            context.observe_services({**self.services, "requests": {"redis": {}, "rustfs": {}}, "evidence": {}})


    def test_identity_observation_retries_transient_permission_denial_but_not_persistent_denial(self):
        with patch.object(context, "process_identity",
                          side_effect=[PermissionError(), None, None, None]), \
                patch.object(context.time, "sleep") as sleep:
            self.assertEqual(context._identity_observations(self.services["tree"]), {
                role: "missing" for role in ("supervisor", "monitor", "process")})
        sleep.assert_called_once_with(0.05)
        with patch.object(context, "process_identity", side_effect=PermissionError), \
                patch.object(context.time, "monotonic", side_effect=[0, 6]), \
                patch.object(context.time, "sleep") as sleep, self.assertRaises(PermissionError):
            context._identity_observations(self.services["tree"])
        sleep.assert_not_called()


    def test_identity_observation_permission_retry_uses_one_shared_deadline(self):
        with patch.object(context, "process_identity",
                          side_effect=[PermissionError(), None, PermissionError()]), \
                patch.object(context.time, "monotonic", side_effect=[0, 1, 6]), \
                patch.object(context.time, "sleep") as sleep, self.assertRaises(PermissionError):
            context._identity_observations(self.services["tree"])
        sleep.assert_called_once_with(0.05)




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
        data = self.run / "rustfs-data"
        data.mkdir()
        request = {"scope_id": "services-fixture-seed", "executable": {"sha256": "1" * 64},
                   "data_directory": {"path": str(data), "device": data.stat().st_dev, "inode": data.stat().st_ino}}
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


    def test_cli_rejects_ambiguous_or_write_status_arguments_with_usage_exit(self):
        common = ["--backend-dir", str(self.backend), "--review", str(self.review), "--environment", str(self.bootstrap)]
        for args in (("status", "--write"), ("close",), ("reconcile", "--write"),
                     ("recover", "--write"), ("close", "--write", "--owner-binding", "owner.json")):
            with self.subTest(args=args), patch("sys.stderr", new_callable=io.StringIO), \
                    self.assertRaises(SystemExit) as error:
                cli.main([*args, *common])
            self.assertEqual(error.exception.code, 2)


    def test_cli_dispatches_reconcile_with_explicit_owner_binding(self):
        expected = {"status": "failed_close_reconciled", "remote_writes": 0}
        arguments = ["reconcile", "--backend-dir", str(self.backend),
                     "--review", str(self.review), "--environment", str(self.bootstrap),
                     "--owner-binding", str(self.run / "state.json"), "--write"]
        with patch("fixture_service_client.lifecycle", return_value=expected) as reconcile, \
                patch("builtins.print") as output:
            cli.main(arguments)
        reconcile.assert_called_once_with(self.backend.resolve(), "reconcile", self.review, self.bootstrap,
                                          self.run / "state.json", write=True)
        output.assert_called_once_with(json.dumps(expected, ensure_ascii=False))


    def test_cli_rejects_duplicate_and_abbreviated_options_before_reading_inputs(self):
        common = ["--backend-dir", str(self.backend), "--review", str(self.review), "--environment", str(self.bootstrap)]
        before = self.snapshot()
        invalid = [("status", "--review", "other"), ("status", "--environment=other"),
                   ("close", "--write", "--write"), ("status", "--rev", "other")]
        for args in invalid:
            with self.subTest(args=args), patch("sys.stderr", new_callable=io.StringIO), \
                    patch.object(cli, "document") as read, \
                    self.assertRaises(SystemExit) as error:
                cli.main([*args, *common])
            self.assertEqual(error.exception.code, 2)
            read.assert_not_called()
            self.assertEqual(before, self.snapshot())



if __name__ == "__main__":
    unittest.main()
