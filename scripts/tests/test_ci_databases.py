import json
import os
import shutil
import sys
import tomllib
import unittest
import uuid
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from ci_full_stack_databases import prepare_databases

ROOT = Path(__file__).resolve().parents[2]
TEMP = ROOT / ".local-tests/python-unit"


class CiDatabaseTests(unittest.TestCase):
    def temporary_root(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        root = TEMP / f"ci-databases-{uuid.uuid4().hex}"
        root.mkdir()
        self.addCleanup(shutil.rmtree, root)
        return root

    def environment(self, root):
        return {"GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "42", "GITHUB_RUN_ATTEMPT": "3",
                "GITHUB_ENV": str(root / "environment"), "APP_ENV": "test", "APP_SCOPE_ID": "ci-42-3",
                "APP_DATABASE_HOST": "127.0.0.1", "APP_DATABASE_PORT": "3306",
                "APP_DATABASE_NAME": "ryframe_e2e_42_3", "APP_DATABASE_USERNAME": "root",
                "APP_DATABASE_PASSWORD": "unit-test-only", "APP_DATABASE_TLS_MODE": "disabled",
                "RYFRAME_CI_MYSQL_CONTAINER_ID": "a" * 64}

    def test_targets_are_exact_new_resources_and_credentials_remain_in_environment(self):
        root = self.temporary_root()
        environment = self.environment(root)
        run = Mock()
        with patch.dict(os.environ, environment, clear=True):
            prepare_databases(run, os.environ.__getitem__, ROOT, root)
            self.assertEqual(os.environ["APP_CONFIG_DIR"], str(root / "config"))
        arguments = run.call_args.args[0]
        self.assertNotIn(environment["APP_DATABASE_PASSWORD"], " ".join(arguments))
        self.assertEqual(run.call_args.kwargs["env"]["MYSQL_PWD"], environment["APP_DATABASE_PASSWORD"])
        sql = arguments[-1].removeprefix("--execute=")
        for suffix in ("", "_shared", "_dedicated_a", "_dedicated_b"):
            self.assertIn(f"CREATE DATABASE `ryframe_e2e_42_3{suffix}`", sql)
        self.assertEqual(sql.count("CREATE DATABASE"), 4)
        self.assertNotIn("IF NOT EXISTS", sql)
        text = (root / "config/app.test.toml").read_text(encoding="utf-8")
        self.assertNotIn(environment["APP_DATABASE_PASSWORD"], text)
        targets = tomllib.loads(text)["tenant_data"]["targets"]
        self.assertEqual([target["mode"] for target in targets], ["shared", "dedicated", "dedicated"])
        self.assertTrue(all(target["password_env"] == "APP_DATABASE_PASSWORD" for target in targets))
        self.assertEqual((root / "environment").read_text(), f"APP_CONFIG_DIR={root / 'config'}\n")
        receipt = json.loads((root / "database-targets.json").read_text(encoding="utf-8"))
        self.assertEqual(receipt["state"], "created")
        self.assertEqual(receipt["scope_id"], "ci-42-3")
        self.assertEqual(receipt["container_id"], "a" * 64)

    def test_unsafe_environment_is_rejected_before_any_resource_creation(self):
        invalid = {"GITHUB_ACTIONS": "false", "APP_ENV": "prod", "GITHUB_RUN_ID": "42;DROP",
                   "GITHUB_RUN_ATTEMPT": "0", "APP_SCOPE_ID": "existing", "APP_DATABASE_NAME": "business",
                   "APP_DATABASE_HOST": "remote.example", "APP_DATABASE_PORT": "70000",
                   "RYFRAME_CI_MYSQL_CONTAINER_ID": "--all"}
        for key, value in invalid.items():
            with self.subTest(key=key):
                environment = {**self.environment(ROOT), key: value}
                run = Mock()
                with patch.dict(os.environ, environment, clear=True), self.assertRaises(ValueError):
                    prepare_databases(run, os.environ.__getitem__, ROOT, ROOT / ".local-tests/unused")
                run.assert_not_called()

    def test_late_or_malformed_inputs_are_rejected_before_files_and_database(self):
        for key, value in (
            ("GITHUB_ENV", "relative-env"),
            ("APP_DATABASE_PORT", "3306x"),
            ("APP_DATABASE_USERNAME", ""),
            ("APP_DATABASE_PASSWORD", ""),
            ("APP_DATABASE_TLS_MODE", "preferred"),
        ):
            with self.subTest(key=key):
                root = self.temporary_root()
                environment = {**self.environment(root), key: value}
                run = Mock()
                with patch.dict(os.environ, environment, clear=True), self.assertRaises(ValueError):
                    prepare_databases(run, os.environ.__getitem__, ROOT, root)
                run.assert_not_called()
                self.assertEqual(list(root.iterdir()), [])

    def test_partial_creation_is_recorded_as_unknown_without_exposing_password(self):
        root = self.temporary_root()
        environment = self.environment(root)
        run = Mock(side_effect=RuntimeError("mysql disconnected"))
        with patch.dict(os.environ, environment, clear=True), self.assertRaises(RuntimeError):
            prepare_databases(run, os.environ.__getitem__, ROOT, root)
        receipt = (root / "database-targets.json").read_text(encoding="utf-8")
        self.assertEqual(json.loads(receipt)["state"], "creation-unknown")
        self.assertNotIn(environment["APP_DATABASE_PASSWORD"], receipt)
        self.assertFalse((root / "environment").exists())

    def test_existing_plan_or_config_prevents_replay_before_database_access(self):
        for existing in ("database-targets.json", "config"):
            with self.subTest(existing=existing):
                root = self.temporary_root()
                path = root / existing
                path.mkdir() if existing == "config" else path.write_text("{}", encoding="utf-8")
                run = Mock()
                with patch.dict(os.environ, self.environment(root), clear=True), self.assertRaisesRegex(
                    ValueError, "拒绝自动重放"
                ):
                    prepare_databases(run, os.environ.__getitem__, ROOT, root)
                run.assert_not_called()
