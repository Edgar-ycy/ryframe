"""参考夹具服务必须在副作用和 ready 边界复核全部冻结输入。"""

from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from devex_clone_run_state import begin, finish, initialize_state
import reference_fixture_services as services
from workspace_directory import WorkspaceDirectory


class ReferenceFixtureServicesTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/python-unit")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.execution = self.root / "device-backend"
        (self.execution / ".local-tests/reference-fixture").mkdir(parents=True)
        self.future = self.execution / ".local-tests/reference-fixture/run-r24"
        self.tool = self.root / "tool.exe"
        self.tool.write_bytes(b"tool")
        self.credentials = {}
        for name in ("rustfs-access-key.txt", "rustfs-secret-key.txt"):
            path = self.root / name
            path.write_text(name, encoding="utf-8")
            self.credentials[name] = services.bound(path)
        self.private = {
            "APP_OBJECT_STORAGE_ACCESS_KEY": "access",
            "APP_OBJECT_STORAGE_SECRET_KEY": "secret",
        }
        self.review = {
            "future_root": str(self.future),
            "scopes": {
                "seed": {
                    "backend_dir": str(self.execution),
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
            "tools": {"rustfs": services.bound(self.tool), "aws": services.bound(self.tool)},
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

    def test_bucket_creation_rechecks_aws_before_the_readback(self):
        run = self.future / "service-run"
        run.mkdir(parents=True)
        manifest = self.write(run / "manifest.json", {"manifest": True})
        initialize_state(run)
        for stage in ("storage-target", "cache-target"):
            number = begin(run, stage, "initial", {"fixture": True})
            finish(run, number, result={"status": stage})
        self.assertTrue(manifest.is_file())
        calls = []

        def invoke(arguments, **_kwargs):
            calls.append(arguments)
            if "create-bucket" in arguments:
                self.tool.write_bytes(b"changed")
            return type("Completed", (), {"returncode": 0})()

        with (
            patch.object(services, "environment", return_value=(self.bootstrap_file, self.bootstrap, self.private)),
            patch.object(services.subprocess, "run", side_effect=invoke),
            self.assertRaisesRegex(ValueError, "AWS 工具"),
        ):
            services.buckets(self.backend, self.review_file, self.bootstrap_file, write=True)

        self.assertEqual(len(calls), 1)
        self.assertIn("create-bucket", calls[0])


if __name__ == "__main__":
    unittest.main()
