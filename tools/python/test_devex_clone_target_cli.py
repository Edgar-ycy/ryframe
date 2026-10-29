"""fresh-target 统一入口的离线参数、绑定和环境隔离测试。"""
from __future__ import annotations

from contextlib import nullcontext
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
        bindings = patch("devex_clone_target_binding.execution_binary_bindings", return_value=[])
        bindings.start()
        self.addCleanup(bindings.stop)
        protector = patch.object(target_cli, "protect_binaries", return_value=nullcontext())
        protector.start()
        self.addCleanup(protector.stop)

    def args(self, operation, *, request=None, environment=None, storage_run=None,
             observation=None, write=True):
        return SimpleNamespace(command="fresh-target", workspace=self.workspace, operation=operation,
                               request=request, environment=environment, storage_run=storage_run,
                               observation_dir=observation, write=write)


    def prepare(self):
        """锁、历史和路径隔离场景只构造登记前像，不发布prepare成功证明。"""
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


    def test_configured_filters_case_insensitively_and_private_values_win(self):
        result = configured({"TEMP": "private-temp", "APP_ENV": "test"}, {
            "Path": "tool-path", "temp": "ambient-temp", "app_env": "prod", "cargo_home": "unsafe",
            "https_proxy": "unsafe", "CUSTOM_PROXY": "unsafe", "safe": "kept"})
        self.assertEqual(result, {"Path": "tool-path", "TEMP": "private-temp", "APP_ENV": "test"})

    def test_dispatch_enforces_exact_stage_arguments_and_write_boundary(self):
        self.prepare()
        reconcile = self.args("reconcile-preflight")
        with patch.object(target_cli, "reconcile_preflight", return_value={"status": "preflight_failure_reconciled"}):
            self.assertEqual(cli.dispatch(reconcile, self.backend)["status"], "preflight_failure_reconciled")
        for operation in ("prepare", "resume-prepare", "status", "initialize", "resume-initialize", "verify"):
            with self.assertRaisesRegex(ValueError, "正式 cargo xtask"):
                cli.dispatch(self.args(operation), self.backend)
        invalid = [self.args("reconcile-preflight", write=False),
                   self.args("reconcile-preflight", request=self.request),
                   self.args("reconcile-preflight", environment=self.environment),
                   self.args("reconcile-preflight", storage_run=self.storage_run),
                   self.args("reconcile-preflight", observation=self.local / "unused")]
        for args in invalid:
            with self.subTest(operation=args.operation), self.assertRaises(ValueError):
                cli.dispatch(args, self.backend)

    def test_dispatch_resolves_relative_fresh_target_workspace(self):
        self.prepare()
        reconcile = self.args("reconcile-preflight")
        reconcile.workspace = Path(".local-tests/fresh-workspace")
        with patch.object(target_cli, "reconcile_preflight", return_value={"status": "preflight_failure_reconciled"}) as execute:
            self.assertEqual(cli.dispatch(reconcile, self.backend)["status"], "preflight_failure_reconciled")
        execute.assert_called_once_with(self.backend, self.workspace)


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


if __name__ == "__main__":
    unittest.main()
