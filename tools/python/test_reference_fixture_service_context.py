"""保留服务上下文、状态观察和私有 CLI 的有效场景，不访问共享服务。"""
from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
import test_reference_fixture_environment as environment_fixture
from devex_clone_run_state import begin, finish, initialize_state
import reference_fixture_service_context as context
from restore_build import file_digest
from restore_reference_plan import plan_hash


class ServiceContextTests(unittest.TestCase):
    def write(self, path: Path, value: dict) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    @staticmethod
    def bound(path: Path) -> dict:
        return {"path": str(path), **file_digest(path)}

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(self.backend / ".local-tests", prefix="rf-")
        self.root = Path(self.temporary.name)
        self.addCleanup(self.temporary.cleanup)
        self.tool = self.root / "tool.exe"
        self.tool.write_bytes(b"tool")
        self.defaults = self.root / "mysql.cnf"
        self.defaults.write_text("[client]", encoding="utf-8")
        tool = {"path": str(self.tool), "sha256": file_digest(self.tool)["sha256"]}
        self.review = {
            "kind": "review-only-perf-resource-plan-with-readonly-preflight",
            "ready_for_execution": True,
            "reference": {
                role: {"databases": []} for role in ("source", "protected_target")
            },
            "tools": {
                "mysql": tool,
                "mysqldump": tool,
                "rustfs": tool,
                "redis_server": {
                    "distribution": "Ubuntu",
                    "resolved_path": "/usr/bin/redis-server",
                    "sha256": "a" * 64,
                },
            },
            "services": {
                "rustfs": {
                    "api": "http://127.0.0.1:29200",
                    "console": "http://127.0.0.1:29201",
                    "data_dir": str(self.root / "rustfs"),
                    "scope_id": "services-fixture-seed",
                },
                "redis": {"directory": str(self.root / "redis")},
            },
            "scopes": {},
        }
        for index, side in enumerate(("seed", "base", "candidate")):
            scope = f"fixture-{side}"
            self.review["scopes"][side] = {
                "scope_id": scope,
                "runtime_dir": str(self.root / f"runtime-{side}"),
                "backend_dir": str(self.root),
                "identity_ledger": str(self.root / f"identities-{side}"),
                "api_url": f"http://127.0.0.1:{18210 + index}",
                "worker_ready_url": f"http://127.0.0.1:{19210 + index}/readyz",
                "frontend_url": f"http://127.0.0.1:{4190 + index}",
                "objects": {
                    "endpoint": "http://127.0.0.1:29200",
                    "region": "us-east-1",
                },
                "redis": {
                    "url": "redis://127.0.0.1:16390/0",
                    "namespace": f"ryframe:{{{scope}}}:",
                    "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner",
                    "ownership_value": f"ryframe-owner:v1:{scope}:redis",
                },
                "databases": [
                    {
                        "key": key,
                        "database": f"fixture_{side}_{key.replace('-', '_')}",
                        "mode": "shared"
                        if key in ("shared-control", "shared")
                        else "dedicated",
                        "expected_server_uuid": f"00000000-0000-0000-{index:04x}-{position:012x}",
                        "connection_file": str(self.defaults),
                        "host": "127.0.0.1",
                        "port": 3306,
                    }
                    for position, key in enumerate(
                        ("shared-control", "shared", "dedicated-a", "dedicated-b"),
                        start=1,
                    )
                ],
            }
        environment_fixture.publish_execution_fixture(self)
        self.execution = Path(self.publication["published"]["backend"]["path"])
        self.fixture = self.original_fixture
        self.review_path = self.write(self.root / "review.json", self.review)
        self.maintenance = self.write(
            self.root / "build.json", {"kind": "devex-clone-tool-build"}
        )
        self.bootstrap = self.make_seed_environment()
        self.service_run = Path(self.review["future_root"]) / "service-run"
        self.make_service_run()
        self.run = self.service_run
        self.review = self.review_path

    def make_seed_environment(self) -> Path:
        side = "seed"
        directory = self.execution / ".local-tests" / f"environment-{side}"
        directory.mkdir()
        selected = self.review["scopes"][side]
        plan = {
            "format_version": 1,
            "kind": "reference-fixture-environment-plan",
            "review": {
                **self.bound(self.review_path),
                "canonical_sha256": plan_hash(self.review),
            },
            "fixture": self.bound(self.fixture),
            "execution_source": self.bound(self.execution_source),
            "maintenance_build": self.bound(self.maintenance),
            "side": side,
            "scope_id": selected["scope_id"],
            "databases": [],
            "object_endpoint": selected["objects"]["endpoint"],
            "redis_url": selected["redis"]["url"],
            "historical_data_used": False,
            "remote_writes": 0,
        }
        plan["sha256"] = plan_hash(plan)
        self.write(directory / "environment.json", {"environment": {"APP_ENV": "test"}})
        config = directory / "config"
        config.mkdir()
        (config / "app.test.toml").write_text(
            f'scope_id = "{selected["scope_id"]}"\n[reset]\ncredential_version = "fixture-v1"\n'
            f'[object_storage]\naccess_key = "fixture-access-{side}"\nsecret_key = "fixture-secret-{side}"\n', encoding="utf-8")
        return self.write(
            directory / "bootstrap.json",
            {
                "format_version": 1,
                "kind": "reference-fixture-environment",
                "status": "prepared",
                "plan": plan,
                "execution_backend": str(self.execution),
                "config_dir": str(config),
                "configuration_sha256": "b" * 64,
                "services_started": False,
                "remote_writes": 0,
            },
        )

    def make_service_run(self) -> None:
        run = self.service_run
        (run / "rustfs").mkdir(parents=True)
        (run / "redis").mkdir()
        self.write(
            run / "manifest.json",
            {
                "format_version": 1,
                "kind": "reference-fixture-service-run",
                "review": self.bound(self.review_path),
                "bootstrap": self.bound(self.bootstrap),
                "execution_backend": str(self.execution),
                "scope_id": "services-fixture-seed",
                "data_directory_was_empty": True,
            },
        )
        initialize_state(run)
        for stage, mode in (("storage-target", "initial"), ("cache-target", "initial"), ("fixture-buckets", "prepare")):
            number = begin(run, stage, mode, {"fixture": True})
            finish(run, number, result={"status": stage})
        identity = {
            "executable": str(self.tool),
            "sha256": file_digest(self.tool)["sha256"],
        }
        self.write(
            run / "rustfs/process.json",
            {"role": "rustfs", "lifecycle": "running", "identity": identity},
        )
        self.write(run / "rustfs/launch.json", {"status": "started"})
        self.write(
            run / "redis/runtime.json",
            {"redis": {"port": 16390, "generation": "redis-r1"}},
        )

    def snapshot(self):
        return {str(path.relative_to(self.root)): path.read_bytes()
                for path in self.root.rglob("*") if path.is_file()}

    def test_context_and_transition_bridges_only_pass_native_protocol(self):
        value = {"backend": str(self.backend), "run": str(self.run), "private": {"APP_ENV": "test"}}
        with patch.object(context, "_invoke", return_value=value) as invoke:
            result = context.context(self.backend, self.review, self.bootstrap)
        self.assertEqual(result["run"], self.run)
        invoke.assert_called_once_with("service-context", {"review": self.review, "environment": self.bootstrap}, backend=self.backend)
        with patch.object(context, "_invoke", return_value=[{"generation": "bound"}, None]) as invoke:
            result = context.runtime_transition(self.backend, self.run, {"storage": "expected"})
        self.assertEqual(result, ({"generation": "bound"}, None))
        invoke.assert_called_once_with("service-transition", {"run": self.run, "expected": {"storage": "expected"}}, backend=self.backend)

    def test_legacy_receipts_do_not_gain_invented_tree_ownership(self):
        value = context.context(self.backend, self.review, self.bootstrap)
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, "历史首代"):
            context.registered_services(value)
        self.assertEqual(before, self.snapshot())


    def test_registration_and_observation_bridges_do_not_interpret_native_result(self):
        value = {"backend": self.backend, "run": self.run}
        expected = {"origin": "frozen"}
        with patch.object(context, "_invoke", return_value=expected) as invoke:
            self.assertIs(context.registered_services(value, settled=False), expected)
        invoke.assert_called_once_with("service-registered", {"value": value, "settled": False}, backend=self.backend)
        with patch.object(context, "_invoke", return_value=expected) as invoke:
            self.assertIs(context.observe_services({"frozen": True}), expected)
        invoke.assert_called_once_with("service-observe", {"services": {"frozen": True}})


if __name__ == "__main__":
    unittest.main()
