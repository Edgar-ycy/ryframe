"""源运行证据严格绑定同一构建、请求、树和实际创建身份；不启动产品。"""
from contextlib import ExitStack, contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
import unittest
from unittest.mock import patch

import devex_clone_seed_generation_runtime as runtime
import devex_clone_seed_generation_control as control
import test_devex_clone_seed_generation as fixtures
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding


class GenerationRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.f = fixtures.GenerationTests()
        self.f.setUp()
        self.addCleanup(self.f.doCleanups)
        self.output = self.f.output
        self.output.mkdir()
        write_json(self.output / "request.json", self.f.request)
        self.directory = self.output / "runtime"
        self.directory.mkdir()
        self.selected = {"scope_id": "same-source", "api_url": "http://127.0.0.1:18210", "runtime_dir": str(self.directory)}
        self.operations = {"api": "a" * 32, "worker": "b" * 32}
        self.trees, self.identities, artifacts, roles = {}, {}, {}, {}
        for index, role in enumerate(runtime.ROLES):
            executable = self.f.directory / f"{role}.exe"
            executable.write_bytes(role.encode())
            artifacts[role] = {"executable": str(executable), "sha256": binding(executable)["sha256"]}
            identities = {key: {"pid": 100 + index * 10 + offset, "started": "123", "executable": str(executable)}
                          for offset, key in enumerate(("process", "supervisor", "monitor"))}
            self.identities.update({value["pid"]: value for value in identities.values()})
            write_json(self.directory / f"{role}.json", {"format_version": 1, "role": role, "scope_id": "same-source", "identity": identities["process"]})
            tree = {"format_version": 2, "kind": "full-stack-process-tree", "runtime_directory": str(self.directory),
                    "role": role, "scope_id": "same-source", "operation_id": self.operations[role], **identities,
                    "group_id": identities["supervisor"]["pid"]}
            self.trees[role] = tree
            write_json(self.directory / f"{role}-tree.json", tree)
            roles[role] = {"process": binding(self.directory / f"{role}.json"), "tree": binding(self.directory / f"{role}-tree.json"), "ready": True}
        self.build = {"artifacts": artifacts}
        self.contract = {"backend_root": str(self.f.backend), "worker_ready_url": "http://127.0.0.1:19210/readyz",
                         "artifacts": {role: {"path": value["executable"], "sha256": value["sha256"]} for role, value in artifacts.items()}}
        write_json(self.directory / "runtime.json", self.contract)
        write_json(self.output / "intent.json", {"format_version": 1, "kind": "seed-source-runtime-intent",
                   "request": binding(self.output / "request.json"), "runtime_directory": str(self.directory), "operations": self.operations})
        self.evidence = {"format_version": 1, "kind": "seed-source-runtime-running", "intent": binding(self.output / "intent.json"),
                         "runtime": binding(self.directory / "runtime.json"), "roles": roles,
                         "physical_binding": self.f.source["generation"]["physical_binding"]}
        write_json(self.output / "running-evidence.json", self.evidence)

    @contextmanager
    def inputs(self):
        with ExitStack() as stack:
            registered = stack.enter_context(patch.object(runtime, "registered_inputs", return_value=(self.f.backend, self.build)))
            stack.enter_context(patch.object(runtime, "verify_runtime", return_value=self.contract))
            stack.enter_context(patch.object(runtime, "source_binding", return_value=self.evidence["physical_binding"]))
            process = stack.enter_context(patch.object(runtime, "process_identity", side_effect=lambda pid: self.identities.get(pid)))
            listener = stack.enter_context(patch.object(runtime, "verify_listener"))
            ready = stack.enter_context(patch("devex_clone_runtime._ready", return_value=True))
            yield registered, process, listener, ready

    def verify(self, live=False):
        return runtime.verify_running_evidence(self.f.backend, self.output, self.f.request, self.selected,
                                               self.evidence["physical_binding"], live=live)

    def test_static_proof_is_readonly_and_live_proof_checks_all_six_process_identities(self):
        before = {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        with self.inputs() as (registered, process, listener, ready):
            self.assertEqual(self.verify(), self.contract)
            self.assertFalse(registered.call_args.kwargs["reconstruct"])
            process.assert_not_called()
            listener.assert_not_called()
            ready.assert_not_called()
            self.verify(live=True)
            self.assertEqual(process.call_count, 6)
            self.assertEqual(listener.call_count, 2)
            self.assertEqual(ready.call_count, 2)
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()})

    def test_unknown_file_or_extra_process_field_fails_before_live_actions(self):
        rogue = self.directory / "unknown.json"
        rogue.write_text("{}", encoding="utf-8")
        with self.inputs() as (_registered, process, _listener, _ready), self.assertRaises(ValueError):
            self.verify(live=True)
        process.assert_not_called()
        rogue.unlink()
        path = self.directory / "api.json"
        value = read_json(path)
        value["extra"] = "not-a-process-field"
        path.write_text(__import__("json").dumps(value), encoding="utf-8")
        self.evidence["roles"]["api"]["process"] = binding(path)
        (self.output / "running-evidence.json").write_text(__import__("json").dumps(self.evidence), encoding="utf-8")
        with self.inputs(), self.assertRaises(ValueError):
            self.verify()

    def test_pid_reuse_wrong_build_and_failed_ready_cannot_claim_running(self):
        with self.inputs() as (_registered, process, _listener, ready):
            process.side_effect = lambda pid: {**self.identities[pid], "started": "new"}
            with self.assertRaisesRegex(ValueError, "PID 复用"):
                self.verify(live=True)
            process.side_effect = lambda pid: self.identities[pid]
            ready.return_value = False
            with self.assertRaisesRegex(ValueError, "双角色 ready"):
                self.verify(live=True)
            ready.return_value = True
            self.contract["artifacts"]["api"]["sha256"] = "e" * 64
            with self.assertRaisesRegex(ValueError, "产物变化"):
                self.verify()

    def test_status_is_readonly_and_missing_tree_never_means_stopped(self):
        write_json(self.f.directory / "state.json", self.f.state)
        (self.directory / "worker-tree.json").unlink()
        before = {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        with patch.object(control, "load_state", return_value=self.f.state), \
                patch("reference_fixture_successor._source_with_loader", return_value=self.f.source), \
                patch.object(control, "process_identity", side_effect=lambda pid: self.identities.get(pid)), \
                patch.object(control, "completion_binding") as completion:
            value = control.status(self.f.backend, self.f.directory)
            self.assertEqual(value["roles"]["api"]["state"], "running")
            self.assertEqual(value["roles"]["worker"]["state"], "unknown")
            completion.assert_not_called()
            self.identities.pop(self.trees["api"]["monitor"]["pid"])
            self.assertEqual(control.status(self.f.backend, self.f.directory)["roles"]["api"]["state"], "degraded")
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()})

    def test_status_unknown_generation_file_and_source_mismatch_fail_before_process_observation(self):
        write_json(self.f.directory / "state.json", self.f.state)
        rogue = self.output / "unexpected.json"
        rogue.write_text("{}", encoding="utf-8")
        with patch.object(control, "load_state", return_value=self.f.state), \
                patch("reference_fixture_successor._source_with_loader", return_value=self.f.source), \
                patch.object(control, "process_identity") as process:
            with self.assertRaisesRegex(ValueError, "未知"):
                control.status(self.f.backend, self.f.directory)
            rogue.unlink()
            self.f.source["review_successor"]["source_result"] = {"different": "C52"}
            with self.assertRaisesRegex(ValueError, "原 C52"):
                control.status(self.f.backend, self.f.directory)
            process.assert_not_called()

    def test_start_crash_before_intent_publication_is_unknown_without_observing_or_stopping_processes(self):
        write_json(self.f.directory / "state.json", self.f.state)
        (self.output / "intent.json").unlink()
        before = {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        with patch.object(control, "load_state", return_value=self.f.state), \
                patch.object(control, "process_identity") as process, \
                patch.object(control, "existing_runtime") as existing, \
                patch.object(control, "_active", return_value=[]):
            value = control.status(self.f.backend, self.f.directory)
            self.assertIsNone(value["intent"])
            self.assertEqual({row["state"] for row in value["roles"].values()}, {"unknown"})
            with self.assertRaisesRegex(ValueError, "缺少完整归属"):
                control.execute_recover(self.f.backend, self.f.directory, self.output / "request.json", 99)
            process.assert_not_called()
            existing.assert_not_called()
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.output.rglob("*") if path.is_file()})

    def test_quiesced_proof_binds_stopped_time_and_static_verification_has_no_live_calls(self):
        completed = self.f.file("completion.json", {"members": "all stopped"})
        evidence = {**self.evidence, "kind": "seed-source-runtime-quiescence", "ports_idle": list(runtime.ROLES),
                    "observed_stopped_at": datetime.now(timezone.utc).isoformat(),
                    "roles": {role: {**row, "completion": completed} for role, row in self.evidence["roles"].items()}}
        path = self.output / "runtime-evidence.json"
        write_json(path, evidence)
        effective = {"runtime": self.evidence["runtime"], "source": self.selected,
                     "processes": {role: row["process"] for role, row in self.evidence["roles"].items()}}
        before = {str(item): item.read_bytes() for item in self.output.rglob("*") if item.is_file()}
        with self.inputs(), patch.object(runtime, "completion_binding", return_value=completed), \
                patch.object(runtime, "require_closed_port") as ports:
            self.assertEqual(runtime.verify_runtime_evidence(self.f.backend, self.output, self.f.request, effective, live=False), evidence)
            ports.assert_not_called()
            runtime.verify_runtime_evidence(self.f.backend, self.output, self.f.request, effective, live=True)
            self.assertEqual(ports.call_count, 2)
            self.assertEqual(before, {str(item): item.read_bytes() for item in self.output.rglob("*") if item.is_file()})
            evidence["observed_stopped_at"] = (datetime.now(timezone.utc) + timedelta(days=1)).isoformat()
            path.write_text(__import__("json").dumps(evidence), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "未来"):
                runtime.verify_runtime_evidence(self.f.backend, self.output, self.f.request, effective, live=False)

    def test_stop_rechecks_run_lock_before_each_owned_tree_mutation(self):
        instance = runtime.GenerationRuntime.__new__(runtime.GenerationRuntime)
        instance.runtime, instance.operations, instance.directory = self.directory, self.operations, self.f.directory
        with patch("devex_clone_run._require_owned_run", side_effect=ValueError("owner changed")) as owned, \
                patch.object(runtime, "terminate_owned_process_tree") as terminate, \
                patch.object(runtime, "require_closed_port") as ports, self.assertRaisesRegex(ValueError, "owner changed"):
            instance.stop()
        self.assertEqual(owned.call_count, 2)
        terminate.assert_not_called()
        ports.assert_not_called()


if __name__ == "__main__":
    unittest.main()
