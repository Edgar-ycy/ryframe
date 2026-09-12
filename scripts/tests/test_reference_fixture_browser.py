"""Device 浏览器绑定来源、预算与进程树均不可被隐式替换或重放。"""

from __future__ import annotations

from contextlib import ExitStack
from io import BytesIO
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_browser as browser
import reference_fixture_browser_process as browser_process
import reference_fixture_browser_publish as terminal_publish
import reference_fixture_browser_security as security
import reference_fixture_environment as fixture_environment
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
        self.private = {
            "APP_ENV": "test", "APP_SCOPE_ID": "fixture-source", "APP_CONFIG_DIR": str(self.execution),
            "APP_APP_HOST": "127.0.0.1", "APP_APP_PORT": "18200", "APP_JOBS_MODE": "external",
            "APP_CORS_ALLOW_ORIGINS": "http://127.0.0.1:14200",
            "APP_DATABASE_HOST": "127.0.0.1", "APP_DATABASE_PORT": "3306", "APP_DATABASE_NAME": "control",
            "APP_DATABASE_USERNAME": "root", "APP_DATABASE_PASSWORD": "Database!Secret123",
            "APP_DATABASE_TLS_MODE": "disabled", "APP_DB_PASSWORD": "Database!Secret123",
            "APP_TENANT_DATA_TARGETS": "[]", "APP_OBJECT_STORAGE_BACKEND": "rustfs",
            "APP_OBJECT_STORAGE_ENDPOINT": "http://127.0.0.1:29200", "APP_OBJECT_STORAGE_REGION": "us-east-1",
            "APP_OBJECT_STORAGE_USE_SSL": "false", "APP_OBJECT_STORAGE_ACCESS_KEY": "RustfsAccess123",
            "APP_OBJECT_STORAGE_SECRET_KEY": "Rustfs!Secret123", "APP_REDIS_HOST": "127.0.0.1",
            "APP_REDIS_PORT": "16390", "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false",
            "APP_REDIS_PASSWORD": "Redis!Secret123", "APP_JOBS_HEALTH_HOST": "127.0.0.1",
            "APP_JOBS_HEALTH_PORT": "19200", "APP_AUTH_JWT_SECRET": "Jwt!Secret123",
            "APP_MONITOR_METRICS_BEARER_TOKEN": "Metrics!Token123",
            "RYFRAME_RESET_ADMIN_PASSWORD": "Admin!Secret123",
            "TEMP": str(self.root / "tmp"), "TMP": str(self.root / "tmp"),
        }
        secrets = self.reference / "secrets"
        secrets.mkdir()
        secret_values = {
            "mysql-client.cnf": "[client]\npassword=Database!Secret123\n",
            "rustfs-access-key.txt": "RustfsAccess123", "rustfs-secret-key.txt": "Rustfs!Secret123",
            "redis-password.txt": "Redis!Secret123", "reset-admin-password.txt": "Admin!Secret123",
            "reset-user-password.txt": "User!Secret123", "jwt-secret.txt": "Jwt!Secret123",
            "metrics-token.txt": "Metrics!Token123",
        }
        for name, value in secret_values.items():
            (secrets / name).write_text(value, encoding="utf-8")
        write_json(self.bootstrap, {
            "format_version": 1, "kind": "reference-fixture-environment", "status": "prepared",
            "services_started": False, "remote_writes": 0, "historical_data_used": False,
            "execution_backend": str(self.execution),
            "environment_sha256": fixture_environment.plan_hash(self.private),
            "secret_files": {name: {"path": str(secrets / name), **runtime.file_digest(secrets / name)}
                             for name in fixture_environment.SECRET_FILES},
            "plan": {"side": "seed", "review": {"path": str(self.review),
                                                    **runtime.file_digest(self.review)}},
        })
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
        self.process_state = "running"
        state = lambda *_args: {"processes": {
            "api": self.process_state, "worker": self.process_state}}
        self.api = browser.RuntimeApi(
            runtime._bootstrap, runtime._output, Mock(return_value=verified),
            Mock(side_effect=state), Mock(side_effect=state),
        )

    def patches(self):
        stack = ExitStack()
        stack.enter_context(patch.object(browser, "verify_build_evidence", return_value={"source": self.source}))
        stack.enter_context(patch.object(browser, "rate_limit_settings", return_value={
            "capacity": 100, "window_secs": 60, "api_window_secs": 30,
            "api_limits": {"POST /api/v1/auth/login": 7},
        }))
        stack.enter_context(patch.object(security, "login_budget_environment", return_value={
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
        stack.enter_context(patch.object(security, "configured", side_effect=safe))
        return stack

    def bind(self, server="preview", run_id="r24-device"):
        binding = self.output / f"browser-binding-{run_id}.json"
        result = browser.bind_browser(
            self.api, self.backend, self.bootstrap, self.output, binding, run_id, server
        )
        return binding, result

    def browser_artifacts(self, server="preview", run_id="r24-device"):
        outputs = browser.browser_outputs(self.frontend, self.output, run_id, server)
        outputs["report"].mkdir(parents=True)
        (outputs["report"] / "index.html").write_text("report", encoding="utf-8")
        outputs["results"].mkdir(parents=True)
        write_json(outputs["results"] / "device-tests.json", {
            "format_version": 1, "kind": "device-browser-tests", "fixture": "device",
            "server": server, "run_id": run_id, "status": "passed",
            "runs": [
                {"title": ["真实 Device 数据从 shared-control 复制校验并切换到 shared"],
                 "status": "passed", "retry": 0, "scenarios": ["shared-migration"]},
                {"title": ["真实 Device 数据从 dedicated-a 复制校验并切换到 dedicated-b"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["dedicated-migration", "retention"]},
                {"title": ["真实排队 Device 迁移取消恢复源数据，并允许再次迁移"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["cancellation"]},
                {"title": ["真实 Device 复制阻塞时 Worker 崩溃，重启后同一迁移恢复并完成校验"],
                 "status": "passed", "retry": 0,
                 "scenarios": ["crash-recovery"]},
            ],
        })
        if server == "preview":
            files = []
            for path, destination in (("/login", "document"), ("/assets/app.js", "script")):
                target = self.frontend / ("dist/index.html" if path == "/login" else "dist/assets/app.js")
                files.append({"sequence": len(files) + 1, "method": "GET", "path": path,
                              "destination": destination, "status": 200,
                              **runtime.file_digest(target), "representation": "identity"})
            write_json(outputs["response_audit"], {
                "format_version": 1, "kind": "device-preview-static-responses",
                "status": "complete", "run_id": run_id, "scope_id": "fixture-source",
                "limits": {"entries": 10_000, "bytes": 8 * 1024 * 1024 * 1024},
                "total_entries": len(files), "total_bytes": sum(item["bytes"] for item in files),
                "entries": files,
            })
        now = int(time.time() * 1000)
        write_json(outputs["login_budget"], {
            "version": 1,
            "binding": {"scope": "fixture-source", "capacity": 7, "windowMs": 30_000},
            "observedAt": now,
            "buckets": {"principal:" + "a" * 64: {
                "generation": "00000000-0000-0000-0000-000000000000", "count": 1,
                "reservedAt": now, "completedAt": now,
            }},
        })
        return outputs

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
        self.assertEqual(value["commands"][0], ["build", "--real"])
        self.assertEqual(value["server"], "preview")
        self.assertEqual(value["login_budget"]["initial_state"], "absent")
        self.assertFalse(Path(value["login_budget"]["path"]).exists())

    def test_binding_rejects_every_unregistered_run_output(self):
        unknown = self.output / "browser-r24-device-unknown.json"
        unknown.write_text("{}", encoding="utf-8")
        with self.patches(), self.assertRaisesRegex(ValueError, "未登记"):
            self.bind()
        self.assertFalse((self.output / "browser-binding-r24-device.json").exists())

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
                (frontend / "dist/index.html").write_text("built", encoding="utf-8")
                (frontend / "dist/assets").mkdir()
                (frontend / "dist/assets/app.js").write_text("script", encoding="utf-8")
            elif arguments[0] == "check":
                self.browser_artifacts()
            return {"directory": str(process_dir), "completion": {"sha256": "d" * 64}}

        receipt = Mock(path=self.frontend / "dist/.vite/restore-build.json")
        with self.patches(), patch.object(browser, "_frontend_command", side_effect=execute), \
                patch.object(browser, "validate_frontend_build", return_value=({}, receipt)), \
                patch.object(browser, "verify_browser_evidence", return_value={}):
            binding, _ = self.bind()
            result = browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertEqual([call[0] for call in calls], [
            ["build", "--real"],
            ["exec", "node", "scripts/restore-build.mjs", "verify", "--source-root", str(self.frontend)],
            ["check", "--stage", "browser", "--real", "--fixture", "device", "--server", "preview"],
            ["exec", "node", "scripts/restore-build.mjs", "verify", "--source-root", str(self.frontend)],
        ])
        self.assertNotIn("RYFRAME_E2E_PASSWORD", calls[0][1])
        self.assertEqual(calls[0][1]["VITE_APP_API_ORIGIN"], "")
        self.assertEqual(calls[0][1]["VITE_APP_PROXY_TARGET"], "http://127.0.0.1:18200")
        self.assertEqual(calls[2][1]["RYFRAME_E2E_PASSWORD"], "Admin!Secret123")
        self.assertEqual(calls[2][1]["RYFRAME_E2E_RUNTIME_DIR"], str(self.output))
        self.assertEqual(calls[2][1]["RYFRAME_E2E_PREVIEW_RESPONSE_AUDIT"],
                         str(self.output / "browser-r24-device-preview-responses.json"))
        self.assertEqual(calls[2][1]["RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY"], "7")
        self.assertEqual(result["status"], "passed")
        self.assertNotIn("Secret123", json.dumps(result))
        self.assertNotIn("RustfsAccess123", json.dumps(result))

    def test_dev_mode_uses_an_independent_run_without_building_dist(self):
        calls = []

        def execute(_binding, _frontend, arguments, _environment, log, process_dir, _timeout, _secrets):
            calls.append(arguments)
            process_dir.mkdir()
            log.write_text("ok\n", encoding="utf-8")
            self.browser_artifacts("dev", "r24-device-dev")
            return {"directory": str(process_dir), "completion": {"sha256": "d" * 64}}

        with self.patches(), patch.object(browser, "_frontend_command", side_effect=execute), \
                patch.object(browser, "verify_browser_evidence", return_value={}):
            binding, _ = self.bind("dev", "r24-device-dev")
            result = browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertEqual(calls, [["check", "--stage", "browser", "--real", "--fixture",
                                  "device", "--server", "dev"]])
        self.assertIsNone(result["build"])
        self.assertFalse((self.frontend / "dist").exists())

    def test_browser_environment_is_explicit_and_redacts_every_bound_secret(self):
        with self.patches():
            binding, context = browser._plan(
                self.api, self.backend, self.bootstrap, self.output, "r24-device", "preview",
                require_fresh=True)
            values = security.browser_environment(context["private"], binding)
        self.assertEqual(values["APP_OBJECT_STORAGE_ACCESS_KEY"], "RustfsAccess123")
        self.assertEqual(values["RYFRAME_E2E_PASSWORD"], "Admin!Secret123")
        self.assertNotIn("RYFRAME_RESET_USER_PASSWORD", values)
        self.assertNotIn("APP_RESET_LEGACY_MYSQL_EXCLUSIVE", values)
        self.assertIn("RustfsAccess123", context["secrets"])
        self.assertIn("Database!Secret123", context["secrets"])

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

    def test_late_artifact_failure_precedes_success_publication_and_blocks_restored_result(self):
        def execute(_binding, _frontend, _arguments, _environment, log, process_dir,
                    _timeout, _secrets):
            process_dir.mkdir()
            log.write_text("ok\n", encoding="utf-8")
            self.browser_artifacts("dev", "r24-device-dev")
            return {"directory": str(process_dir)}

        guard = Mock(spec=["assert_unchanged"])
        guard.assert_unchanged.side_effect = ValueError("late artifact drift")
        outputs = browser.browser_outputs(self.frontend, self.output, "r24-device-dev", "dev")
        with self.patches(), patch.object(browser, "_frontend_command", side_effect=execute), \
                patch.object(browser, "_browser_artifacts", return_value=({
                    "report": {}, "results": {}, "tests": {}, "responses": None}, (guard,))), \
                patch.object(browser, "verify_browser_evidence", return_value={}):
            binding, _ = self.bind("dev", "r24-device-dev")
            with self.assertRaisesRegex(ValueError, "late artifact drift"):
                browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertFalse(outputs["result"].exists())
        self.assertTrue(outputs["failure"].is_file())
        write_json(outputs["result"], {"status": "restored"})
        with self.assertRaisesRegex(ValueError, "没有唯一成功结果"):
            browser.verify_browser_result({}, {"outputs": outputs}, binding)

    def test_terminal_result_interrupted_before_unlink_is_rejected_as_unknown(self):
        target = self.output / "browser-r24-terminal-result.json"
        failure = Mock()
        with patch.object(terminal_publish, "_unlink_pending", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                terminal_publish.publish_terminal_success(
                    self.output, target, {"status": "passed"}, failure
                )
        self.assertTrue(target.is_file())
        self.assertEqual(len(list(self.output.glob(target.name + ".pending-*"))), 1)
        failure.assert_not_called()
        with self.assertRaisesRegex(ValueError, "未登记"):
            browser._assert_output_set(
                self.output, "r24-terminal", {"result": target}, fresh=False
            )

    def test_terminal_unlink_and_failure_publication_failure_still_blocks_consumer(self):
        def execute(_binding, _frontend, _arguments, _environment, log, process_dir,
                    _timeout, _secrets):
            process_dir.mkdir()
            log.write_text("ok\n", encoding="utf-8")
            self.browser_artifacts("dev", "r24-device-dev")
            return {"directory": str(process_dir)}

        with self.patches():
            binding, _ = self.bind("dev", "r24-device-dev")
        outputs = browser.browser_outputs(self.frontend, self.output, "r24-device-dev", "dev")
        publish = browser._publish_json

        def failure_unavailable(path, value):
            if path == outputs["failure"]:
                raise OSError("failure receipt unavailable")
            publish(path, value)

        with self.patches(), patch.object(browser, "_frontend_command", side_effect=execute), \
                patch.object(browser, "verify_browser_evidence", return_value={}), \
                patch.object(browser, "_publish_json", side_effect=failure_unavailable), \
                patch.object(terminal_publish, "_unlink_pending", side_effect=OSError("unlink failed")):
            with self.assertRaisesRegex(RuntimeError, "失败证据保存失败"):
                browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        self.assertTrue(outputs["result"].is_file())
        self.assertFalse(outputs["failure"].exists())
        self.assertEqual(len(list(self.output.glob(
            "browser-r24-device-dev-result.json.pending-*"))), 1)
        with self.patches(), self.assertRaisesRegex(ValueError, "未登记"):
            browser.verify_browser(
                self.api, self.backend, self.bootstrap, self.output, binding, closed=False
            )

    def test_terminal_unlink_completion_is_the_last_commit_point(self):
        def execute(_binding, _frontend, _arguments, _environment, log, process_dir,
                    _timeout, _secrets):
            process_dir.mkdir()
            log.write_text("ok\n", encoding="utf-8")
            self.browser_artifacts("dev", "r24-commit")
            return {"directory": str(process_dir)}

        def committed_then_exit(path):
            path.unlink()
            raise SystemExit(99)

        with self.patches(), patch.object(browser, "_frontend_command", side_effect=execute), \
                patch.object(browser, "verify_browser_evidence", return_value={}), \
                patch.object(terminal_publish, "_unlink_pending", side_effect=committed_then_exit):
            binding, _ = self.bind("dev", "r24-commit")
            with self.assertRaisesRegex(SystemExit, "99"):
                browser.run_browser(self.api, self.backend, self.bootstrap, self.output, binding)
        outputs = browser.browser_outputs(
            self.frontend, self.output, "r24-commit", "dev"
        )
        self.assertTrue(outputs["result"].is_file())
        self.assertFalse(outputs["failure"].exists())
        self.assertFalse(list(self.output.glob(
            "browser-r24-commit-result.json.pending-*")))
        with self.patches(), patch.object(browser, "verify_browser_result", return_value={"status": "passed"}):
            verified = browser.verify_browser(
                self.api, self.backend, self.bootstrap, self.output, binding, closed=False
            )
        self.assertEqual(verified["status"], "reference_fixture_browser_verified")

    def test_terminal_concurrent_target_creation_keeps_pending_and_fails_closed(self):
        target = self.output / "browser-r24-concurrent-result.json"
        failure = Mock()

        def concurrent(_source, destination):
            Path(destination).write_text("concurrent\n", encoding="utf-8")
            raise FileExistsError(destination)

        with patch.object(terminal_publish.os, "link", side_effect=concurrent):
            with self.assertRaisesRegex(ValueError, "并发创建"):
                terminal_publish.publish_terminal_success(
                    self.output, target, {"status": "passed"}, failure
                )
        failure.assert_called_once()
        self.assertEqual(target.read_text(encoding="utf-8"), "concurrent\n")
        self.assertEqual(len(list(self.output.glob(target.name + ".pending-*"))), 1)
        with self.assertRaisesRegex(ValueError, "未登记"):
            browser._assert_output_set(
                self.output, "r24-concurrent", {"result": target}, fresh=False
            )

    def test_verify_and_close_are_read_only_and_require_the_expected_runtime_state(self):
        with self.patches(), patch.object(browser, "verify_browser_result", return_value={}) as verify:
            binding, _ = self.bind("dev", "r24-device-dev")
            write_json(self.output / "browser-r24-device-dev-result.json", {"status": "fixture"})
            control_count = self.api.control.call_count
            before = {path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
            result = browser.verify_browser(
                self.api, self.backend, self.bootstrap, self.output, binding, closed=False
            )
            self.process_state = "stopped"
            closed = browser.verify_browser(
                self.api, self.backend, self.bootstrap, self.output, binding, closed=True
            )
            after = {path: path.read_bytes() for path in self.output.rglob("*") if path.is_file()}
        self.assertEqual(before, after)
        self.assertEqual(result["status"], "reference_fixture_browser_verified")
        self.assertEqual(closed["status"], "reference_fixture_browser_closed")
        self.assertEqual(verify.call_count, 2)
        self.assertEqual(self.api.control.call_count, control_count)
        self.assertEqual(self.api.observe.call_count, 2)

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
        process.supervisor.stdout = BytesIO(b"Admin!Secret123\nRustfsAccess123\n")
        process.wait.side_effect = [KeyboardInterrupt(), 137]
        binding = {"scope_id": "fixture-source", "tools": {
            "corepack": {"path": str(self.tools["corepack.cmd"])},
            "launcher": {"path": str(self.tools["launcher.exe"])}},
        }
        log = self.output / "interrupt.log"
        process_dir = self.output / "interrupt-process"

        with patch.object(browser_process, "launch_supervised_process", return_value=process) as started, \
                patch.object(browser_process, "terminate_owned_process_tree") as terminated, \
                patch.object(browser_process, "completion_binding", return_value={"sha256": "e" * 64}):
            with self.assertRaises(KeyboardInterrupt):
                browser_process.run_frontend_command(
                    binding, self.frontend, ["check"], {}, log, process_dir,
                    60, ("Admin!Secret123", "RustfsAccess123")
                )
        self.assertTrue(Path(started.call_args.args[3][0]).is_absolute())
        terminated.assert_called_once_with(process.tree, crash=True)
        self.assertNotIn("Secret123", log.read_text(encoding="utf-8"))
        self.assertNotIn("RustfsAccess123", log.read_text(encoding="utf-8"))
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
        tree = {"scope_id": "fixture-source", "process": {"pid": 123}}
        process.tree = tree
        document = Mock(unsafe=True)
        document.path = process_dir / "frontend.json"
        document.assert_unchanged.return_value = None
        with patch.object(browser_process, "launch_supervised_process", side_effect=launch), \
                patch.object(browser_process, "read_process_tree", return_value=tree), \
                patch.object(browser_process, "process_document",
                             return_value=(document, tree["process"])), \
                patch.object(browser_process, "completion_binding", return_value=completion):
            evidence = browser_process.run_frontend_command(
                binding, self.frontend, ["build", "--real"], {}, self.output / "success.log",
                process_dir, 60, (),
            )
        self.assertEqual(evidence["completion"], completion)
        self.assertEqual(set(evidence), {"directory", "process", "tree", "completion"})
        self.assertEqual((self.output / "success.log").read_text(encoding="utf-8"), "completed\n")

    def test_failure_evidence_uses_tree_result_and_members_completion_names(self):
        directory = self.output / "failed-process"
        directory.mkdir()
        write_json(directory / "frontend-tree.json", {"operation_id": "operation-1"})
        write_json(directory / "frontend-tree-operation-1-result.json", {"status": "failed"})
        write_json(directory / "frontend-members-operation-1-stopped.json", {"status": "stopped"})
        with patch.object(browser_process, "process_evidence", side_effect=ValueError("incomplete")):
            result = browser_process.failure_process(directory, "fixture-source")
        self.assertEqual(set(result), {"directory", "tree", "tree_result", "completion"})
        self.assertTrue(result["tree_result"]["path"].endswith("-result.json"))
        self.assertTrue(result["completion"]["path"].endswith("-stopped.json"))

    def test_failure_collects_all_completed_processes_and_logs(self):
        outputs = browser.browser_outputs(self.frontend, self.output, "r24-device", "preview")
        completed = {"build": {"completion": "build"},
                     "build_verify_before": {"completion": "before"}}
        for name in ("build", "build_verify_before", "browser"):
            outputs[name + "_process"].mkdir()
            outputs[name + "_log"].write_text(name, encoding="utf-8")
        captured_browser = {"directory": str(outputs["browser_process"])}
        with patch.object(browser, "_failure_process", side_effect=lambda path, _scope: {
                "directory": str(path)}) as capture:
            processes, logs = browser._failure_evidence(
                outputs, "preview", "fixture-source", completed
            )
        self.assertEqual(set(processes), {"build", "build_verify_before", "browser"})
        self.assertEqual(processes["browser"], captured_browser)
        self.assertEqual(set(logs), {"build", "build_verify_before", "browser"})
        capture.assert_called_once_with(outputs["browser_process"], "fixture-source")


def read(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))
