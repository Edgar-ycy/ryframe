"""参考夹具的三侧 fresh request 必须共享 seed 服务代次并绑定各自环境。"""

from __future__ import annotations

import json
from pathlib import Path
import shutil
import sys
import unittest
import uuid
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_request as request
from restore_build import file_digest
from restore_reference_plan import plan_hash


class ReferenceFixtureRequestTests(unittest.TestCase):
    def write(self, path: Path, value: dict) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    @staticmethod
    def bound(path: Path) -> dict:
        return {"path": str(path), **file_digest(path)}

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.root = (
            self.backend / ".local-tests" / f"reference-request-test-{uuid.uuid4().hex}"
        )
        self.root.mkdir()
        self.addCleanup(shutil.rmtree, self.root)
        self.execution = self.root / "device-backend"
        (self.execution / ".local-tests/reference-fixture").mkdir(parents=True)
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
                "aws": tool,
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
            "future_root": str(
                self.execution / ".local-tests/reference-fixture/run-r1"
            ),
        }
        for index, side in enumerate(request.SIDES):
            scope = f"fixture-{side}"
            self.review["scopes"][side] = {
                "scope_id": scope,
                "runtime_dir": str(self.root / f"runtime-{side}"),
                "backend_dir": str(self.execution),
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
        self.review_path = self.write(self.root / "review.json", self.review)
        self.fixture = self.write(self.root / "fixture.json", {"fixture": "device"})
        self.maintenance = self.write(
            self.root / "build.json", {"kind": "devex-clone-tool-build"}
        )
        self.environments = {
            side: self.make_environment(side) for side in request.SIDES
        }
        self.service_run = Path(self.review["future_root"]) / "service-run"
        self.make_service_run()

    def make_environment(self, side: str) -> Path:
        directory = self.root / f"environment-{side}"
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
        self.write(
            directory / "environment.json",
            {
                "environment": {
                    "APP_SCOPE_ID": selected["scope_id"],
                    "APP_RESET_CREDENTIAL_VERSION": "fixture-v1",
                }
            },
        )
        return self.write(
            directory / "bootstrap.json",
            {
                "format_version": 1,
                "kind": "reference-fixture-environment",
                "status": "prepared",
                "plan": plan,
                "execution_backend": str(self.execution),
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
                "bootstrap": self.bound(self.environments["seed"]),
                "execution_backend": str(self.execution),
                "scope_id": "services-fixture-seed",
                "data_directory_was_empty": True,
            },
        )
        self.write(
            run / "state.json",
            {
                "attempts": [
                    {"stage": "storage-target", "mode": "initial", "status": "passed"},
                    {"stage": "cache-target", "mode": "initial", "status": "passed"},
                    {"stage": "fixture-buckets", "mode": "prepare", "status": "passed"},
                ]
            },
        )
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

    def test_each_side_uses_its_environment_and_the_same_seed_service_generation(self):
        requests = {
            side: request.build(
                self.backend, environment, self.service_run, f"fresh-{side}", side
            )
            for side, environment in self.environments.items()
        }
        self.assertEqual(set(requests), set(request.SIDES))
        for side, value in requests.items():
            with self.subTest(side=side):
                self.assertEqual(value["side"], side)
                self.assertEqual(value["target"]["scope_id"], f"fixture-{side}")
                self.assertEqual(
                    value["execution_backend"]["path"], str(self.execution)
                )
                self.assertEqual(value["storage"]["redis"]["generation"], "redis-r1")
                self.assertEqual(
                    {item["database"] for item in value["target"]["databases"]},
                    {
                        f"fixture_{side}_{key.replace('-', '_')}"
                        for key in (
                            "shared-control",
                            "shared",
                            "dedicated-a",
                            "dedicated-b",
                        )
                    },
                )

    def test_service_run_must_be_bound_to_the_seed_environment_of_the_same_review(self):
        manifest_path = self.service_run / "manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["bootstrap"] = self.bound(self.environments["base"])
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "指定侧|seed"):
            request.build(
                self.backend,
                self.environments["base"],
                self.service_run,
                "fresh-base",
                "base",
            )

    def test_side_plan_review_and_scope_drift_fail_before_request_publication(self):
        bootstrap = json.loads(
            self.environments["candidate"].read_text(encoding="utf-8")
        )
        bootstrap["plan"]["side"] = "base"
        self.environments["candidate"].write_text(
            json.dumps(bootstrap), encoding="utf-8"
        )
        output = self.root / "candidate-request.json"
        with self.assertRaisesRegex(ValueError, "指定侧|摘要"):
            request.publish(
                self.backend,
                self.environments["candidate"],
                self.service_run,
                "fresh-candidate",
                "candidate",
                output,
            )
        self.assertFalse(output.exists())

    def test_publish_never_overwrites_and_preserves_a_drifted_local_request(self):
        output = self.root / "request.json"
        first = {"request": "first"}
        with (
            patch.object(request, "build", side_effect=[first, {"request": "changed"}]),
            self.assertRaisesRegex(ValueError, "发布期间"),
        ):
            request.publish(
                self.backend,
                Path("unused"),
                Path("unused"),
                "fresh-base",
                "base",
                output,
            )
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), first)
        with (
            patch.object(request, "build", return_value=first),
            self.assertRaises(ValueError),
        ):
            request.publish(
                self.backend,
                Path("unused"),
                Path("unused"),
                "fresh-base",
                "base",
                output,
            )

    def test_cli_requires_explicit_side_and_write(self):
        common = [
            "reference_fixture_request.py",
            "--backend-dir",
            str(self.backend),
            "--environment",
            str(self.environments["base"]),
            "--service-run",
            str(self.service_run),
            "--id",
            "fresh-base",
            "--output",
            str(self.root / "request.json"),
        ]
        for missing in ([], ["--side", "base"]):
            with (
                self.subTest(arguments=missing),
                patch.object(sys, "argv", [*common, *missing]),
                self.assertRaises(SystemExit) as raised,
            ):
                request.main()
            self.assertEqual(raised.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
