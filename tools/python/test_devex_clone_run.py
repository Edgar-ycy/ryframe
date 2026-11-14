"""统一入口的阶段隔离、失败续跑与来源证据；不连接外部服务。"""
from contextlib import contextmanager, nullcontext
import json
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory
import devex_clone_run as run
import devex_clone_run_state as state
from devex_clone_capture import read_json, write_json


class RunTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(base, "devex-clone-run-")
        self.addCleanup(temporary.cleanup)
        self.backend = temporary.path
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.directory = self.local / "run"
        self.value = {"format_version": 1, "kind": "devex-clone-run", "id": "copy-case",
                      "source_request": self.file("request", {"kind": "fixture"}),
                      "initialized": self.file("initialized", {"kind": "fixture"}),
                      "source_environment": self.file("source-env", {"environment": {"SIDE": "source"}}),
                      "target_environment": self.file("target-env", {"environment": {"SIDE": "target"}}),
                      "copy_directory": str(self.local / "copy"), "copy_stage": "source_to_seed", "build_bridges": []}
        self.value["source_export"] = self.file("export", {"request": self.value["source_request"]})
        self.directory.mkdir()
        write_json(self.directory / "manifest.json", self.value)
        state.initialize_state(self.directory)
        self.sources = {"product": "unchanged", "test_tools": "tool-a", "complete": "all-a"}
        self.addCleanup(patch.stopall)
        patch("source_fingerprints.current_execution_source", return_value=self.sources).start()
        patch("source_fingerprints.artifact_sources", return_value=nullcontext()).start()
        patch.object(run, "copy_binary_bindings", return_value=[]).start()
        patch.object(run, "protect_binaries", return_value=nullcontext()).start()
        patch("devex_clone_resume.check_continuation").start()

    def file(self, name, value):
        path = self.local / (name + ".json")
        write_json(path, value)
        return state.binding(path)

    def test_fixture_storage_run_forwards_owned_lock_identity(self):
        target = self.local / "candidate/target"
        target.mkdir(parents=True)
        write_json(target.parent / "registration.json", {"storage_run": {}})
        initialized = target / "initialized.json"
        write_json(initialized, {"kind": "fixture"})
        observed = []

        def history(_backend, path, identity=None):
            observed.append((Path(path).name, identity))
            return ({}, {})

        with patch.object(run, "initialization_history", side_effect=history):
            result = run.target_fixture_storage_run(self.backend, state.binding(initialized),
                                                    owned_lock_identity=7)
        self.assertIsNone(result)
        self.assertEqual(observed, [("initialized.json", 7)])

    def test_failed_copy_preserves_history_and_next_stage_attempt_reuses_run(self):
        with patch.object(run, "run_copy", side_effect=ValueError("fixture")):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "copy", "run")
        self.assertFalse((self.directory / "run.lock").exists())
        with patch.object(run, "run_copy", return_value={"status": "copy_completed", "remote_writes": 0}):
            result = run.execute(self.backend, self.directory, "copy", "run")
        self.assertEqual(result["attempt"], 2)
        observed = state.load_state(self.directory)
        self.assertEqual([a["status"] for a in observed["attempts"]], ["failed", "passed"])
        self.assertEqual(observed["attempts"][0]["error_type"], "ValueError")
        failure = read_json(self.directory / "failure-0001.json")
        self.assertEqual(failure["error_type"], "ValueError")
        self.assertEqual(failure["controller"], state.binding(self.directory / "controller-0001.json"))
        self.assertNotIn("fixture", json.dumps(failure))

    def test_failure_diagnostic_omits_exception_text_source_and_locals(self):
        filename = self.backend / "tools/python/failure_fixture.py"
        filename.parent.mkdir(parents=True)
        namespace = {}
        exec(compile("def fail():\n    password = 'fixture-secret'\n    raise ValueError(password)\n",
                     str(filename), "exec"), namespace)
        with patch.object(run, "run_copy", side_effect=lambda *args: namespace["fail"]()):
            with self.assertRaisesRegex(ValueError, "fixture-secret"):
                run.execute(self.backend, self.directory, "copy", "run")
        failure = read_json(self.directory / "failure-0001.json")
        self.assertEqual(failure["frames"], [{"file": "tools/python/failure_fixture.py", "function": "fail", "line": 3}])
        self.assertNotIn("fixture-secret", json.dumps(failure))
        self.assertNotIn(str(self.backend), json.dumps(failure["frames"]))

    def test_diagnostic_write_failure_keeps_original_error_and_failed_stage(self):
        with patch.object(run, "run_copy", side_effect=ValueError("original")), \
                patch.object(run, "record_failure", side_effect=OSError("diagnostic")):
            with self.assertRaisesRegex(ValueError, "original") as raised:
                run.execute(self.backend, self.directory, "copy", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][-1]["status"], "failed")
        self.assertEqual(raised.exception.__notes__, ["阶段位置证据保存失败：OSError"])

    def test_changed_input_is_rejected_before_any_execution(self):
        Path(self.value["source_environment"]["path"]).write_text('{"environment":{}}', encoding="utf-8")
        with patch.object(run, "run_copy") as operation, self.assertRaises(ValueError):
            run.execute(self.backend, self.directory, "copy", "resume")
        operation.assert_not_called()
        self.assertEqual(state.load_state(self.directory)["attempts"], [])

    def test_generation_request_is_rejected_before_attempt_or_controller_is_created(self):
        request = self.local / "generation-request.json"
        write_json(request, {"kind": "wrong-generation-request"})
        before = state.binding(self.directory / "state.json")
        with patch("devex_clone_seed_generation.preflight",
                   side_effect=ValueError("request binding changed")) as preflight, \
                patch("devex_clone_seed_runtime.execute_seed") as execute, \
                self.assertRaisesRegex(ValueError, "request binding changed"):
            run.execute(
                self.backend,
                self.directory,
                "seed-runtime",
                "source-generation-start",
                seed_request=request,
            )
        preflight.assert_called_once_with(
            self.directory, backend=self.backend, request_path=request
        )
        execute.assert_not_called()
        self.assertEqual(state.load_state(self.directory)["attempts"], [])
        self.assertFalse((self.directory / "run.lock").exists())
        self.assertFalse(any(self.directory.glob("controller-*.json")))
        self.assertFalse(any(self.directory.glob("failure-*.json")))
        self.assertFalse(any(self.directory.glob("g[0-9][0-9][0-9][0-9]")))
        self.assertEqual(before, state.binding(self.directory / "state.json"))

    def test_existing_copy_never_restarts_factory(self):
        output = Path(self.value["copy_directory"])
        output.mkdir()
        write_json(output / "session.json", {"source_export": self.value["source_export"],
                   "initialized": self.value["initialized"]})
        environment = run.environments(self.backend, self.value)
        with patch("devex_clone_factory.copy_to_fresh_target") as first:
            with self.assertRaises(ValueError):
                run.run_copy(self.backend, self.directory, self.value, environment, "run")
            first.assert_not_called()
        with patch("devex_clone_resume.continue_copy", return_value={"status": "copy_reconciled"}) as continuation:
            run.run_copy(self.backend, self.directory, self.value, environment, "reconcile")
        self.assertEqual(continuation.call_args.kwargs, {
            "mode": "reconcile", "source_storage_run": self.directory,
            "target_storage_run": self.directory,
        })

    def test_copy_binary_protection_failure_prevents_dispatch_and_success_publication(self):
        output = Path(self.value["copy_directory"])
        output.mkdir()
        write_json(output / "session.json", {"source_export": self.value["source_export"],
                   "initialized": self.value["initialized"]})
        with patch.object(run, "protect_binaries", side_effect=OSError("protection unavailable")), \
                patch("devex_clone_resume.continue_copy") as continuation:
            with self.assertRaises(OSError):
                run.execute(self.backend, self.directory, "copy", "reconcile")
            continuation.assert_not_called()
        @contextmanager
        def failed_release(_bindings):
            yield
            raise OSError("protection release failed")
        with patch.object(run, "protect_binaries", side_effect=failed_release), \
                patch("devex_clone_resume.continue_copy", return_value={"status": "copy_reconciled"}):
            with self.assertRaisesRegex(OSError, "release failed"):
                run.execute(self.backend, self.directory, "copy", "reconcile")
        observed = state.load_state(self.directory)
        self.assertEqual([item["status"] for item in observed["attempts"]], ["failed", "failed"])
        self.assertTrue(all(item["result"] is None for item in observed["attempts"]))
        self.assertFalse((self.directory / "run.lock").exists())

    def test_unknown_write_or_residual_lock_prevents_binary_collection(self):
        from devex_clone_ledger import LedgerError

        output = Path(self.value["copy_directory"])
        output.mkdir()
        write_json(output / "session.json", {"source_export": self.value["source_export"],
                   "initialized": self.value["initialized"]})
        for message in ("unknown write", "residual lock"):
            with patch("devex_clone_resume.check_continuation", side_effect=LedgerError(message)), \
                    patch.object(run, "copy_binary_bindings") as collect, \
                    patch.object(run, "protect_binaries") as protect, \
                    patch("devex_clone_resume.continue_copy") as continuation:
                with self.assertRaises(LedgerError):
                    run.run_copy(self.backend, self.directory, self.value, run.environments(self.backend, self.value), "resume")
                collect.assert_not_called()
                protect.assert_not_called()
                continuation.assert_not_called()

    def test_retired_runtime_is_rejected_before_paths_or_locks(self):
        original = {path: path.read_bytes() for path in self.directory.iterdir() if path.is_file()}
        with patch.object(run, "process_guard") as guard:
            for side in ("source", "target"):
                for operation in ("start", "stop", "status", "recover"):
                    with self.subTest(side=side, operation=operation):
                        with self.assertRaisesRegex(ValueError, "Rust 正式入口"):
                            run.execute(self.backend, self.directory, "runtime-" + side, operation)
        guard.assert_not_called()
        self.assertEqual({path: path.read_bytes() for path in self.directory.iterdir() if path.is_file()}, original)

    def test_session_recovery_uses_same_run_journal_without_source_or_environment(self):
        for field in ('source_export', 'source_environment', 'target_environment'):
            Path(self.value[field]['path']).unlink()
        descriptor = {'path': 'explicit-only', 'bytes': 1, 'sha256': 'a' * 64}
        with patch('source_fingerprints.current_execution_source', side_effect=OSError('source unavailable')), \
                patch('devex_clone_post.execute_post', return_value={'status': 'post_session_recovered'}) as recover:
            result = run.execute(self.backend, self.directory, 'post-copy', 'recover-session', producer_binding=descriptor)
        self.assertEqual(result['status'], 'stage_finished')
        self.assertEqual(recover.call_args.args[-1], descriptor)
        self.assertEqual(state.load_state(self.directory)['attempts'][0]['mode'], 'recover-session')

    def test_retired_arm_input_cannot_create_a_python_attempt(self):
        request = self.local / "arm-request.json"
        write_json(request, {"kind": "fixture-arm-input"})
        original = (self.directory / "state.json").read_bytes()
        with patch("devex_clone_seed_runtime.execute_seed") as publish, \
                self.assertRaisesRegex(ValueError, "Rust 正式入口"):
            run.execute(self.backend, self.directory, "seed-runtime", "arm-input", seed_request=request)
        publish.assert_not_called()
        self.assertEqual((self.directory / "state.json").read_bytes(), original)

    def test_target_start_requires_complete_copy_and_holds_worker_for_schedule_actions(self):
        output = Path(self.value["copy_directory"])
        output.mkdir()
        saved = {"initialized": self.value["initialized"], "source_export": self.value["source_export"],
                 "plan_sha256": "a" * 64, "generation_sha256": "b" * 64}
        result = {"status": "data_steps_verified", "plan_sha256": saved["plan_sha256"],
                  "generation_sha256": saved["generation_sha256"], "written_steps": 1,
                  "source_export_sha256": self.value["source_export"]["sha256"],
                  "fresh_target_sha256": self.value["initialized"]["sha256"], "pending_target_actions": [{"id": "schedule"}]}
        write_json(output / "session.json", saved)
        write_json(output / "result.json", result)
        number = state.begin(self.directory, "copy", "resume", self.sources)
        state.finish(self.directory, number, result=result)
        with patch("devex_clone_ledger.CloneLedger") as ledger, \
                patch("devex_clone.verify_plan_result", return_value=SimpleNamespace(plan={"plan_sha256": saved["plan_sha256"], "pending_target_actions": result["pending_target_actions"]})):
            ledger.return_value.inspect.return_value = {"status": "needs_reconciliation", "steps": {"one": {}}}
            with self.assertRaises(ValueError):
                run.require_target_copy(self.backend, self.directory, self.value, ("api",))
            ledger.return_value.inspect.return_value["status"] = "ledger_evidence_complete"
            run.require_target_copy(self.backend, self.directory, self.value, ("api",))
            with self.assertRaises(ValueError):
                run.require_target_copy(self.backend, self.directory, self.value, ("api", "worker"))

    def test_partial_ledger_only_allows_a_complete_zero_write_copy(self):
        output = Path(self.value["copy_directory"])
        output.mkdir()
        saved = {"initialized": self.value["initialized"], "source_export": self.value["source_export"],
                 "plan_sha256": "a" * 64, "generation_sha256": "b" * 64}
        item = {"key": "shared-control"}
        database_id = "database-075921cce671fc36b9360281d8b631a2"
        result = {"status": "data_steps_verified", "plan_sha256": saved["plan_sha256"],
                  "generation_sha256": saved["generation_sha256"], "written_steps": 0,
                  "source_export_sha256": self.value["source_export"]["sha256"],
                  "fresh_target_sha256": self.value["initialized"]["sha256"], "pending_target_actions": [],
                  "unchanged_databases": 1, "unchanged_database_images": {database_id: {"observed": True}}}
        write_json(output / "session.json", saved)
        write_json(output / "result.json", result)
        number = state.begin(self.directory, "copy", "resume", self.sources)
        state.finish(self.directory, number, result=result)
        plan = {"plan_sha256": saved["plan_sha256"], "pending_target_actions": [], "objects": [], "databases": [item]}
        with patch("devex_clone_ledger.CloneLedger") as ledger, patch("devex_clone.verify_plan_result", return_value=SimpleNamespace(plan=plan)):
            ledger.return_value.inspect.return_value = {"status": "partial", "steps": {}}
            run.require_target_copy(self.backend, self.directory, self.value, ("api", "worker"))
            plan["databases"] = [{"key": "different"}]
            with self.assertRaises(ValueError):
                run.require_target_copy(self.backend, self.directory, self.value, ("api",))
            plan["databases"] = [item]
            plan["objects"] = [{"pending": True}]
            with self.assertRaises(ValueError):
                run.require_target_copy(self.backend, self.directory, self.value, ("api",))

    def test_mismatch_is_recorded_as_failed_with_reconciliation_evidence(self):
        with patch.object(run, "run_copy", return_value={"status": "needs_reconciliation", "remote_writes": 0}):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "copy", "reconcile")
        attempt = state.load_state(self.directory)["attempts"][0]
        self.assertEqual(attempt["status"], "failed")
        self.assertEqual(json.loads(Path(attempt["result"]["path"]).read_text())["status"], "needs_reconciliation")

    def test_changed_source_during_stage_cannot_publish_success(self):
        with patch("source_fingerprints.current_execution_source", side_effect=[self.sources, {"changed": True}]), \
                patch.object(run, "run_copy", return_value={"status": "copy_completed"}):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "copy", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")

    def test_final_source_check_failure_preserves_result_without_publishing_success(self):
        @contextmanager
        def source_context(*_args):
            yield
            raise ValueError("final source check")

        with patch("source_fingerprints.artifact_sources", side_effect=source_context), \
                patch.object(run, "run_copy", return_value={"status": "copy_completed"}):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "copy", "run")
        attempt = state.load_state(self.directory)["attempts"][0]
        self.assertEqual(attempt["status"], "failed")
        self.assertIsNotNone(attempt["result"])
        self.assertFalse((self.directory / "run.lock").exists())

    def test_lock_release_failure_cannot_publish_success(self):
        @contextmanager
        def failing_lock(directory):
            with state.claim_run_lock(directory) as owner:
                yield owner
            raise OSError("lock release")

        with patch.object(run, "claim_run_lock", side_effect=failing_lock), \
                patch.object(run, "run_copy", return_value={"status": "copy_completed"}):
            with self.assertRaises(OSError):
                run.execute(self.backend, self.directory, "copy", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")

    def test_failed_lock_acquisition_does_not_finish_another_attempt(self):
        state.begin(self.directory, "copy", "resume", self.sources)
        with state.run_lock(self.directory), self.assertRaises(ValueError):
            run.execute(self.backend, self.directory, "copy", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "running")

    def test_concurrent_control_rejected_and_result_tampering_detected(self):
        with state.run_lock(self.directory), self.assertRaises(ValueError):
            run.execute(self.backend, self.directory, "copy", "run")
        with patch.object(run, "run_copy", return_value={"status": "copy_completed"}):
            run.execute(self.backend, self.directory, "copy", "run")
        (self.directory / "results/0001.json").write_text('{}', encoding="utf-8")
        with self.assertRaises(ValueError):
            state.load_state(self.directory)

    def test_recovery_requires_dead_exact_controller_and_keeps_failed_attempt(self):
        state.begin(self.directory, "copy", "resume", self.sources)
        lock = self.directory / "run.lock"
        lock.mkdir()
        owner = {"format_version": 1, "identity": {"pid": 10001, "started": "123", "executable": "fixture"},
                 "directory": str(self.directory), "manifest_sha256": state.binding(self.directory / "manifest.json")["sha256"]}
        write_json(lock / "owner.json", owner)
        bound = state.binding(lock / "owner.json")
        with patch.object(state, "process_identity", return_value=owner["identity"]), self.assertRaises(ValueError):
            state.recover_lock(self.backend, self.directory, bound)
        with patch.object(state, "process_identity", return_value={**owner["identity"], "started": "456"}), self.assertRaises(ValueError):
            state.recover_lock(self.backend, self.directory, bound)
        with patch.object(state, "process_identity", return_value=None):
            report = state.recover_lock(self.backend, self.directory, bound)
        self.assertTrue(report["copy_requires_reconciliation"])
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["error_type"], "ControllerInterrupted")
        self.assertFalse(lock.exists())


if __name__ == "__main__":
    unittest.main()
