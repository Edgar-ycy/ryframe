"""统一入口的阶段隔离、失败续跑与来源证据；不连接外部服务。"""
from contextlib import contextmanager, nullcontext
import json
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
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
        manifest = self.file("manifest-input", self.value)
        run.initialize(self.backend, Path(manifest["path"]), self.directory)
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

    def test_failed_export_preserves_history_and_next_stage_attempt_reuses_run(self):
        with patch.object(run, "run_export", side_effect=ValueError("fixture")):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "export", "run")
        self.assertFalse((self.directory / "run.lock").exists())
        with patch.object(run, "run_export", return_value={"status": "export_verified", "remote_writes": 0}):
            result = run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(result["attempt"], 2)
        observed = state.load_state(self.directory)
        self.assertEqual([a["status"] for a in observed["attempts"]], ["failed", "passed"])
        self.assertEqual(observed["attempts"][0]["error_type"], "ValueError")
        failure = read_json(self.directory / "failure-0001.json")
        self.assertEqual(failure["error_type"], "ValueError")
        self.assertEqual(failure["controller"], state.binding(self.directory / "controller-0001.json"))
        self.assertNotIn("fixture", json.dumps(failure))

    def test_failure_diagnostic_omits_exception_text_source_and_locals(self):
        filename = self.backend / "scripts/failure_fixture.py"
        namespace = {}
        exec(compile("def fail():\n    password = 'fixture-secret'\n    raise ValueError(password)\n",
                     str(filename), "exec"), namespace)
        with patch.object(run, "run_export", side_effect=lambda *args: namespace["fail"]()):
            with self.assertRaisesRegex(ValueError, "fixture-secret"):
                run.execute(self.backend, self.directory, "export", "run")
        failure = read_json(self.directory / "failure-0001.json")
        self.assertEqual(failure["frames"], [{"file": "scripts/failure_fixture.py", "function": "fail", "line": 3}])
        self.assertNotIn("fixture-secret", json.dumps(failure))
        self.assertNotIn(str(self.backend), json.dumps(failure["frames"]))

    def test_diagnostic_write_failure_keeps_original_error_and_failed_stage(self):
        with patch.object(run, "run_export", side_effect=ValueError("original")), \
                patch.object(run, "record_failure", side_effect=OSError("diagnostic")):
            with self.assertRaisesRegex(ValueError, "original") as raised:
                run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][-1]["status"], "failed")
        self.assertEqual(raised.exception.__notes__, ["阶段位置证据保存失败：OSError"])

    def test_source_storage_failure_blocks_export_before_existing_proof_is_read(self):
        with patch("devex_clone_storage.current_storage_binding", side_effect=ValueError("latest storage stopped")) as storage, \
                patch("devex_clone_export_verify.verify_source_export") as verify, \
                patch("devex_clone_source.export_source") as export:
            with self.assertRaisesRegex(ValueError, "storage stopped"):
                run.execute(self.backend, self.directory, "export", "run")
        storage.assert_called_once_with(self.backend, self.directory, "source")
        verify.assert_not_called()
        export.assert_not_called()
        self.assertEqual(state.load_state(self.directory)["attempts"][-1]["status"], "failed")

    def test_source_storage_failure_after_export_verification_prevents_publication(self):
        verified = {"export": {"logical_inventory_sha256": "a" * 64}}
        original = {key: Path(self.value[key]["path"]).read_bytes() for key in ("source_request", "source_export", "initialized")}
        with patch("devex_clone_storage.current_storage_binding", side_effect=[None, ValueError("latest storage failed")]) as storage, \
                patch("devex_clone_export_verify.verify_source_export", return_value=verified) as verify:
            with self.assertRaisesRegex(ValueError, "storage failed"):
                run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(storage.call_count, 2)
        verify.assert_called_once_with(self.backend, self.value["source_export"])
        latest = state.load_state(self.directory)["attempts"][-1]
        self.assertEqual(latest["status"], "failed")
        self.assertIsNone(latest["result"])
        self.assertEqual({key: Path(self.value[key]["path"]).read_bytes() for key in original}, original)

    def test_source_without_storage_registration_retains_existing_export_contract(self):
        verified = {"export": {"logical_inventory_sha256": "a" * 64}}
        with patch("devex_clone_storage.current_storage_binding", return_value=None) as storage, \
                patch("devex_clone_export_verify.verify_source_export", return_value=verified):
            result = run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(result["result"], {"status": "export_verified", "export": self.value["source_export"],
            "logical_inventory_sha256": "a" * 64, "remote_writes": 0, "restore_qualified": False})
        self.assertEqual(storage.call_count, 2)
        self.assertTrue(all(item.args == (self.backend, self.directory, "source") for item in storage.call_args_list))
        self.assertEqual(state.load_state(self.directory)["attempts"][-1]["status"], "passed")

    def test_changed_input_is_rejected_before_any_execution(self):
        Path(self.value["source_environment"]["path"]).write_text('{"environment":{}}', encoding="utf-8")
        with patch.object(run, "run_copy") as operation, self.assertRaises(ValueError):
            run.execute(self.backend, self.directory, "copy", "resume")
        operation.assert_not_called()
        self.assertEqual(state.load_state(self.directory)["attempts"], [])

    def test_storage_restart_preflight_failure_does_not_create_attempt_or_lock(self):
        request = self.local / "storage-request.json"
        before = {str(path.relative_to(self.directory)): path.read_bytes()
                  for path in self.directory.rglob("*") if path.is_file()}
        with patch("devex_clone_storage.preflight_restart", side_effect=PermissionError("protected successor")) as preflight, \
                patch("devex_clone_storage.execute_storage") as execute, \
                self.assertRaisesRegex(PermissionError, "protected successor"):
            run.execute(self.backend, self.directory, "storage-target", "restart",
                        storage_request=request)
        preflight.assert_called_once_with(self.backend, self.directory, self.value, "target", request)
        execute.assert_not_called()
        self.assertEqual(state.load_state(self.directory)["attempts"], [])
        self.assertFalse((self.directory / "run.lock").exists())
        self.assertFalse(any(self.directory.glob("controller-*.json")))
        self.assertEqual(before, {str(path.relative_to(self.directory)): path.read_bytes()
                                  for path in self.directory.rglob("*") if path.is_file()})

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

    def test_cache_action_uses_registered_controller_and_fixed_request(self):
        request = self.local / "cache-input.json"
        def action(backend, directory, value, mode, number, supplied):
            self.assertEqual((backend, directory, value, mode, number, supplied),
                             (self.backend, self.directory, self.value, "restart", 1, request))
            self.assertEqual(state.load_state(directory)["attempts"][-1]["stage"], "cache-target")
            self.assertTrue((directory / "controller-0001.json").is_file())
            self.assertTrue((directory / "run.lock/owner.json").is_file())
            return {"status": "cache_ready"}
        with patch("devex_clone_cache.execute_cache", side_effect=action):
            result = run.execute(self.backend, self.directory, "cache-target", "restart", cache_request=request)
        self.assertEqual(result["attempt"], 1)
        self.assertEqual(state.load_state(self.directory)["attempts"][-1]["status"], "passed")

    def test_cache_cleanup_uses_saved_manifest_when_product_or_private_environment_changed(self):
        Path(self.value["target_environment"]["path"]).write_text('{}', encoding="utf-8")
        with patch("source_fingerprints.artifact_sources", side_effect=ValueError("changed build")), \
                patch("devex_clone_cache.execute_cache", return_value={"status": "cache_stopped"}) as stop:
            result = run.execute(self.backend, self.directory, "cache-target", "stop")
        self.assertEqual(result["result"]["status"], "cache_stopped")
        stop.assert_called_once_with(self.backend, self.directory, self.value, "stop", 1, None)

    def test_cache_cli_status_is_readonly_and_mutations_require_explicit_write(self):
        import devex_clone_run_cli as cli

        args = SimpleNamespace(command="cache", run_dir=self.directory, operation="status", request=None, write=False)
        before = state.binding(self.directory / "state.json")
        with patch("devex_clone_cache.cache_status", return_value={"status": "cache_unregistered"}) as status, \
                patch.object(cli, "execute") as execute:
            self.assertEqual(cli.dispatch(args, self.backend)["status"], "cache_unregistered")
            status.assert_called_once_with(self.backend, self.directory)
            for operation in ("restart", "stop", "recover", "reconcile", "resume"):
                args.operation = operation
                with self.subTest(operation=operation), self.assertRaises(ValueError):
                    cli.dispatch(args, self.backend)
            args.operation, args.write, args.request = "resume", True, self.local / "request.json"
            with self.assertRaises(ValueError):
                cli.dispatch(args, self.backend)
            execute.assert_not_called()
        self.assertEqual(state.binding(self.directory / "state.json"), before)

    def test_maintenance_cli_requires_explicit_build_and_keeps_verify_readonly(self):
        import devex_clone_run_cli as cli

        output = self.local / "maintenance"
        build = SimpleNamespace(command="maintenance", operation="build", output=output, write=False)
        with patch("devex_clone_tools.build", return_value={}) as create, \
                patch("devex_clone_tools.verify", return_value={}) as verify:
            with self.assertRaises(ValueError):
                cli.dispatch(build, self.backend)
            create.assert_not_called()
            self.assertEqual(cli.dispatch(SimpleNamespace(**{**vars(build), "write": True}), self.backend), {
                "status": "maintenance_build_created", "receipt": str(output / "build.json"),
                "resources_modified": False, "restore_qualified": False,
            })
            create.assert_called_once_with(self.backend, output)
            check = SimpleNamespace(command="maintenance", operation="verify", output=output / "build.json", write=False)
            self.assertEqual(cli.dispatch(check, self.backend), {
                "status": "maintenance_build_verified", "receipt": str(output / "build.json"),
                "resources_modified": False, "restore_qualified": False,
            })
            verify.assert_called_once_with(self.backend, output / "build.json")
            verify.reset_mock()
            output.mkdir()
            directory = SimpleNamespace(command="maintenance", operation="verify", output=output, write=False)
            self.assertEqual(cli.dispatch(directory, self.backend), {
                "status": "maintenance_build_verified", "receipt": str(output / "build.json"),
                "resources_modified": False, "restore_qualified": False,
            })
            verify.assert_called_once_with(self.backend, output / "build.json")
            with self.assertRaises(ValueError):
                cli.dispatch(SimpleNamespace(**{**vars(check), "write": True}), self.backend)

    def test_maintenance_cli_resolves_relative_evidence_path_under_backend(self):
        import devex_clone_run_cli as cli

        relative = Path(".local-tests/maintenance-relative")
        with patch("devex_clone_tools.build", return_value={}) as create:
            cli.dispatch(SimpleNamespace(command="maintenance", operation="build", output=relative, write=True),
                         self.backend)
        create.assert_called_once_with(self.backend, self.backend / relative)

    def test_clone_status_resolves_relative_run_directory_under_backend(self):
        import devex_clone_run_cli as cli

        with patch.object(cli, "status", return_value={"status": "observed"}) as observe:
            result = cli.dispatch(SimpleNamespace(command="status", run_dir=Path(".local-tests/run")), self.backend)
        self.assertEqual(result, {"status": "observed"})
        observe.assert_called_once_with(self.backend, self.directory)

    def test_runtime_environment_and_identity_are_delegated_without_build(self):
        value = dict(self.value)
        request = self.file("runtime-request", {"source": {"runtime_dir": str(self.local / "runtime"), "api_url": "http://127.0.0.1:18210"}})
        value["source_request"] = request
        with patch("devex_clone_runtime.control", return_value={"state": "stopped"}) as control:
            run.run_runtime(self.backend, value, run.environments(self.backend, value), "source", "stop", ("api",))
        self.assertEqual(control.call_args.args[2:4], ("stop", ("api",)))

    def test_stop_does_not_require_current_source_to_match_build(self):
        with patch("source_fingerprints.artifact_sources", side_effect=ValueError("changed product")) as bridge, \
                patch.object(run, "run_runtime", return_value={"state": "stopped"}):
            result = run.execute(self.backend, self.directory, "runtime-source", "stop")
        self.assertEqual(result["status"], "stage_finished")
        bridge.assert_not_called()

    def test_stop_survives_missing_export_opposite_environment_and_source_scan_error(self):
        Path(self.value["source_export"]["path"]).unlink()
        Path(self.value["target_environment"]["path"]).unlink()
        with patch("source_fingerprints.current_execution_source", side_effect=OSError("source unavailable")), \
                patch.object(run, "run_runtime", return_value={"state": "stopped"}) as stop:
            result = run.execute(self.backend, self.directory, "runtime-source", "stop")
        self.assertEqual(result["status"], "stage_finished")
        stop.assert_called_once()
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["sources"], {"source_capture_error_type": "OSError"})

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

    def test_arm_input_is_local_handoff_and_does_not_open_product_bridge_context(self):
        request = self.local / "arm-request.json"
        write_json(request, {"kind": "fixture-arm-input"})
        expected = {"status": "seed_arm_input_published", "remote_writes": 0}
        with patch("source_fingerprints.artifact_sources",
                   side_effect=AssertionError("unexpected product context")) as sources, \
                patch("devex_clone_seed_runtime.execute_seed", return_value=expected) as publish:
            result = run.execute(self.backend, self.directory, "seed-runtime", "arm-input",
                                 seed_request=request)
        self.assertEqual(result["result"], expected)
        sources.assert_not_called()
        self.assertEqual(publish.call_args.args[3:6], ("arm-input", 1, request))

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
        from devex_clone_transfer import TransferSteps

        output = Path(self.value["copy_directory"])
        output.mkdir()
        saved = {"initialized": self.value["initialized"], "source_export": self.value["source_export"],
                 "plan_sha256": "a" * 64, "generation_sha256": "b" * 64}
        item = {"key": "shared-control"}
        result = {"status": "data_steps_verified", "plan_sha256": saved["plan_sha256"],
                  "generation_sha256": saved["generation_sha256"], "written_steps": 0,
                  "source_export_sha256": self.value["source_export"]["sha256"],
                  "fresh_target_sha256": self.value["initialized"]["sha256"], "pending_target_actions": [],
                  "unchanged_databases": 1, "unchanged_database_images": {TransferSteps.db_step(item): {"observed": True}}}
        write_json(output / "session.json", saved)
        write_json(output / "result.json", result)
        number = state.begin(self.directory, "copy", "resume", self.sources)
        state.finish(self.directory, number, result=result)
        plan = {"plan_sha256": saved["plan_sha256"], "pending_target_actions": [], "objects": [], "databases": [item]}
        with patch("devex_clone_ledger.CloneLedger") as ledger, patch("devex_clone.verify_plan_result", return_value=SimpleNamespace(plan=plan)):
            ledger.return_value.inspect.return_value = {"status": "partial", "steps": {}}
            run.require_target_copy(self.backend, self.directory, self.value, ("api", "worker"))
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
                patch.object(run, "run_export", return_value={"status": "export_verified"}):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")

    def test_final_source_check_failure_preserves_result_without_publishing_success(self):
        @contextmanager
        def source_context(*_args):
            yield
            raise ValueError("final source check")

        with patch("source_fingerprints.artifact_sources", side_effect=source_context), \
                patch.object(run, "run_export", return_value={"status": "export_verified"}):
            with self.assertRaises(ValueError):
                run.execute(self.backend, self.directory, "export", "run")
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
                patch.object(run, "run_export", return_value={"status": "export_verified"}):
            with self.assertRaises(OSError):
                run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "failed")

    def test_failed_lock_acquisition_does_not_finish_another_attempt(self):
        state.begin(self.directory, "copy", "resume", self.sources)
        with state.run_lock(self.directory), self.assertRaises(ValueError):
            run.execute(self.backend, self.directory, "export", "run")
        self.assertEqual(state.load_state(self.directory)["attempts"][0]["status"], "running")

    def test_status_is_readonly_and_reports_old_execution_source(self):
        with patch.object(run, "run_export", return_value={"status": "export_verified"}):
            run.execute(self.backend, self.directory, "export", "run")
        before = state.binding(self.directory / "state.json")
        with patch("source_fingerprints.current_execution_source", return_value={"changed": True}):
            report = run.status(self.backend, self.directory)
        self.assertFalse(report["attempts"][0]["execution_source_matches"])
        self.assertEqual(state.binding(self.directory / "state.json"), before)

    def test_concurrent_control_rejected_and_result_tampering_detected(self):
        with state.run_lock(self.directory), self.assertRaises(ValueError):
            run.execute(self.backend, self.directory, "export", "run")
        with patch.object(run, "run_export", return_value={"status": "export_verified"}):
            run.execute(self.backend, self.directory, "export", "run")
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
