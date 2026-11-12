"""保留服务上下文、状态观察和私有 CLI 的有效场景，不访问共享服务。"""
from __future__ import annotations

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
                # 每个场景由独立夹具提供，Python 写入历史与原生控制器恢复共同验证文件证据。
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
                    self.assertEqual(lifecycle(fixture.backend, "status", fixture.review, fixture.bootstrap,
                                               None, write=False)["next_operation"],
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

    def test_legacy_receipts_do_not_gain_invented_tree_ownership(self):
        value = context.context(self.backend, self.review, self.bootstrap)
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, "历史首代"):
            context.registered_services(value)
        self.assertEqual(before, self.snapshot())


    def test_registration_and_observation_bridges_do_not_interpret_native_result(self):
        value = {"backend": self.backend, "run": self.run}
        expected = {"origin": "frozen"}
        with patch.object(context, "_invoke", return_value=expected) as invoke:
            self.assertIs(context.registered_services(value, settled=False), expected)
        invoke.assert_called_once_with("service-registered", {"value": value, "settled": False}, backend=self.backend)
        with patch.object(context, "_invoke", return_value=expected) as invoke:
            self.assertIs(context.observe_services({"frozen": True}), expected)
        invoke.assert_called_once_with("service-observe", {"services": {"frozen": True}})


if __name__ == "__main__":
    unittest.main()
