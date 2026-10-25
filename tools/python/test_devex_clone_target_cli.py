"""fresh-target 统一入口的离线参数、绑定和环境隔离测试。"""
from __future__ import annotations

from contextlib import contextmanager, nullcontext
import json
import os
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

from devex_clone_capture import read_json, write_json
from process_environment import configured
from devex_clone_run_state import binding
import devex_clone_run_state as run_state
import devex_clone_run_cli as cli
import devex_clone_target_cli as target_cli
from devex_clone_target_binding import target_files
from devex_clone_target_state import generation_lock


def publish_initialized(target: Path, publish_files) -> None:
    after_unlock = []
    with generation_lock(target, after_initialize_unlock=after_unlock):
        write_json(target / "initialized.json", {"status": "fresh_target_initialized"})
        files = target_files(target, ignored={"initialize.lock"}, locked_guard=True)
        after_unlock.append(lambda: publish_files(files))


class TargetCliTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.workspace = self.local / "fresh-workspace"
        self.storage_run = self.local / "storage-run"
        self.storage_run.mkdir()
        write_json(self.storage_run / "manifest.json", {"kind": "offline-run"})
        run_state.initialize_state(self.storage_run)
        self.request = self.local / "request.json"
        self.environment = self.local / "environment.json"
        self.temporary = self.local / "temp"
        self.temporary.mkdir()
        write_json(self.request, {"kind": "offline-request"})
        write_json(self.environment, {"environment": {
            "APP_ENV": "test",
            "TEMP": str(self.temporary), "TMP": str(self.temporary)}})
        validator = patch.object(target_cli, "_validate_registered_request", return_value=None)
        self.request_validator = validator.start()
        self.addCleanup(validator.stop)
        bindings = patch("devex_clone_target_binding.execution_binary_bindings", return_value=[])
        bindings.start()
        self.addCleanup(bindings.stop)
        protector = patch.object(target_cli, "protect_binaries", return_value=nullcontext())
        protector.start()
        self.addCleanup(protector.stop)
        prepared_validator = patch.object(
            target_cli.snapshot_evidence, "validate_prepared_tree", return_value=None
        )
        prepared_validator.start()
        self.addCleanup(prepared_validator.stop)

    def args(self, operation, *, request=None, environment=None, storage_run=None,
             observation=None, write=True):
        return SimpleNamespace(command="fresh-target", workspace=self.workspace, operation=operation,
                               request=request, environment=environment, storage_run=storage_run,
                               observation_dir=observation, write=write)



    def prepare(self):
        """剩余 initialize/verify 场景只需要固定登记前像，不执行已迁走的 prepare。"""
        self.workspace.mkdir()
        target = self.workspace / "target"
        target.mkdir()
        registration = {"format_version": 1, "kind": "devex-clone-fresh-target-registration",
                        "request": binding(self.request), "environment": binding(self.environment),
                        "storage_run": {"path": str(self.storage_run),
                                        "manifest": binding(self.storage_run / "manifest.json"),
                                        "state": binding(self.storage_run / "state.json")},
                        "target_directory": str(target)}
        write_json(self.workspace / "registration.json", registration)
        result = {"status": "fresh_creation_prepared", "request": registration["request"]}
        write_json(target / "prepare.json", result)
        files = target_cli.snapshot_evidence.write_snapshot(self.backend, self.workspace, registration,
                                                          "prepared", expected_document=result)
        return {"status": result["status"], "registration": binding(self.workspace / "registration.json"),
                "target": binding(target / "prepare.json"), "files": files, "restore_qualified": False}




    def test_fixture_initial_services_remain_locked_without_being_treated_as_restarts(self):
        fixture = self.local / "fixture-services"
        fixture.mkdir()
        write_json(fixture / "manifest.json", {
            "format_version": 1, "kind": "reference-fixture-service-run",
            "review": {"path": "review", "bytes": 1, "sha256": "0" * 64},
            "bootstrap": {"path": "bootstrap", "bytes": 1, "sha256": "1" * 64},
            "execution_backend": str(self.backend), "scope_id": "fixture-services",
            "data_directory_was_empty": True,
        })
        run_state.initialize_state(fixture)
        for stage, mode in (("storage-target", "initial"), ("cache-target", "initial"),
                            ("fixture-buckets", "prepare")):
            number = run_state.begin(fixture, stage, mode, {"fixture": True})
            run_state.finish(fixture, number, result={"status": stage + "-initial"})
        self.storage_run = fixture
        self.prepare()
        def initialize(_backend, _target, *, storage_run):
            self.assertIsNone(storage_run)
            self.assertTrue((fixture / "run.lock").is_dir())
            return {"status": "fixture-context-observed"}
        result = target_cli._run_registered(self.backend, self.workspace, initialize)
        self.assertEqual(result["status"], "fixture-context-observed")
        self.assertEqual(target_cli._storage_run(self.backend, read_json(self.workspace / "registration.json")["storage_run"]), fixture)

    def test_fixture_service_generation_reads_appended_closed_state(self):
        fixture = self.local / "fixture-services-closed"
        fixture.mkdir()
        write_json(fixture / "manifest.json", {
            "format_version": 1, "kind": "reference-fixture-service-run",
            "review": {"path": "review", "bytes": 1, "sha256": "0" * 64},
            "bootstrap": {"path": "bootstrap", "bytes": 1, "sha256": "1" * 64},
            "execution_backend": str(self.backend), "scope_id": "fixture-services",
            "data_directory_was_empty": True,
        })
        run_state.initialize_state(fixture)
        for stage, mode in (("storage-target", "initial"), ("cache-target", "initial"),
                            ("fixture-buckets", "prepare")):
            number = run_state.begin(fixture, stage, mode, {"fixture": True})
            run_state.finish(fixture, number, result={"status": stage + "-initial"})
        descriptor = {"path": str(fixture), "manifest": binding(fixture / "manifest.json"),
                      "state": binding(fixture / "state.json")}
        number = run_state.begin(fixture, "fixture-services", "close", {"fixture": True})
        run_state.finish(fixture, number, result={"status": "services_closed"})

        def closed(_directory, state):
            self.assertEqual(len(state["attempts"]), 4)
            return {"closed": True, "external_recovery": None,
                    "active_generation": {"kind": "initial"}}

        with patch("fixture_service_client.validate_history", side_effect=closed):
            result = target_cli._fixture_service_generation(fixture, descriptor)
        self.assertFalse(result["available"])
        self.assertIn("fixture services restart", result["reason"])

    def test_fixture_service_generation_delegates_failed_close_reconcile_history(self):
        fixture = self.local / "fixture-services-reconciled"
        fixture.mkdir()
        write_json(fixture / "manifest.json", {
            "format_version": 1, "kind": "reference-fixture-service-run",
            "review": {"path": "review", "bytes": 1, "sha256": "0" * 64},
            "bootstrap": {"path": "bootstrap", "bytes": 1, "sha256": "1" * 64},
            "execution_backend": str(self.backend), "scope_id": "fixture-services",
            "data_directory_was_empty": True,
        })
        run_state.initialize_state(fixture)
        for stage, mode in (("storage-target", "initial"), ("cache-target", "initial"),
                            ("fixture-buckets", "prepare")):
            number = run_state.begin(fixture, stage, mode, {"fixture": True})
            run_state.finish(fixture, number, result={"status": stage + "-initial"})
        descriptor = {"path": str(fixture), "manifest": binding(fixture / "manifest.json"),
                      "state": binding(fixture / "state.json")}
        number = run_state.begin(fixture, "fixture-services", "close", {"fixture": True})
        run_state.finish(fixture, number, error=OSError("fixture close interrupted"))
        number = run_state.begin(fixture, "fixture-services", "reconcile", {"fixture": True})
        run_state.finish(fixture, number, result={"status": "failed_close_reconciled"})

        def reconciled(_directory, state):
            attempts = state["attempts"]
            self.assertEqual([(item["mode"], item["status"]) for item in attempts[3:]],
                             [("close", "failed"), ("reconcile", "passed")])
            return {"closed": True, "external_recovery": None,
                    "active_generation": {"kind": "initial"}}

        with patch("fixture_service_client.validate_history", side_effect=reconciled):
            result = target_cli._fixture_service_generation(fixture, descriptor)
        self.assertFalse(result["available"])
        self.assertIn("fixture services restart", result["reason"])

    def test_initialize_and_verify_reopen_registration_and_never_replay(self):
        self.prepare()
        initialized = self.workspace / "target/initialized.json"

        def initialize(backend, target, *, storage_run, publish_files):
            self.assertEqual(storage_run, self.storage_run)
            self.assertTrue((storage_run / "run.lock").is_dir())
            self.assertEqual(os.environ["APP_ENV"], "test")
            publish_initialized(target, publish_files)
            return {"status": "fresh_target_initialized"}

        with patch("devex_clone_target.initialize_target", side_effect=initialize):
            result = target_cli.initialize(self.backend, self.workspace)
        self.assertEqual(result["target"], binding(initialized))
        observation = self.local / "observation-0001"

        def verify(backend, target, output, *, storage_run):
            self.assertEqual((target, output), (self.workspace / "target", observation))
            self.assertEqual(storage_run, self.storage_run)
            self.assertTrue((storage_run / "run.lock").is_dir())
            output.mkdir()
            write_json(output / "verify.json", {"status": "fresh_target_reverified"})
            return {"status": "fresh_target_reverified"}

        initialized_history = ({"status": "fresh_target_initialized"}, {"kind": "offline-request"})
        with patch.object(target_cli, "initialization_history", return_value=initialized_history), \
                patch("devex_clone_target.verify_target", side_effect=verify):
            verified = target_cli.verify(self.backend, self.workspace, observation)
        self.assertEqual(verified["observation"], binding(observation / "verify.json"))
        with patch("devex_clone_target.initialize_target") as repeat:
            with self.assertRaises(ValueError):
                target_cli.initialize(self.backend, self.workspace)
            repeat.assert_not_called()
        self.assertTrue(initialized.is_file())

    def test_initialize_holds_binary_protection_around_registered_operation(self):
        self.prepare()
        active = []

        @contextmanager
        def protect(bindings):
            self.assertEqual(bindings, [{"path": "fixture.exe", "sha256": "a" * 64}])
            active.append(True)
            try:
                yield
            finally:
                active.pop()

        def initialize(_backend, target, *, storage_run, publish_files):
            self.assertEqual(active, [True])
            self.assertTrue((storage_run / "run.lock").is_dir())
            publish_initialized(target, publish_files)
            return {"status": "fresh_target_initialized"}

        with patch("devex_clone_target_binding.execution_binary_bindings",
                   return_value=[{"path": "fixture.exe", "sha256": "a" * 64}]) as collect, \
                patch.object(target_cli, "protect_binaries", side_effect=protect), \
                patch("devex_clone_target.initialize_target", side_effect=initialize):
            result = target_cli.initialize(self.backend, self.workspace)
        self.assertEqual(result["status"], "fresh_target_initialized")
        collect.assert_called_once_with(self.backend, {"kind": "offline-request"})

    def test_binary_protection_failure_prevents_registered_operation_and_run_lock(self):
        self.prepare()
        with patch("devex_clone_target_binding.execution_binary_bindings",
                   return_value=[{"path": "fixture.exe", "sha256": "a" * 64}]), \
                patch.object(target_cli, "protect_binaries", side_effect=PermissionError("busy")), \
                patch("devex_clone_target.initialize_target") as initialize, \
                self.assertRaisesRegex(PermissionError, "busy"):
            target_cli.initialize(self.backend, self.workspace)
        initialize.assert_not_called()
        self.assertFalse((self.storage_run / "run.lock").exists())
        self.assertFalse((self.workspace / "target/initialize.started.json").exists())

    def test_resume_initialize_routes_strict_migration_state_through_registered_guards(self):
        self.prepare()
        target = self.workspace / "target"
        write_json(target / "initialize.started.json", {"at": "fixture", "generation_sha256": "a" * 64})
        write_json(target / "failure.json", {"status": "needs_reconciliation"})
        state = {"resumable": True, "reason": None, "mode": "migration",
                 "completed": [{"id": "control-up"}], "next_index": 1,
                 "operations": [{"id": "control-up", "write": True},
                                {"id": "control-verify", "write": False}]}
        protected = []

        @contextmanager
        def protect(_bindings):
            protected.append(True)
            try:
                yield
            finally:
                protected.pop()

        def resume(_backend, output, *, storage_run, request_descriptor,
                   prepared_files, publish_files):
            self.assertEqual(output, target)
            self.assertEqual(storage_run, self.storage_run)
            self.assertEqual(request_descriptor, binding(self.request))
            self.assertEqual(prepared_files["descriptor"], binding(
                self.workspace / "prepared-files.json"))
            self.assertTrue((self.storage_run / "run.lock").is_dir())
            self.assertEqual(protected, [True])
            publish_initialized(output, publish_files)
            return {"status": "fresh_target_initialized"}

        with patch.object(target_cli, "protect_binaries", side_effect=protect), \
                patch("devex_clone_target_resume.initialize_resume_state", return_value=state), \
                patch("devex_clone_target_resume.resume_initialize_target", side_effect=resume) as operation:
            blocked = {"available": False, "active_generation": {"kind": "initial"},
                       "reason": "夹具服务已经关闭"}
            with patch.object(target_cli, "_fixture_service_generation", return_value=blocked), \
                    self.assertRaisesRegex(ValueError, "已经关闭"):
                target_cli.resume_initialize(self.backend, self.workspace)
            operation.assert_not_called()
            result = target_cli.resume_initialize(self.backend, self.workspace)
        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertTrue(result["resumed"])
        operation.assert_called_once()



    def test_binding_drift_fails_after_stage_without_copying_environment(self):
        self.prepare()

        def drift(_backend, _target, *, storage_run, publish_files):
            write_json(self.workspace / "target/initialized.json", {"status": "fresh_target_initialized"})
            self.environment.write_text('{"environment":{"APP_SCOPE_ID":"changed"}}', encoding="utf-8")
            return {"status": "fresh_target_initialized"}

        with patch("devex_clone_target.initialize_target", side_effect=drift), \
                self.assertRaises(ValueError):
            target_cli.initialize(self.backend, self.workspace)
        self.assertNotIn("private-secret", (self.workspace / "registration.json").read_text(encoding="utf-8"))

    def test_prepare_descriptor_or_unknown_target_file_blocks_initialize(self):
        for kind in ("descriptor", "unknown"):
            with self.subTest(kind=kind):
                self.workspace = self.local / ("fresh-" + kind)
                self.prepare()
                if kind == "descriptor":
                    write_json(self.local / "other-request.json", {"kind": "other"})
                    (self.workspace / "target/prepare.json").write_text(json.dumps({
                        "request": binding(self.local / "other-request.json")}), encoding="utf-8")
                else:
                    (self.workspace / "target/unknown.txt").write_text("unexpected", encoding="utf-8")
                with patch("devex_clone_target.initialize_target") as operation, self.assertRaises(ValueError):
                    target_cli.initialize(self.backend, self.workspace)
                operation.assert_not_called()

    def test_missing_managed_tool_binding_fails_before_workspace_creation(self):
        with patch("rate_limit_tool.tool_binding", return_value=None), self.assertRaisesRegex(
                ValueError, "缺少受管 Rust 工具"):
            cli.dispatch(self.args("prepare", request=self.request, environment=self.environment,
                                   storage_run=self.storage_run), self.backend)
        self.assertFalse(self.workspace.exists())



    def test_workspace_guard_wraps_remaining_initialize_and_verify_stages(self):
        active = []
        actual = target_cli.process_guard

        def guarded(directory, filename):
            context = actual(directory, filename)

            class Guard:
                def __enter__(self):
                    context.__enter__()
                    active.append((directory, filename))

                def __exit__(self, *args):
                    active.pop()
                    return context.__exit__(*args)

            return Guard()

        self.prepare()

        def initialize(_backend, target, *, storage_run, publish_files):
            self.assertEqual(active, [(self.workspace, target_cli.WORKSPACE_GUARD)])
            publish_initialized(target, publish_files)
            return {"status": "fresh_target_initialized"}

        with patch.object(target_cli, "process_guard", side_effect=guarded), \
                patch("devex_clone_target.initialize_target", side_effect=initialize):
            target_cli.initialize(self.backend, self.workspace)

        observation = self.local / "guard-observation"

        def verify(_backend, target, output, *, storage_run):
            self.assertEqual(active, [(self.workspace, target_cli.WORKSPACE_GUARD)])
            self.assertEqual(target, self.workspace / "target")
            output.mkdir()
            write_json(output / "verify.json", {"status": "fresh_target_reverified"})
            return {"status": "fresh_target_reverified"}

        history = ({"status": "fresh_target_initialized"}, {"kind": "offline-request"})
        with patch.object(target_cli, "process_guard", side_effect=guarded), \
                patch.object(target_cli, "initialization_history", return_value=history), \
                patch("devex_clone_target.verify_target", side_effect=verify):
            target_cli.verify(self.backend, self.workspace, observation)

    def test_configured_filters_case_insensitively_and_private_values_win(self):
        result = configured({"TEMP": "private-temp", "APP_SCOPE": "private"}, {
            "Path": "tool-path", "temp": "ambient-temp", "app_old": "unsafe", "cargo_home": "unsafe",
            "https_proxy": "unsafe", "CUSTOM_PROXY": "unsafe", "safe": "kept"})
        self.assertEqual(result, {"Path": "tool-path", "TEMP": "private-temp", "APP_SCOPE": "private"})

    def test_dispatch_enforces_exact_stage_arguments_and_write_boundary(self):
        prepare = self.args("prepare", request=self.request, environment=self.environment,
                            storage_run=self.storage_run, write=False)
        with self.assertRaises(ValueError):
            cli.dispatch(prepare, self.backend)
        prepare.write = True
        with patch("devex_clone_target_binding._native", return_value={"status": "prepared"}) as operation:
            self.assertEqual(cli.dispatch(prepare, self.backend)["status"], "prepared")
            operation.assert_called_once_with(
                "fresh-prepare", self.backend, workspace=str(self.workspace), request=str(self.request),
                environment=str(self.environment), storage_run=str(self.storage_run), write=True)
        resume = self.args("resume-prepare")
        with patch("devex_clone_target_binding._native", return_value={"status": "prepared"}) as operation:
            self.assertEqual(cli.dispatch(resume, self.backend)["status"], "prepared")
            operation.assert_called_once_with("fresh-resume-prepare", self.backend,
                                              workspace=str(self.workspace), write=True)
        resume_initialize = self.args("resume-initialize")
        with patch.object(target_cli, "resume_initialize", return_value={"status": "initialized"}) as operation:
            self.assertEqual(cli.dispatch(resume_initialize, self.backend)["status"], "initialized")
            operation.assert_called_once_with(self.backend, self.workspace)
        invalid = [self.args("prepare", request=self.request, environment=self.environment),
                   self.args("prepare", request=self.request, environment=self.environment,
                             storage_run=self.storage_run, observation=self.local / "unused"),
                   self.args("initialize", request=self.request),
                   self.args("initialize", storage_run=self.storage_run),
                   self.args("verify", observation=None), self.args("status", write=True),
                   self.args("status", observation=self.local / "unused", write=False)]
        for args in invalid:
            with self.subTest(operation=args.operation), self.assertRaises(ValueError):
                cli.dispatch(args, self.backend)

    def test_dispatch_resolves_relative_fresh_target_workspace(self):
        status = self.args("status", write=False)
        status.workspace = Path(".local-tests/fresh-workspace")
        with patch("devex_clone_target_binding._native", return_value={"status": "fresh_target_unregistered"}) as observe:
            self.assertEqual(cli.dispatch(status, self.backend)["status"], "fresh_target_unregistered")
        observe.assert_called_once_with("fresh-status", self.backend, workspace=str(self.workspace))
        for operation in ("prepare", "resume-prepare"):
            args = self.args(operation, request=self.request if operation == "prepare" else None,
                             environment=self.environment if operation == "prepare" else None,
                             storage_run=self.storage_run if operation == "prepare" else None)
            args.workspace = Path(".local-tests/fresh-workspace")
            with patch("devex_clone_target_binding._native", return_value={"status": "fresh_creation_prepared"}) as native:
                self.assertEqual(cli.dispatch(args, self.backend)["status"], "fresh_creation_prepared")
            expected = {"workspace": str(self.workspace), "write": True}
            if operation == "prepare":
                expected.update(request=str(self.request), environment=str(self.environment), storage_run=str(self.storage_run))
            native.assert_called_once_with("fresh-" + operation, self.backend, **expected)







    def test_storage_history_accepts_only_published_target_restart_append(self):
        self.prepare()
        historical = read_json(self.workspace / "registration.json")["storage_run"]
        number = run_state.begin(self.storage_run, "storage-target", "restart", {"fixture": True})
        run_state.finish(self.storage_run, number, result={"status": "storage_restarted"})
        with patch("devex_clone_storage.registered_storage_binding", return_value={"published": True}):
            self.assertEqual(target_cli._storage_run(self.backend, historical), self.storage_run)
        number = run_state.begin(self.storage_run, "export", "run", {"fixture": True})
        run_state.finish(self.storage_run, number, result={"status": "export_verified"})
        with self.assertRaisesRegex(ValueError, "受控目标存储阶段"):
            target_cli._storage_run(self.backend, historical)



    def local_files(self):
        return {path.relative_to(self.local).as_posix(): binding(path)
                for path in self.local.rglob("*") if path.is_file()}







    def test_storage_run_binding_drift_is_rejected_after_stage(self):
        self.prepare()

        def drift(_backend, _target, *, storage_run, publish_files):
            write_json(self.workspace / "target/initialized.json", {"status": "fresh_target_initialized"})
            (storage_run / "state.json").write_text('{"attempts":[{"status":"changed"}]}', encoding="utf-8")
            return {"status": "fresh_target_initialized"}

        with patch("devex_clone_target.initialize_target", side_effect=drift), self.assertRaises(ValueError):
            target_cli.initialize(self.backend, self.workspace)

    def test_even_valid_storage_append_during_stage_is_rejected(self):
        self.prepare()

        def drift(_backend, _target, *, storage_run, publish_files):
            number = run_state.begin(storage_run, "storage-target", "restart", {"fixture": True})
            run_state.finish(storage_run, number, result={"status": "storage_restarted"})
            return {"status": "fresh_target_initialized"}

        with patch("devex_clone_storage.registered_storage_binding", return_value={"published": True}), \
                patch("devex_clone_target.initialize_target", side_effect=drift), \
                self.assertRaisesRegex(ValueError, "阶段期间变化"):
            target_cli.initialize(self.backend, self.workspace)

    def test_verify_observation_must_be_topologically_separate(self):
        self.prepare()
        candidates = (self.workspace / "observation", self.workspace / "target/observation",
                      self.storage_run / "observation")
        with patch("devex_clone_target.verify_target") as operation:
            for observation in candidates:
                with self.subTest(observation=observation), self.assertRaises(ValueError):
                    target_cli.verify(self.backend, self.workspace, observation)
            operation.assert_not_called()


if __name__ == "__main__":
    unittest.main()
