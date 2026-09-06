import sys
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from full_stack_rate_limit_config import (
    DEFAULT_API_WINDOW_SECS,
    DEFAULT_LOGIN_CAPACITY,
    LOGIN_RULE,
    login_budget_environment,
    login_rate_limit,
    rate_limit_authority,
)

ROOT = Path(__file__).resolve().parents[2]


class LoginRateLimitConfigTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=local)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.config = self.root / "config"
        self.config.mkdir()
        self.base = self.config / "app.toml"
        self.base.write_text(
            "[rate_limit]\napi_window_secs = 60\n"
            '[rate_limit.api_limits]\n"POST /api/v1/auth/login" = 5\n',
            encoding="utf-8",
        )
        self.env = {"APP_ENV": "test"}

    def test_current_config_and_default_contract(self):
        self.assertEqual(
            login_rate_limit(self.root, self.env), {"capacity": 5, "window_secs": 60}
        )
        self.base.write_text("[rate_limit]\n", encoding="utf-8")
        self.assertEqual(
            login_rate_limit(self.root, self.env), {"capacity": 5, "window_secs": 60}
        )

    def test_nested_environment_override_and_supported_env_override(self):
        (self.config / "app.test.toml").write_text(
            "[rate_limit]\napi_window_secs = 90\n[rate_limit.api_limits]\n"
            '"POST /api/v1/auth/login" = 7\n',
            encoding="utf-8",
        )
        self.assertEqual(
            login_rate_limit(self.root, self.env), {"capacity": 7, "window_secs": 90}
        )
        actual = login_rate_limit(
            self.root, {**self.env, "APP_RATE_LIMIT_API_WINDOW_SECS": "30"}
        )
        self.assertEqual(actual, {"capacity": 7, "window_secs": 30})
        actual = login_rate_limit(
            ROOT, {**self.env, "APP_CONFIG_DIR": str(self.config)}
        )
        self.assertEqual(actual, {"capacity": 7, "window_secs": 90})

    def test_missing_invalid_and_disabled_inputs_fail_closed(self):
        for changes in [
            {"APP_ENV": ""},
            {"APP_ENV": "prod"},
            {"APP_CONFIG_DIR": ""},
            {"APP_RATE_LIMIT_API_WINDOW_SECS": "0"},
            {"APP_RATE_LIMIT_API_WINDOW_SECS": " 60"},
            {"APP_RATE_LIMIT_API_WINDOW_SECS": "x"},
            {"APP_RATE_LIMIT_ENABLED": "false"},
            {"APP_RATE_LIMIT_ENABLED": "TRUE"},
        ]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                login_rate_limit(self.root, {**self.env, **changes})
        with self.assertRaises(ValueError):
            login_rate_limit(self.root, {})
        for content in [
            "[rate_limit]\napi_window_secs = 0",
            '[rate_limit]\napi_window_secs = "60"',
            '[rate_limit.api_limits]\n"POST /api/v1/auth/login" = 0',
            '[rate_limit.api_limits]\n"POST /api/v1/auth/login" = true',
            "[rate_limit]\nenabled = false",
            '[rate_limit]\napi_limits = "bad"',
        ]:
            self.base.write_text(content, encoding="utf-8")
            with self.subTest(content=content), self.assertRaises(ValueError):
                login_rate_limit(self.root, self.env)
        self.base.unlink()
        with self.assertRaises(FileNotFoundError):
            login_rate_limit(self.root, self.env)

    def test_export_only_selected_values_and_explicit_ledger_path(self):
        values = login_budget_environment(
            self.root,
            {**self.env, "APP_DATABASE_PASSWORD": "secret"},
            self.root / "budget.json",
        )
        self.assertEqual(len(values), 3)
        self.assertEqual(values["RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY"], "5")
        self.assertNotIn("secret", str(values))
        with self.assertRaises(ValueError):
            login_budget_environment(self.root, self.env, Path("relative.json"))

    def test_complete_authority_uses_all_current_overrides_without_credentials(self):
        actual = rate_limit_authority(
            self.root,
            {
                **self.env,
                "APP_DATABASE_PASSWORD": "secret",
                "APP_RATE_LIMIT_CAPACITY": "80",
                "APP_RATE_LIMIT_WINDOW_SECS": "45",
                "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT": "true",
                "APP_RATE_LIMIT_USER_CAPACITY": "12",
                "APP_RATE_LIMIT_USER_WINDOW_SECS": "30",
                "APP_RATE_LIMIT_API_WINDOW_SECS": "90",
            },
        )
        self.assertEqual(
            actual,
            {
                "format_version": 1,
                "global": {"capacity": 80, "window_ms": 45000},
                "user": {"enabled": True, "capacity": 12, "window_ms": 30000},
                "api": {"rules": {LOGIN_RULE: 5}, "window_ms": 90000},
                "login": {"capacity": 5, "window_ms": 90000},
            },
        )
        self.assertNotIn("secret", str(actual))
        for field, value in (
            ("APP_RATE_LIMIT_CAPACITY", "0"),
            ("APP_RATE_LIMIT_WINDOW_SECS", "false"),
            ("APP_RATE_LIMIT_USER_CAPACITY", "-1"),
            ("APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT", "yes"),
        ):
            with self.subTest(field=field), self.assertRaises(ValueError):
                rate_limit_authority(self.root, {**self.env, field: value})

    def test_current_global_user_and_csrf_authority(self):
        actual = rate_limit_authority(ROOT, self.env)
        self.assertEqual(actual["global"], {"capacity": 100, "window_ms": 60000})
        self.assertEqual(
            actual["user"], {"enabled": False, "capacity": 500, "window_ms": 60000}
        )
        self.assertEqual(actual["api"]["rules"]["GET /api/v1/auth/csrf"], 30)
        self.assertEqual(
            actual["api"]["rules"]["POST /api/v1/auth/password-reset/complete"], 3
        )
        self.assertEqual(actual["api"]["window_ms"], 60000)
        self.assertEqual(
            set(actual["api"]["rules"]),
            {
                LOGIN_RULE,
                "GET /api/v1/auth/csrf",
                "POST /api/v1/auth/password-reset/complete",
            },
        )

    def test_authority_cli_reads_only_explicit_local_configuration(self):
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("APP_")
        }
        environment.update({"APP_ENV": "test", "APP_CONFIG_DIR": str(self.config)})
        result = subprocess.run(
            [
                sys.executable,
                "-X",
                "utf8",
                str(ROOT / "scripts/full_stack_rate_limit_config.py"),
                "--authority",
            ],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout), rate_limit_authority(self.root, self.env)
        )

    def test_ci_shares_ledger_and_exports_real_config_before_start(self):
        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/extended-ci.yml").read_text(encoding="utf-8")
        )
        steps = workflow["jobs"]["full-stack-e2e"]["steps"]
        names = [step["name"] for step in steps]
        selected = {step["name"]: step for step in steps}
        setup = selected["配置全栈临时目录"]["run"]
        self.assertIn(
            "RYFRAME_E2E_LOGIN_BUDGET_STATE=$RUNNER_TEMP/ryframe-full-stack/login-budget.json",
            setup,
        )
        self.assertLess(
            names.index("构建并安全初始化临时全栈环境"),
            names.index("从当前隔离配置导出登录预算"),
        )
        self.assertLess(
            names.index("从当前隔离配置导出登录预算"),
            names.index("启动真实 API、Worker 并等待就绪"),
        )
        exporter = selected["从当前隔离配置导出登录预算"]
        self.assertEqual(exporter["working-directory"], "${{ matrix.backend }}")
        self.assertNotIn("if", exporter)
        self.assertIn(
            'full_stack_rate_limit_config.py --environment-file "$GITHUB_ENV"',
            exporter["run"],
        )

    def test_rust_configuration_and_login_guard_policy_cannot_drift_silently(self):
        config = (ROOT / "crates/ryframe-config/src/rate_limit_config.rs").read_text(
            encoding="utf-8"
        )
        self.assertRegex(
            config,
            rf"fn default_window\(\) -> u64\s*\{{\s*{DEFAULT_API_WINDOW_SECS}\s*\}}",
        )
        self.assertIn(
            '#[serde(default = "default_window")]\n    pub api_window_secs: u64', config
        )
        self.assertIn("pub api_limits: HashMap<String, u32>", config)
        guard = (
            ROOT / "crates/ryframe-api/src/handlers/auth_handler/guards.rs"
        ).read_text(encoding="utf-8")
        self.assertIn('format!("POST {}", api_path("auth/login"))', guard)
        self.assertIn(f".unwrap_or({DEFAULT_LOGIN_CAPACITY})", guard)
        self.assertIn("state.settings.rate_limit.api_window_secs.max(1)", guard)
        spec = (
            ROOT / "crates/ryframe-config/src/app_config/environment_overrides/spec.rs"
        ).read_text(encoding="utf-8")
        self.assertRegex(
            spec,
            r'"APP_RATE_LIMIT_API_WINDOW_SECS",\s*&\["rate_limit", "api_window_secs"\]',
        )
        self.assertNotIn('"APP_RATE_LIMIT_API_LIMITS"', spec)
        contract = (ROOT / "openapi/openapi.json").read_text(encoding="utf-8")
        self.assertIn('"/api/v1/auth/login"', contract)
        self.assertEqual(LOGIN_RULE, "POST /api/v1/auth/login")
        loader = (ROOT / "crates/ryframe-config/src/app_config/loader.rs").read_text(
            encoding="utf-8"
        )
        self.assertIn("merge_tables(base_table, env_table)", loader)
        self.assertIn("apply_env_overrides(&mut table)?", loader)


if __name__ == "__main__":
    unittest.main()
