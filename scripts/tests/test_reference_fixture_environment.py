"""隔离参考夹具计划不得回退读取历史复制来源。"""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_environment as environment
from restore_build import file_digest


class ReferenceFixtureEnvironmentTests(unittest.TestCase):
    def write(self, name: str, value: dict) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = tempfile.TemporaryDirectory(dir=self.backend / ".local-tests")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        tool = self.root / "tool.exe"; tool.write_bytes(b"tool")
        defaults = self.root / "mysql.cnf"; defaults.write_text("[client]", encoding="utf-8")
        self.review = {"kind": "review-only-perf-resource-plan-with-readonly-preflight", "ready_for_execution": True,
                       "reference": {name: {"databases": []} for name in ("source", "protected_target")},
                       "tools": {name: {"path": str(tool), "sha256": file_digest(tool)["sha256"]}
                                 for name in ("mysql", "aws", "rustfs")},
                       "services": {"rustfs": {"api": "http://127.0.0.1:29200", "console": "http://127.0.0.1:29201", "data_dir": str(self.root / "data")},
                                    "redis": {"directory": str(self.root / "redis")}}, "scopes": {}}
        self.review["tools"]["redis_server"] = {"distribution": "Ubuntu", "resolved_path": "/usr/bin/redis-server", "sha256": "a" * 64}
        for side in ("seed", "base", "candidate"):
            scope = "fixture-" + side
            self.review["scopes"][side] = {"scope_id": scope, "runtime_dir": str(self.root / (side + "-runtime")),
                "identity_ledger": str(self.root / (side + "-identities")), "api_url": "http://127.0.0.1:18210",
                "worker_ready_url": "http://127.0.0.1:19210/readyz", "frontend_url": "http://127.0.0.1:4190",
                "objects": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1"},
                "redis": {"url": "redis://127.0.0.1:16390/0", "namespace": f"ryframe:{{{scope}}}:",
                          "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner", "ownership_value": "owner"},
                "databases": [{"key": key, "database": f"fixture_{side}_{key.replace('-', '_')}",
                               "expected_server_uuid": "uuid", "connection_file": str(defaults), "host": "127.0.0.1", "port": 3306}
                              for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")]}
        self.fixture = {"format_version": 1, "fixture": "device", "status": "ready", "sources": {"backend": {"head": "a" * 40}, "frontend": {"head": "b" * 40}},
                        "paths": {"backend": str(self.root / "device-backend"), "frontend": str(self.root / "device-frontend")}}
        self.maintenance = {"format_version": 1, "status": "maintenance_build_created", "artifacts":
                            {key: {"executable": str(tool)} for key in ("reset", "migrate", "tenant-data")}}

    def test_plan_is_read_only_and_excludes_historical_data(self):
        result = environment.plan(self.backend, self.write("review.json", self.review), self.write("fixture.json", self.fixture), self.write("build.json", self.maintenance))
        self.assertFalse(result["historical_data_used"])
        self.assertEqual(result["remote_writes"], 0)
        self.assertEqual(result["side"], "seed")

    def test_plan_rejects_historical_source_database(self):
        self.review["reference"]["source"]["databases"] = [{"database": "old"}]
        with self.assertRaisesRegex(ValueError, "历史来源"):
            environment.plan(self.backend, self.write("review.json", self.review), self.write("fixture.json", self.fixture), self.write("build.json", self.maintenance))


if __name__ == "__main__":
    unittest.main()
