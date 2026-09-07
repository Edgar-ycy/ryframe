"""隔离参考夹具计划不得回退读取历史复制来源。"""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

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
        self.wsl = self.root / "wsl.exe"; self.wsl.write_bytes(b"wsl")
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
                "backend_dir": str(self.root / "device-backend"),
                "identity_ledger": str(self.root / (side + "-identities")), "api_url": "http://127.0.0.1:18210",
                "worker_ready_url": "http://127.0.0.1:19210/readyz", "frontend_url": "http://127.0.0.1:4190",
                "objects": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1"},
                "redis": {"url": "redis://127.0.0.1:16390/0", "namespace": f"ryframe:{{{scope}}}:",
                          "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner", "ownership_value": "owner"},
                "databases": [{"key": key, "database": f"fixture_{side}_{key.replace('-', '_')}",
                               "mode": "shared" if key in ("shared-control", "shared") else "dedicated",
                               "expected_server_uuid": "uuid", "connection_file": str(defaults), "host": "127.0.0.1", "port": 3306}
                              for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")]}
        self.fixture = {"format_version": 1, "fixture": "device", "status": "ready", "sources": {"backend": {"head": "a" * 40}, "frontend": {"head": "b" * 40}},
                        "paths": {"backend": str(self.root / "device-backend"), "frontend": str(self.root / "device-frontend")}}
        self.maintenance = {"format_version": 1, "kind": "devex-clone-tool-build", "resources_modified": False, "artifacts":
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

    def test_plan_accepts_project_relative_evidence_paths(self):
        review = self.write("review.json", self.review)
        fixture = self.write("fixture.json", self.fixture)
        maintenance = self.write("build.json", self.maintenance)
        result = environment.plan(self.backend, review.relative_to(self.backend), fixture.relative_to(self.backend), maintenance.relative_to(self.backend))
        self.assertEqual(result["scope_id"], "fixture-seed")

    def test_revalidate_binds_redis_server_and_its_supervisor_tools(self):
        review = self.write("review.json", self.review)
        output = self.root / "review-r1.json"

        def run(arguments, **_):
            self.assertEqual(arguments[:4], [str(self.wsl.resolve()), "--distribution", "Ubuntu", "--exec"])
            command = arguments[4:]
            if command == ["/usr/bin/readlink", "-f", "/usr/bin/redis-server"]:
                value = "/usr/bin/redis-server"
            elif command == ["/usr/bin/sha256sum", "/usr/bin/redis-server"]:
                value = "c" * 64 + "  /usr/bin/redis-server"
            elif command == ["/usr/bin/redis-server", "--version"]:
                value = "Redis server v=7.0.15"
            elif command == ["/usr/bin/readlink", "-f", "/usr/bin/python3"]:
                value = "/usr/bin/python3.12"
            else:
                self.assertEqual(command, ["/usr/bin/sha256sum", "/usr/bin/python3.12"])
                value = "d" * 64 + "  /usr/bin/python3.12"
            return type("Completed", (), {"stdout": value.encode()})()

        with patch.object(environment.shutil, "which", return_value=str(self.wsl)):
            result = environment.revalidate(self.backend, review, output, run)

        self.assertEqual(result["tools"]["redis_server"]["resolved_path"], "/usr/bin/redis-server")
        self.assertEqual(result["tools"]["wsl"], {"path": str(self.wsl.resolve()), "sha256": file_digest(self.wsl)["sha256"]})
        self.assertEqual(result["tools"]["redis_python"]["resolved_path"], "/usr/bin/python3.12")
        self.assertEqual(result["services"]["rustfs"]["scope_id"], "services-fixture-seed")
        self.assertEqual(result["preflight"]["status"], "verified")
        self.assertNotIn("preflight", self.review)

    def test_preflight_binding_rejects_the_unreviewed_plan(self):
        with self.assertRaisesRegex(ValueError, "预检"):
            environment._preflight_binding(self.review)

    def test_environment_uses_the_frozen_device_tree_and_private_secret_files(self):
        execution = self.root / "device-backend"
        secrets = execution / ".local-tests/reference-fixture/secrets"
        config = execution / "config"
        secrets.mkdir(parents=True)
        config.mkdir(parents=True)
        (execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        (execution / ".git").write_text("gitdir: fixture", encoding="utf-8")
        (config / "app.toml").write_text("[app]", encoding="utf-8")
        (secrets / "mysql-client.cnf").write_text(
            "[client]\nhost=127.0.0.1\nport=3306\nuser=root\npassword=db-secret\nssl-mode=DISABLED\n",
            encoding="utf-8",
        )
        names = {
            "rustfs-access-key.txt": "access",
            "rustfs-secret-key.txt": "secret",
            "redis-password.txt": "redis",
            "reset-admin-password.txt": "admin",
            "reset-user-password.txt": "user",
            "jwt-secret.txt": "jwt",
            "metrics-token.txt": "metrics",
        }
        for name, value in names.items():
            (secrets / name).write_text(value, encoding="utf-8")
        seed = self.review["scopes"]["seed"]
        seed["backend_dir"] = str(execution)
        for item in seed["databases"]:
            item["connection_file"] = str(secrets / "mysql-client.cnf")
        generated = {"head": "a" * 40, "patch_sha256": "b" * 64, "files": []}
        fixture = {"format_version": 1, "fixture": "device", "status": "ready",
                   "paths": {"backend": str(execution), "frontend": str(self.root / "device-frontend")},
                   "generated": {"backend": generated}}

        with patch.object(environment, "snapshot", return_value=(generated, b"")):
            values, files = environment._environment(self.backend, self.review, fixture, self.root / "output")

        self.assertEqual(values["APP_SCOPE_ID"], "fixture-seed")
        self.assertEqual(values["APP_DATABASE_NAME"], "fixture_seed_shared_control")
        self.assertEqual(values["APP_OBJECT_STORAGE_ACCESS_KEY"], "access")
        self.assertEqual(values["APP_MONITOR_METRICS_BEARER_TOKEN"], "metrics")
        self.assertEqual(set(files), {"mysql-client.cnf", *names})


if __name__ == "__main__":
    unittest.main()
