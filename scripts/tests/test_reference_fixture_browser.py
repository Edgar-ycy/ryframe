"""Device 浏览器绑定来源、预算与进程树均不可被隐式替换或重放。"""

from __future__ import annotations

from contextlib import ExitStack
from io import BytesIO
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_browser as browser
import reference_fixture_runtime as runtime
from devex_clone_capture import write_json
from workspace_directory import WorkspaceDirectory


class StableGuard:
    def assert_unchanged(self):
        return None


class ReferenceFixtureBrowserTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests", prefix="reference-browser-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.execution = self.root / "device"
        self.reference = self.execution / ".local-tests/reference-fixture"
        self.reference.mkdir(parents=True)
        (self.execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        self.frontend = self.root / "device-frontend"
        self.frontend.mkdir()
        (self.frontend / "package.json").write_text("{}", encoding="utf-8")
        self.environment = self.root / "environment"
        self.environment.mkdir()
        self.tools = {}
        for name in ("mysql.exe", "corepack.cmd", "launcher.exe", "python.exe"):
            path = self.root / name
            path.write_bytes(name.encode())
            self.tools[name] = path
        self.review = self.root / "review.json"
        write_json(self.review, {
            "tools": {"mysql": {"path": str(self.tools["mysql.exe"]),
                                **runtime.file_digest(self.tools["mysql.exe"])}},
            "scopes": {"seed": {"scope_id": "fixture-source",
                                 "api_url": "http://127.0.0.1:18200",
                                 "frontend_url": "http://127.0.0.1:14200"}},
        })
        self.bootstrap = self.environment / "bootstrap.json"
        write_json(self.bootstrap, {
            "format_version": 1, "kind": "reference-fixture-environment", "status": "prepared",
            "services_started": False, "remote_writes": 0, "execution_backend": str(self.execution),
            "plan": {"side": "seed", "review": {"path": str(self.review),
                                                    **runtime.file_digest(self.review)}},
        })
        self.private = {
            "APP_ENV": "test", "APP_SCOPE_ID": "fixture-source", "APP_CONFIG_DIR": str(self.execution),
            "APP_APP_HOST": "127.0.0.1", "APP_APP_PORT": "18200", "APP_JOBS_MODE": "external",
            "APP_CORS_ALLOW_ORIGINS": "http://127.0.0.1:14200",
            "APP_DATABASE_PASSWORD": "Database!Secret123",
            "RYFRAME_RESET_ADMIN_PASSWORD": "Admin!Secret123",
            "TEMP": str(self.root / "tmp"), "TMP": str(self.root / "tmp"),
        }
        write_json(self.environment / "environment.json", {"environment": self.private})
        self.output = self.reference / "runtime-r1"
        self.output.mkdir()
        for name in ("runtime.json", "source-pair.json", "backend-build.json"):
            write_json(self.output / name, {"name": name})
        self.source = {
            "fixture": "device", "roots": {"backend": str(self.execution), "frontend": str(self.frontend)},
            "original": {name: {"head": "a" * 40, "patch_sha256": "b" * 64, "files": []}
                         for name in ("backend", "frontend")},
            "generated": {name: {"head": "a" * 40, "patch_sha256": "c" * 64, "files": []}
                          for name in ("backend", "frontend")},
        }
        verified = {"runtime": {"path": str(self.output / "runtime.json"),
                                **runtime.file_digest(self.output / "runtime.json")},
                    "scope_id": "fixture-source", "remote_writes": 0}
        self.api = browser.RuntimeApi(runtime._bootstrap, runtime._output, Mock(return_value=verified),
                                      Mock(return_value={"processes": {"api": "running", "worker": "running"}}))

    def patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(browser, "verify_build_evidence", return_value={"source": self.source}))
        stack.enter_context(patch.object(browser, "rate_limit_settings", return_value={
            "capacity": 100, "window_secs": 60, "api_window_secs": 30,
            "api_limits": {"POST /api/v1/auth/login": 7},
        }))
        stack.enter_context(patch.object(browser, "login_budget_environment", return_value={
            "RYFRAME_E2E_LOGIN_BUDGET_STATE": str(self.output / "browser-r24-device-login-budget.json"),
            "RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY": "7",
            "RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS": "30",
        }))
        stack.enter_context(patch.object(browser.SourceGuard, "capture", return_value=StableGuard()))
        stack.enter_context(patch.object(browser, "require_closed_port"))
        stack.enter_context(patch.object(browser.shutil, "which", return_value=str(self.tools["corepack.cmd"])))
        stack.enter_context(patch.object(browser.sys, "executable", str(self.tools["python.exe"])))

        def safe(values):
            return {"PATH": str(self.root), "COMSPEC": str(self.tools["launcher.exe"]), **values}

        stack.enter_context(patch.object(browser, "configured", side_effect=safe))
        return stack

    def bind(self):
        binding = self.output / "browser-binding-r24-device.json"
        result = browser.bind_browser(
            self.api, self.backend, self.bootstrap, self.output, binding, "r24-device"
        )
        return binding, result

    def test_binding_is_create_new_read_only_and_contains_no_credentials(self):
        with self.patches():
            binding, result = self.bind()
            probes = self.api.control.call_count
            with self.assertRaisesRegex(ValueError, "明确文件"):
                self.bind()
            self.assertEqual(self.api.control.call_count, probes)
        text = binding.read_text(encoding="utf-8")
        self.assertEqual(result["status"], "reference_fixture_browser_bound")
        self.assertNotIn("Admin!Secret123", text)
        self.assertNotIn("Database!Secret123", text)
        value = json.loads(text)
        self.assertEqual(value["commands"][0], "corepack pnpm build --real")
        self.assertEqual(value["login_budget"]["initial_state"], "absent")
        self.assertFalse(Path(value["login_budget"]["path"]).exists())

    def test_real_build_precedes_device_browser_and_binds_same_origin(self):
        calls = []

        def execute(_binding, frontend, arguments, environment, log, process_dir, _timeout, _secrets):
            calls.append((arguments, dict(environment)))
            process_dir.mkdir()
            log.write_text("ok\n", encoding="utf-8")
            if arguments[0] == "build":
                receipt = frontend / "dist/.vite/restore-build.json"
                receipt.parent.mkdir(parents=True)
                receipt.write_text("{}\n", encoding="utf-8")
            else:
                outputs = browser.browser_outputs(frontend, self.output, "r24-device")
                outputs["report"].parent.mkdir(parents=True)
                outputs["report"].write_text("ok", encoding="utf-8")
                outputs["results"].mkdir(parents=True)
                write_json(outputs["login_budget"], {"scope": "fixture-source"})
            return {"directory": str(process_dir), "completion": {"sha256": "d" * 64}}

        with self.patches(), patch.object(browser, "_frontend_command", side_effect=execute):
            binding, _ = self.bind()
            result = browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertEqual([call[0] for call in calls], [
            ["build", "--real"],
            ["check", "--stage", "browser", "--real", "--fixture", "device", "--server", "preview"],
        ])
        self.assertNotIn("RYFRAME_E2E_PASSWORD", calls[0][1])
        self.assertEqual(calls[0][1]["VITE_APP_API_ORIGIN"], "")
        self.assertEqual(calls[0][1]["VITE_APP_PROXY_TARGET"], "http://127.0.0.1:18200")
        self.assertEqual(calls[1][1]["RYFRAME_E2E_PASSWORD"], "Admin!Secret123")
        self.assertEqual(calls[1][1]["RYFRAME_E2E_RUNTIME_DIR"], str(self.output))
        self.assertEqual(calls[1][1]["RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY"], "7")
        self.assertEqual(result["status"], "passed")
        self.assertNotIn("Secret123", json.dumps(result))

    def test_failure_preserves_exit_code_blocks_replay_and_does_not_publish_success(self):
        error = subprocess.CalledProcessError(19, ["corepack", "pnpm", "build", "--real"])
        execute = Mock(side_effect=error)
        with self.patches(), patch.object(browser, "_frontend_command", execute):
            binding, _ = self.bind()
            with self.assertRaises(subprocess.CalledProcessError) as raised:
                browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
            with self.assertRaisesRegex(ValueError, "禁止重放"):
                browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertEqual(raised.exception.returncode, 19)
        failure = read(self.output / "browser-r24-device-failure.json")
        self.assertEqual((failure["stage"], failure["returncode"]), ("build", 19))
        self.assertFalse(failure["unknown_business_writes"])
        self.assertEqual(execute.call_count, 1)
        self.assertFalse((self.output / "browser-r24-device-result.json").exists())

    def test_binding_rejects_port_or_review_source_change_before_intent(self):
        with self.patches():
            binding, _ = self.bind()
            review = read(self.review)
            review["scopes"]["seed"]["frontend_url"] = "http://127.0.0.1:14201"
            self.review.unlink()
            write_json(self.review, review)
            with self.assertRaisesRegex(ValueError, "摘要已变化"):
                browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertFalse((self.output / "browser-r24-device-intent.json").exists())

    def test_binding_rejects_an_occupied_reviewed_frontend_port(self):
        binding = self.output / "browser-binding-r24-device.json"
        with self.patches(), patch.object(browser, "require_closed_port",
                                          side_effect=ValueError("port occupied")):
            with self.assertRaisesRegex(ValueError, "port occupied"):
                self.bind()
        self.assertFalse(binding.exists())

    def test_source_guard_detects_a_to_b_to_a_replacement(self):
        source = self.execution / "source.txt"
        source.write_text("A", encoding="utf-8")
        state = browser.file_state(source.stat())
        inventory = {"source": {"snapshot": {"head": "a" * 40, "patch_sha256": "b" * 64,
                                                "files": [], "clean": False}},
                     "files": [{"path": "source.txt", "sha256": "c" * 64}], "guard": {}}
        expected = {key: value for key, value in inventory["source"]["snapshot"].items() if key != "clean"}
        with patch.object(browser, "capture_inventory", return_value=inventory), \
                patch.object(browser, "source_file", return_value=(source, source.stat())):
            guard = browser.SourceGuard.capture(self.execution, expected)
        source.write_text("B", encoding="utf-8")
        source.write_text("A", encoding="utf-8")
        os.utime(source, ns=(state[3] + 1_000_000_000, state[3] + 1_000_000_000))
        with patch.object(browser, "source_file", return_value=(source, source.stat())):
            with self.assertRaisesRegex(ValueError, "曾被替换"):
                guard.assert_unchanged()

    def test_interruption_reclaims_supervised_tree_and_redacts_password_log(self):
        process = Mock()
        process.tree = {"scope_id": "fixture-source"}
        process.supervisor.stdout = BytesIO(b"Admin!Secret123\n")
        process.wait.side_effect = [KeyboardInterrupt(), 137]
        binding = {"scope_id": "fixture-source", "tools": {
            "corepack": {"path": str(self.tools["corepack.cmd"])},
            "launcher": {"path": str(self.tools["launcher.exe"])}},
        }
        log = self.output / "interrupt.log"
        process_dir = self.output / "interrupt-process"

        with patch.object(browser, "launch_supervised_process", return_value=process) as started, \
                patch.object(browser, "terminate_owned_process_tree") as terminated, \
                patch.object(browser, "completion_binding", return_value={"sha256": "e" * 64}):
            with self.assertRaises(KeyboardInterrupt):
                browser._frontend_command(binding, self.frontend, ["check"], {}, log, process_dir,
                                          60, ("Admin!Secret123",))
        self.assertTrue(Path(started.call_args.args[3][0]).is_absolute())
        terminated.assert_called_once_with(process.tree, crash=True)
        self.assertNotIn("Secret123", log.read_text(encoding="utf-8"))
        self.assertIn("[REDACTED]", log.read_text(encoding="utf-8"))

    def test_successful_frontend_command_binds_tree_and_completion_receipts(self):
        process_dir = self.output / "success-process"
        process = Mock()
        process.tree = {"scope_id": "fixture-source"}
        process.supervisor.stdout = BytesIO(b"completed\n")
        process.wait.return_value = 0
        binding = {"scope_id": "fixture-source", "tools": {
            "corepack": {"path": str(self.tools["corepack.cmd"])},
            "launcher": {"path": str(self.tools["launcher.exe"])}},
        }

        def launch(*_args, **_kwargs):
            write_json(process_dir / "frontend.json", {"process": "frontend"})
            write_json(process_dir / "frontend-tree.json", {"tree": "frontend"})
            return process

        completion = {"path": str(process_dir / "stopped.json"), "bytes": 1, "sha256": "e" * 64}
        with patch.object(browser, "launch_supervised_process", side_effect=launch), \
                patch.object(browser, "completion_binding", return_value=completion):
            evidence = browser._frontend_command(
                binding, self.frontend, ["build", "--real"], {}, self.output / "success.log",
                process_dir, 60, (),
            )
        self.assertEqual(evidence["completion"], completion)
        self.assertEqual(set(evidence), {"directory", "process", "tree", "completion"})
        self.assertEqual((self.output / "success.log").read_text(encoding="utf-8"), "completed\n")


def read(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))
