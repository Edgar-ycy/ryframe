"""参考夹具服务必须在副作用和 ready 边界复核全部冻结输入。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

from devex_clone_run_state import begin, finish, initialize_state
import reference_fixture_services as services
from workspace_directory import WorkspaceDirectory


class ReferenceFixtureServicesTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/python-unit")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.execution = self.root / "business-backend"
        (self.execution / ".local-tests/reference-fixture").mkdir(parents=True)
        self.future = self.execution / ".local-tests/reference-fixture/run-r24"
        self.tool = self.root / "tool.exe"
        self.tool.write_bytes(b"tool")
        self.credentials = {}
        for name in ("rustfs-access-key.txt", "rustfs-secret-key.txt"):
            path = self.root / name
            path.write_text(name, encoding="utf-8")
            self.credentials[name] = services.bound(path)
        self.private = {"APP_ENV": "test"}
        self.settings = {"object_storage": {"access_key": "fixture-access", "secret_key": "fixture-secret"}}
        config = self.root / "config"
        config.mkdir()
        (config / "app.test.toml").write_text('[object_storage]\naccess_key = "fixture-access"\nsecret_key = "fixture-secret"\n', encoding="utf-8")
        self.review = {
            "future_root": str(self.future),
            "scopes": {
                "seed": {
                    "backend_dir": str(self.execution),
                    "scope_id": "fixture-seed-r24",
                    "objects": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1"},
                }
            },
            "services": {
                "rustfs": {
                    "api": "http://127.0.0.1:29200",
                    "console": "http://127.0.0.1:29201",
                    "data_dir": str(self.future / "rustfs"),
                    "scope_id": "services-fixture-seed-r24",
                }
            },
            "tools": {"rustfs": services.bound(self.tool)},
        }
        self.review_file = self.write(self.root / "review.json", self.review)
        self.bootstrap = {
            "execution_backend": str(self.execution),
            "secret_files": {
                "rustfs-access-key.txt": self.credentials["rustfs-access-key.txt"],
                "rustfs-secret-key.txt": self.credentials["rustfs-secret-key.txt"],
            },
            "plan": {"scope_id": "fixture-seed-r24"},
        }
        self.bootstrap_file = self.write(self.root / "bootstrap.json", self.bootstrap)
        self.environment_file = self.write(
            self.root / "environment.json", {"environment": self.private})

    @staticmethod
    def write(path: Path, value: dict) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def guard_sources(self) -> dict:
        manifest = self.write(self.root / "manifest.json", {"manifest": True})
        request = self.write(self.root / "request.json", {"request": True})
        return {
            "review": services.bound(self.review_file),
            "bootstrap": services.bound(self.bootstrap_file),
            "environment": services.bound(self.environment_file),
            "manifest": services.bound(manifest),
            "request": services.bound(request),
        }

    def test_bucket_configuration_uses_only_direct_file_values(self):
        ambient = {"AWS_ACCESS_KEY_ID": "ambient-access", "AWS_SECRET_ACCESS_KEY": "ambient-secret",
                   "AWS_SESSION_TOKEN": "ambient-token", "AWS_PROFILE": "ambient-profile",
                   "TEST_ACCESS_KEY": "ambient-test-access", "TEST_SECRET_KEY": "ambient-test-secret"}
        with patch.dict(os.environ, ambient):
            configuration = services._bucket_configuration(self.review, self.settings)
            self.assertEqual(configuration, {
                "endpoint": "http://127.0.0.1:29200", "region": "us-east-1",
                "access_key": "fixture-access", "secret_key": "fixture-secret",
            })
            for key in ("access_key", "secret_key"):
                for invalid in (None, "", 123):
                    with self.subTest(key=key, invalid=invalid), self.assertRaisesRegex(ValueError, "直接配置"):
                        services._bucket_configuration(self.review, {"object_storage": {**self.settings["object_storage"], key: invalid}})
                missing = {name: value for name, value in self.settings["object_storage"].items() if name != key}
                with self.assertRaisesRegex(ValueError, "直接配置"):
                    services._bucket_configuration(self.review, {"object_storage": missing})
            with self.assertRaisesRegex(ValueError, "直接配置"):
                services._bucket_configuration(self.review, {
                    "access_key_env": "TEST_ACCESS_KEY", "secret_key_env": "TEST_SECRET_KEY",
                })

    def test_ready_guard_rejects_every_frozen_input_and_tool_drift(self):
        sources = self.guard_sources()
        with patch.object(services, "validate_current_review_tools") as validate:
            services._ready_guard(
                self.backend, self.review_file, self.review, self.bootstrap_file,
                self.bootstrap, self.private, self.credentials, sources)
        validate.assert_called_once_with(self.review)

        paths = {
            "review": self.review_file,
            "bootstrap": self.bootstrap_file,
            "environment": self.environment_file,
            "credential": Path(self.credentials["rustfs-access-key.txt"]["path"]),
            "manifest": Path(sources["manifest"]["path"]),
            "request": Path(sources["request"]["path"]),
        }
        for name, path in paths.items():
            with self.subTest(input=name):
                before = path.read_bytes()
                path.write_bytes(before + b"changed")
                with (
                    patch.object(services, "validate_current_review_tools"),
                    self.assertRaises(ValueError),
                ):
                    services._ready_guard(
                        self.backend, self.review_file, self.review, self.bootstrap_file,
                        self.bootstrap, self.private, self.credentials, sources)
                path.write_bytes(before)

        with (
            patch.object(services, "validate_current_review_tools", side_effect=ValueError("tool drift")),
            self.assertRaisesRegex(ValueError, "tool drift"),
        ):
            services._ready_guard(
                self.backend, self.review_file, self.review, self.bootstrap_file,
                self.bootstrap, self.private, self.credentials, sources)

    def test_rustfs_passes_the_real_ready_guard_to_the_supervisor(self):
        seen = []

        def start(_backend, _request, _environment, _output, _registration, _controller,
                  _number, guard, *, supervised):
            self.assertTrue(supervised)
            seen.append(guard)
            guard()
            return {"identity": {"pid": 100}, "sha256": "a" * 64}

        with (
            patch.object(services, "environment", return_value=(self.bootstrap_file, self.bootstrap, self.private)),
            patch.object(services, "require_closed_port"),
            patch.object(services, "validate_current_review_tools"),
            patch.object(services, "start_rustfs", side_effect=start),
        ):
            result = services.rustfs(self.backend, self.review_file, self.bootstrap_file, write=True)

        self.assertEqual(result["status"], "rustfs_started")
        self.assertEqual(len(seen), 1)
        self.assertTrue(callable(seen[0]))

    def test_bucket_creation_rechecks_bound_file_configuration_before_readback(self):
        run = self.future / "service-run"
        run.mkdir(parents=True)
        manifest = self.write(run / "manifest.json", {"manifest": True})
        initialize_state(run)
        for stage in ("storage-target", "cache-target"):
            number = begin(run, stage, "initial", {"fixture": True})
            finish(run, number, result={"status": stage})
        self.assertTrue(manifest.is_file())
        calls = []

        def create(request):
            calls.append(request)
            self.assertEqual(request["configuration"]["secret_key"], self.settings["object_storage"]["secret_key"])
            self.environment_file.write_text('{"environment":{"changed":true}}', encoding="utf-8")
            return ({"status": "created_unverified", "http_status": 200,
                     "remote_write_may_have_occurred": True,
                     "automatic_retry_permitted": False},
                    {"returncode": 0, "error_type": None})

        with (
            patch.object(services, "environment", return_value=(self.bootstrap_file, self.bootstrap, self.private)),
            patch.object(services, "validate_current_review_tools"),
            patch.object(services, "native_create_bucket", side_effect=create),
            patch.object(services, "native_s3_inspect") as inspect,
            self.assertRaisesRegex(ValueError, "服务账本清单"),
        ):
            services.buckets(self.backend, self.review_file, self.bootstrap_file, write=True)

        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0]["kind"], "ryframe-native-s3-create-bucket")
        inspect.assert_not_called()


if __name__ == "__main__":
    unittest.main()
