"""fresh-target 统一入口的离线参数、绑定和环境隔离测试。"""
from __future__ import annotations

import json
import os
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from tests.workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import configured
from devex_clone_run_state import binding
import devex_clone_run_state as run_state
import devex_clone_run_cli as cli
import devex_clone_target_cli as target_cli


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
        write_json(target / "prepare.json", {"request": binding(request)})
        return {"status": "fresh_creation_prepared"}

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

    def test_initialize_and_verify_reopen_registration_and_never_replay(self):
        self.prepare()
        initialized = self.workspace / "target/initialized.json"

        def initialize(backend, target, *, storage_run):
            self.assertEqual(storage_run, self.storage_run)
            self.assertTrue((storage_run / "run.lock").is_dir())
            self.assertEqual(os.environ["APP_SCOPE_ID"], "fresh-fixture")
            write_json(initialized, {"status": "fresh_target_initialized"})
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

    def test_binding_drift_fails_after_stage_without_copying_environment(self):
        self.prepare()

        def drift(_backend, _target, *, storage_run):
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

        def initialize(_backend, target, *, storage_run):
            self.assertEqual(active, [(self.workspace, target_cli.WORKSPACE_GUARD)])
            write_json(target / "initialized.json", {"status": "fresh_target_initialized"})
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
            write_json(target / "prepare.json", {"request": request_descriptor})
            return {"status": "fresh_creation_prepared"}

        with patch("devex_clone_target.resume_prepare_target", side_effect=resumed):
            result = target_cli.resume_prepare(self.backend, self.workspace)
        self.assertTrue(result["resumed"])
        self.assertEqual(result["status"], "fresh_creation_prepared")
        self.assertTrue((self.workspace / "prepared-files.json").is_file())

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
            write_json(target / "prepare.json", {"request": request_descriptor})
            return {"status": "fresh_creation_prepared"}

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

        def drift(_backend, _target, *, storage_run):
            write_json(self.workspace / "target/initialized.json", {"status": "fresh_target_initialized"})
            (storage_run / "state.json").write_text('{"attempts":[{"status":"changed"}]}', encoding="utf-8")
            return {"status": "fresh_target_initialized"}

        with patch("devex_clone_target.initialize_target", side_effect=drift), self.assertRaises(ValueError):
            target_cli.initialize(self.backend, self.workspace)

    def test_even_valid_storage_append_during_stage_is_rejected(self):
        self.prepare()

        def drift(_backend, _target, *, storage_run):
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
