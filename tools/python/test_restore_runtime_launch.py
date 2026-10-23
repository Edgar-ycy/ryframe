"""恢复启动配置协议离线回归；不启动应用或共享服务。"""

import copy
import json
import os
from pathlib import Path
import unittest
from unittest.mock import patch

import restore_runtime_launch as launch
import test_restore_runtime_lifecycle as fixtures
from restore_runtime_evidence import read_json_document


class LaunchConfigurationTests(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.RestoreRuntimeLifecycleTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.base, self.request = fixture.base, fixture.request
        self.path = self.base / "private settings.json"
        self.private = {"scope_id": fixture.authority["scope_id"], "metrics_bearer_token": "private-token"}
        for index, role in enumerate(("api", "worker"), 1):
            directory = self.base / (role + " 中文 config")
            directory.mkdir()
            self.private[f"{role}_config_dir"] = str(directory)
            (directory / "app.toml").write_text(
                f'scope_id = "{self.private["scope_id"]}"\nsnowflake_worker_id = {index}\n'
                '[monitor]\nmetrics_bearer_token = "private-token"\n[jobs]\nmode = "external"\n',
                encoding="utf-8",
            )
            (directory / "app.test.toml").write_text(
                '[app]\nhost = "127.0.0.1"\nport = 18180\n[jobs]\n'
                'health_host = "127.0.0.1"\nhealth_port = 19191\n', encoding="utf-8",
            )
        self.bind()

    def bind(self):
        self.path.write_text(json.dumps(self.private), encoding="utf-8")
        document = read_json_document(self.path)
        self.request["environment"] = launch._environment(
            self.request["authority"], self.base, document, self.private,
        )

    def test_config_arguments_and_filtered_environment_never_publish_credentials(self):
        with patch.dict(os.environ, {"APP_SCOPE_ID": "ambient-private", "App_Private": "secret",
                                     "MYSQL_PWD": "password", "APP_ENV": "prod"}):
            commands = launch.commands(self.request)
            for role in launch.ROLES:
                environment = launch.launch_environment(self.request, role)
                self.assertEqual({key: value for key, value in environment.items()
                                  if key.upper().startswith("APP_")},
                                 {"APP_ENV": "test"} if role != "frontend" else {})
                self.assertNotIn("MYSQL_PWD", environment)
                self.assertNotIn("private-token", json.dumps([commands, environment, self.request]))
            for role in ("api", "worker"):
                self.assertEqual(commands[role][1:], ["--config-dir", self.private[f"{role}_config_dir"]])

    def test_private_document_rejects_old_wrappers_extra_keys_and_non_text(self):
        for value in ({"environment": self.private}, {**self.private, "APP_JOBS_MODE": "external"},
                      {**self.private, "app_scope_id": "retired"}, {**self.private, "extra": True},
                      {**self.private, "metrics_bearer_token": False}):
            self.path.write_text(json.dumps(value), encoding="utf-8")
            descriptor = launch._document_descriptor(read_json_document(self.path))
            with self.assertRaises(ValueError):
                launch._environment_document(fixtures.ROOT, {"fresh_target": {"environment": descriptor}})

    def test_file_drift_addition_removal_and_invalid_toml_fail_without_secret(self):
        directory = Path(self.private["api_config_dir"])
        selected = directory / "app.test.toml"
        original = selected.read_bytes()
        for content in (b'[monitor]\nmetrics_bearer_token = "private-wrong"\n',
                        b'scope_id = "other"\n', b'snowflake_worker_id = 2\n',
                        b'[app]\nhost = "127.0.0.1"\nport = 18181\n', b'private-token = [invalid'):
            selected.write_bytes(content)
            with self.assertRaises(ValueError) as raised:
                launch.launch_environment(self.request, "api")
            self.assertNotIn("private-wrong", str(raised.exception))
            self.assertNotIn("private-token", str(raised.exception))
        selected.write_bytes(original)
        extra = directory / "extra.toml"
        extra.write_text("# changed", encoding="utf-8")
        with self.assertRaises(ValueError):
            launch.launch_environment(self.request, "api")
        extra.unlink()
        selected.unlink()
        with self.assertRaises(ValueError):
            launch.launch_environment(self.request, "api")

    def test_request_rejects_retired_snapshot_keys_extra_fields_and_relative_directories(self):
        for change in ({"variables": []}, {"sha256": "a" * 64}, {"extra": True}):
            request = copy.deepcopy(self.request)
            request["environment"].update(change)
            with self.assertRaises(ValueError):
                launch.validate_request(request)
        self.private["api_config_dir"] = "relative"
        with self.assertRaises(ValueError):
            self.bind()


if __name__ == "__main__":
    unittest.main()
