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

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

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
            "APP_SCOPE_ID": "fresh-fixture", "RYFRAME_RESET_ADMIN_PASSWORD": "private-secret",
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

    @staticmethod
    def _prepare_stub(backend, request, target, *, storage_run, request_descriptor):
        if not (storage_run / "run.lock").is_dir():
            raise AssertionError("storage run lock missing")
        if request_descriptor != binding(request):
            raise AssertionError("request descriptor missing")
        target.mkdir()
        result = {"status": "fresh_creation_prepared", "request": binding(request)}
        write_json(target / "prepare.json", result)
        return result

    def prepare(self):
        with patch("devex_clone_target.prepare_target", side_effect=self._prepare_stub):
            return target_cli.prepare(self.backend, self.workspace, self.request, self.environment, self.storage_run)

    def test_prepare_registers_only_bindings_and_uses_safely_merged_environment(self):
        observed = {}

        def prepare(backend, request, target, *, storage_run, request_descriptor):
            self.assertEqual(storage_run, self.storage_run)
            self.assertEqual(request_descriptor, binding(request))
            observed.update(os.environ)
            return self._prepare_stub(backend, request, target, storage_run=storage_run,
                                      request_descriptor=request_descriptor)

        ambient = {"PATH": "fixture-path", "SystemRoot": "fixture-root", "SAFE_BASE": "removed",
                   "APP_SCOPE_ID": "ambient-wrong", "RYFRAME_OLD": "ambient-secret",
                   "SNOWFLAKE_WORKER_ID": "9", "CARGO_HOME": "unsafe", "AWS_SECRET_ACCESS_KEY": "unsafe",
                   "RUSTFS_SECRET_KEY": "unsafe", "MYSQL_PWD": "unsafe", "HTTP_PROXY": "unsafe",
                   "NO_PROXY": "unsafe", "GH_TOKEN": "unsafe", "PYTHONPATH": "unsafe",
                   "NODE_OPTIONS": "unsafe"}
        with patch.dict(os.environ, ambient, clear=True), \
                patch("devex_clone_target.prepare_target", side_effect=prepare):
            result = target_cli.prepare(self.backend, self.workspace, self.request, self.environment, self.storage_run)
        self.assertEqual(result["status"], "fresh_creation_prepared")
        folded = {key.upper(): value for key, value in observed.items()}
        self.assertEqual(folded["PATH"], "fixture-path")
        self.assertEqual(folded["SYSTEMROOT"], "fixture-root")
        self.assertEqual(observed["APP_SCOPE_ID"], "fresh-fixture")
        self.assertEqual(observed["RYFRAME_RESET_ADMIN_PASSWORD"], "private-secret")
        for name in ("RYFRAME_OLD", "SNOWFLAKE_WORKER_ID", "CARGO_HOME", "AWS_SECRET_ACCESS_KEY",
                     "RUSTFS_SECRET_KEY", "MYSQL_PWD", "HTTP_PROXY", "NO_PROXY", "SAFE_BASE",
                     "GH_TOKEN", "PYTHONPATH", "NODE_OPTIONS"):
            self.assertNotIn(name, observed)
        registration = read_json(self.workspace / "registration.json")
        self.assertEqual(registration["request"], binding(self.request))
        self.assertEqual(registration["environment"], binding(self.environment))
        self.assertEqual(registration["storage_run"], {
            "path": str(self.storage_run), "manifest": binding(self.storage_run / "manifest.json"),
            "state": binding(self.storage_run / "state.json")})
        self.assertEqual(registration["target_directory"], str(self.workspace / "target"))
        self.assertNotIn("private-secret", json.dumps(registration))
        self.assertEqual(binding(self.environment), registration["environment"])
        status = target_cli.status(self.backend, self.workspace)
        self.assertEqual(status["status"], "fresh_creation_prepared")
        self.assertEqual(status["last_successful_stage"], "prepared")
        self.assertEqual(status["pending_stage"], "initialization")
        self.assertEqual(status["next_action"], "initialize")

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
        observed = {}

        def prepare(backend, request, target, *, storage_run, request_descriptor):
            observed["storage_run"] = storage_run
            return self._prepare_stub(backend, request, target, storage_run=fixture,
                                      request_descriptor=request_descriptor)

        with patch("devex_clone_target.prepare_target", side_effect=prepare):
            result = target_cli.prepare(self.backend, self.workspace, self.request, self.environment, fixture)
        self.assertEqual(result["status"], "fresh_creation_prepared")
        self.assertIsNone(observed["storage_run"])
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

        with patch("reference_fixture_service_history.validate_history", side_effect=closed):
            result = target_cli.fixture_context.fixture_service_generation(fixture, descriptor)
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

        with patch("reference_fixture_service_history.validate_history", side_effect=reconciled):
            result = target_cli.fixture_context.fixture_service_generation(fixture, descriptor)
        self.assertFalse(result["available"])
        self.assertIn("fixture services restart", result["reason"])

    def test_initialize_and_verify_reopen_registration_and_never_replay(self):
        self.prepare()
        initialized = self.workspace / "target/initialized.json"

        def initialize(backend, target, *, storage_run, publish_files):
            self.assertEqual(storage_run, self.storage_run)
            self.assertTrue((storage_run / "run.lock").is_dir())
            self.assertEqual(os.environ["APP_SCOPE_ID"], "fresh-fixture")
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
        with patch.object(target_cli, "initialization_history", return_value=initialized_history):
            status = target_cli.status(self.backend, self.workspace)
        self.assertEqual(status["status"], "fresh_target_initialized")
        self.assertEqual(status["last_successful_stage"], "initialized")
        self.assertEqual(status["pending_stage"], "verification")
        self.assertEqual(status["next_action"], "verify")
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
            with patch.object(target_cli.fixture_context, "fixture_service_generation", return_value=blocked), \
                    self.assertRaisesRegex(ValueError, "已经关闭"):
                target_cli.resume_initialize(self.backend, self.workspace)
            operation.assert_not_called()
            result = target_cli.resume_initialize(self.backend, self.workspace)
        self.assertEqual(result["status"], "fresh_target_initialized")
        self.assertTrue(result["resumed"])
        operation.assert_called_once()

    def test_status_reports_exact_migration_prefix_and_readonly_next_operation(self):
        self.prepare()
        target = self.workspace / "target"
        write_json(target / "initialize.started.json", {"at": "fixture", "generation_sha256": "a" * 64})
        state = {"resumable": True, "reason": None, "mode": "migration",
                 "completed": [{"id": "control-up"}, {"id": "control-verify"},
                               {"id": "tenant-data-dedicated-a-up"}],
                 "next_index": 3,
                 "operations": [{"id": "control-up", "write": True},
                                {"id": "control-verify", "write": False},
                                {"id": "tenant-data-dedicated-a-up", "write": True},
                                {"id": "tenant-data-dedicated-a-verify", "write": False}]}
        before = self.local_files()
        with patch("devex_clone_target_resume.initialize_resume_state", return_value=state):
            result = target_cli.status(self.backend, self.workspace)
        self.assertEqual(result["status"], "fresh_target_migration_resume_pending")
        self.assertEqual(result["last_successful_stage"],
                         "migration:tenant-data-dedicated-a-up")
        self.assertEqual(result["next_action"], "resume-initialize")
        self.assertEqual(result["resume"]["next_operation"],
                         "tenant-data-dedicated-a-verify")
        self.assertTrue(result["resume"]["next_operation_is_read_only"])
        blocked_services = {"available": False, "active_generation": {"kind": "initial"},
                            "reason": "夹具服务已正常关闭，当前生命周期没有可执行重启"}
        with patch("devex_clone_target_resume.initialize_resume_state", return_value=state), \
                patch.object(target_cli.fixture_context, "fixture_service_generation", return_value=blocked_services):
            blocked = target_cli.status(self.backend, self.workspace)
        self.assertEqual(blocked["status"], "fresh_target_migration_resume_pending")
        self.assertEqual(blocked["pending_stage"], "service_restart")
        self.assertIsNone(blocked["next_action"])
        self.assertFalse(blocked["evidence_valid"])
        self.assertEqual(blocked["resume"], result["resume"])
        self.assertIn("没有可执行重启", blocked["blocking_reason"])
        self.assertEqual(self.local_files(), before)

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

    def test_private_environment_rejects_shell_and_case_duplicate_fields(self):
        valid = {"TEMP": str(self.temporary), "TMP": str(self.temporary)}
        for number, environment in enumerate((
                {**valid, "PATH": "forbidden"},
                {**valid, "APP_SCOPE": "one", "app_scope": "two"},
                {**valid, "APP_SCOPE": 1},
                {"APP_SCOPE": "missing-private-temp"},
                {"TEMP": str(self.temporary), "TMP": str(self.local / "other-temp")},
                {"TEMP": "relative-temp", "TMP": "relative-temp"},
                {"TEMP": str(self.backend.parent), "TMP": str(self.backend.parent)},
                {"TEMP": str(self.request), "TMP": str(self.request)},
                {"TEMP": str(self.local / "missing-temp"), "TMP": str(self.local / "missing-temp")})):
            with self.subTest(environment=environment):
                candidate = self.local / f"candidate-{number}.json"
                write_json(candidate, {"environment": environment})
                with self.assertRaises(ValueError):
                    target_cli.prepare(self.backend, self.workspace, self.request, candidate, self.storage_run)
                self.assertFalse(self.workspace.exists())

    def test_workspace_guard_wraps_all_mutating_stages(self):
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

        def prepare(backend, request, target, *, storage_run, request_descriptor):
            self.assertEqual(active, [(self.workspace, target_cli.WORKSPACE_GUARD)])
            return self._prepare_stub(backend, request, target, storage_run=storage_run,
                                      request_descriptor=request_descriptor)

        with patch.object(target_cli, "process_guard", side_effect=guarded), \
                patch("devex_clone_target.prepare_target", side_effect=prepare):
            target_cli.prepare(self.backend, self.workspace, self.request, self.environment, self.storage_run)

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
        with patch.object(target_cli, "prepare", return_value={"status": "prepared"}) as operation:
            self.assertEqual(cli.dispatch(prepare, self.backend)["status"], "prepared")
            operation.assert_called_once_with(self.backend, self.workspace, self.request,
                                              self.environment, self.storage_run)
        resume = self.args("resume-prepare")
        with patch.object(target_cli, "resume_prepare", return_value={"status": "prepared"}) as operation:
            self.assertEqual(cli.dispatch(resume, self.backend)["status"], "prepared")
            operation.assert_called_once_with(self.backend, self.workspace)
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
        status = self.args("status", write=False)
        with patch.object(target_cli, "status", return_value={"status": "fresh_target_unregistered"}):
            self.assertEqual(cli.dispatch(status, self.backend)["status"], "fresh_target_unregistered")

    def test_dispatch_resolves_relative_fresh_target_workspace(self):
        status = self.args("status", write=False)
        status.workspace = Path(".local-tests/fresh-workspace")
        with patch.object(target_cli, "status", return_value={"status": "fresh_target_unregistered"}) as observe:
            self.assertEqual(cli.dispatch(status, self.backend)["status"], "fresh_target_unregistered")
        observe.assert_called_once_with(self.backend, self.workspace)

    def test_status_is_local_and_marks_incomplete_workspace_for_reconciliation(self):
        unregistered = target_cli.status(self.backend, self.workspace)
        self.assertEqual(unregistered["status"], "fresh_target_unregistered")
        self.assertEqual(unregistered["next_action"], "prepare")
        self.assertFalse((self.workspace / target_cli.WORKSPACE_GUARD).exists())
        self.workspace.mkdir()
        incomplete = target_cli.status(self.backend, self.workspace)
        self.assertEqual(incomplete["status"], "fresh_target_needs_reconciliation")
        self.assertFalse(incomplete["evidence_valid"])
        self.assertIsNone(incomplete["next_action"])
        self.assertFalse((self.workspace / target_cli.WORKSPACE_GUARD).exists())

    def test_status_requires_new_workspace_after_verified_preflight_reconciliation(self):
        self.prepare()
        target = self.workspace / "target"
        write_json(target / "initialize.started.json", {"status": "started"})
        write_json(target / "failure.json", {"status": "needs_reconciliation"})
        reports = target / "reset-state"
        reports.mkdir()
        report = reports / "fixture.report.json"
        write_json(report, {"status": "failed", "failed_phase": "preflight"})
        digest = lambda path: {key: value for key, value in binding(path).items() if key != "path"}
        before = {"databases": [{"exists": True}], "objects": {}, "redis": {}}
        after = {"databases": [{"exists": False}], "objects": {}, "redis": {}}
        write_json(target / "reconciliation-completed.json", {
            "status": "preflight_failure_reconciled", "before": before, "after": after,
            "failure": digest(target / "failure.json"), "reset_report": digest(report),
            "automatic_retry": False, "restore_qualified": False,
        })
        status = target_cli.status(self.backend, self.workspace)
        self.assertEqual(status["status"], "fresh_target_preflight_reconciled")
        self.assertEqual(status["last_successful_stage"], "preflight_reconciled")
        self.assertEqual(status["pending_stage"], "new_registration")
        self.assertEqual(status["next_action"], "prepare-new-workspace")
        self.assertTrue(status["evidence_valid"])

    def test_registration_only_interruption_uses_explicit_resume_prepare(self):
        def interrupted(*_args, **_kwargs):
            raise OSError("fixture interruption before target")

        with patch("devex_clone_target.prepare_target", side_effect=interrupted), \
                self.assertRaises(OSError):
            target_cli.prepare(self.backend, self.workspace, self.request,
                               self.environment, self.storage_run)
        status = target_cli.status(self.backend, self.workspace)
        self.assertEqual(status["status"], "fresh_target_prepare_interrupted")
        self.assertEqual(status["last_successful_stage"], "registered")
        self.assertEqual(status["next_action"], "resume-prepare")

        def resumed(_backend, target, *, storage_run, request_descriptor):
            target.mkdir()
            write_json(target / "request.json", read_json(self.request))
            result = {"status": "fresh_creation_prepared", "request": request_descriptor}
            write_json(target / "prepare.json", result)
            return result

        with patch("devex_clone_target.resume_prepare_target", side_effect=resumed):
            result = target_cli.resume_prepare(self.backend, self.workspace)
        self.assertTrue(result["resumed"])
        self.assertEqual(result["status"], "fresh_creation_prepared")
        self.assertTrue((self.workspace / "prepared-files.json").is_file())

    def test_status_does_not_offer_resume_when_registered_request_is_not_executable(self):
        with patch("devex_clone_target.prepare_target", side_effect=OSError("interrupted")), \
                self.assertRaises(OSError):
            target_cli.prepare(self.backend, self.workspace, self.request,
                               self.environment, self.storage_run)
        before = self.local_files()
        self.request_validator.side_effect = ValueError("审阅计划尚未就绪")
        status = target_cli.status(self.backend, self.workspace)
        self.assertEqual(status["status"], "fresh_target_needs_reconciliation")
        self.assertEqual(status["last_successful_stage"], "registered")
        self.assertEqual(status["pending_stage"], "reconciliation")
        self.assertIsNone(status["next_action"])
        self.assertFalse(status["evidence_valid"])
        self.assertEqual(status["blocking_reason"], "固定请求未达到可执行条件")
        self.assertEqual(self.local_files(), before)

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

    def test_dead_controller_requires_explicit_recovery_before_same_directory_resume(self):
        with patch("devex_clone_target.prepare_target", side_effect=OSError("interrupted")), \
                self.assertRaises(OSError):
            target_cli.prepare(self.backend, self.workspace, self.request, self.environment, self.storage_run)
        registration = binding(self.workspace / "registration.json")
        manifest = binding(self.storage_run / "manifest.json")
        ledger = read_json(self.storage_run / "state.json")
        lock = self.storage_run / "run.lock"
        lock.mkdir()
        owner = {"format_version": 1, "identity": {"pid": 10001, "started": "123", "executable": "fixture"},
                 "directory": str(self.storage_run), "manifest_sha256": manifest["sha256"]}
        write_json(lock / "owner.json", owner)
        owner_binding = binding(lock / "owner.json")
        before = self.local_files()
        with patch.object(run_state, "process_identity", return_value=None):
            status = target_cli.status(self.backend, self.workspace)
            self.assertEqual(status["last_successful_stage"], "registered")
            self.assertEqual(status["pending_stage"], "controller")
            self.assertEqual(status["next_action"], "recover")
            self.assertEqual(status["controller"]["owner"], owner_binding)
            with patch.object(target_cli, "_workspace_control") as guard, \
                    patch("devex_clone_target.resume_prepare_target") as operation, \
                    self.assertRaisesRegex(ValueError, "原 recover"):
                target_cli.resume_prepare(self.backend, self.workspace)
            guard.assert_not_called()
            operation.assert_not_called()
            self.assertEqual(self.local_files(), before)
            recovered = run_state.recover_lock(self.backend, self.storage_run, owner_binding)
        self.assertEqual(recovered["remote_writes"], 0)
        self.assertEqual(read_json(self.storage_run / "state.json"), ledger)
        self.assertEqual(target_cli.status(self.backend, self.workspace)["next_action"], "resume-prepare")

        def resumed(_backend, target, *, storage_run, request_descriptor):
            target.mkdir()
            result = {"status": "fresh_creation_prepared", "request": request_descriptor}
            write_json(target / "prepare.json", result)
            return result

        with patch("devex_clone_target.resume_prepare_target", side_effect=resumed):
            self.assertTrue(target_cli.resume_prepare(self.backend, self.workspace)["resumed"])
        self.assertEqual(binding(self.workspace / "registration.json"), registration)
        self.assertEqual(binding(self.storage_run / "manifest.json"), manifest)
        self.assertEqual(target_cli.status(self.backend, self.workspace)["next_action"], "initialize")
        before = self.local_files()
        with patch.object(target_cli, "_workspace_control") as guard, self.assertRaisesRegex(ValueError, "已完整发布"):
            target_cli.resume_prepare(self.backend, self.workspace)
        guard.assert_not_called()
        self.assertEqual(self.local_files(), before)

    def local_files(self):
        return {path.relative_to(self.local).as_posix(): binding(path)
                for path in self.local.rglob("*") if path.is_file()}

    def test_resume_rejects_unknown_writes_before_acquiring_any_guard(self):
        with patch("devex_clone_target.prepare_target", side_effect=OSError("interrupted")), \
                self.assertRaises(OSError):
            target_cli.prepare(self.backend, self.workspace, self.request, self.environment, self.storage_run)
        target = self.workspace / "target"
        target.mkdir()
        for name in ("reset.intent.json", "sentinel.intent.json", "create-shared.intent.json", "unknown.json"):
            with self.subTest(name=name):
                write_json(target / name, {})
                before = self.local_files()
                with patch.object(target_cli, "_workspace_control") as guard, \
                        patch("devex_clone_target.resume_prepare_target") as operation, \
                        self.assertRaises(ValueError):
                    target_cli.resume_prepare(self.backend, self.workspace)
                guard.assert_not_called()
                operation.assert_not_called()
                self.assertEqual(self.local_files(), before)
                self.assertIsNone(target_cli.status(self.backend, self.workspace)["next_action"])
                (target / name).unlink()

    def test_invalid_registration_is_rejected_before_guard_creation(self):
        self.workspace.mkdir()
        write_json(self.workspace / "registration.json", {"kind": "invalid"})
        before = self.local_files()
        with patch.object(target_cli, "_workspace_control") as guard, self.assertRaises(ValueError):
            target_cli.resume_prepare(self.backend, self.workspace)
        guard.assert_not_called()
        self.assertEqual(self.local_files(), before)

    def test_status_does_not_offer_recovery_for_live_or_reused_controller_pid(self):
        self.prepare()
        lock = self.storage_run / "run.lock"
        lock.mkdir()
        owner = {"format_version": 1, "identity": {"pid": 10001, "started": "123", "executable": "fixture"},
                 "directory": str(self.storage_run),
                 "manifest_sha256": binding(self.storage_run / "manifest.json")["sha256"]}
        write_json(lock / "owner.json", owner)
        before = self.local_files()
        for observed in (owner["identity"], {**owner["identity"], "started": "456"}):
            with self.subTest(observed=observed), patch.object(run_state, "process_identity", return_value=observed):
                result = target_cli.status(self.backend, self.workspace)
            self.assertEqual(result["last_successful_stage"], "prepared")
            self.assertEqual(result["pending_stage"], "controller")
            self.assertIsNone(result["next_action"])
            self.assertFalse(result["controller"]["process_missing"])
            self.assertEqual(self.local_files(), before)

    def test_status_requires_valid_prepare_receipt_before_publication_resume(self):
        self.prepare()
        (self.workspace / "prepared-files.json").unlink()
        before = self.local_files()
        result = target_cli.status(self.backend, self.workspace)
        self.assertFalse(result["evidence_valid"])
        self.assertIsNone(result["next_action"])
        self.assertEqual(self.local_files(), before)

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
