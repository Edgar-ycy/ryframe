"""隔离参考夹具计划不得回退读取历史复制来源。"""
from __future__ import annotations

import copy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import reference_fixture_environment as environment
import reference_fixture_services as services
import reference_fixture_successor as successor
from reference_fixture_paths import service_run
from restore_build import file_digest


class ReferenceFixtureEnvironmentTests(unittest.TestCase):
    def write(self, name: str, value: dict) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def observed_tools(self) -> dict:
        observed = {name: copy.deepcopy(self.review["tools"][name]) for name in environment.REVIEW_FILE_TOOLS}
        observed.update({
            "redis_server": {**self.review["tools"]["redis_server"], "version": "Redis server v=7.0.15"},
            "wsl": {"path": str(self.wsl.resolve()), "sha256": file_digest(self.wsl)["sha256"]},
            "redis_python": {"distribution": "Ubuntu", "path": "/usr/bin/python3",
                             "resolved_path": "/usr/bin/python3.12", "sha256": "d" * 64},
        })
        return observed

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/python-unit")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = {}
        for name in environment.REVIEW_FILE_TOOLS:
            tool = self.root / f"{name}.bin"
            tool.write_bytes(name.encode())
            self.tools[name] = tool
        self.wsl = self.root / "wsl.bin"; self.wsl.write_bytes(b"wsl")
        defaults = self.root / "mysql.cnf"; defaults.write_text("[client]", encoding="utf-8")
        self.review = {"kind": "review-only-perf-resource-plan-with-readonly-preflight", "ready_for_execution": True,
                       "reference": {name: {"databases": []} for name in ("source", "protected_target")},
                       "tools": {name: {"path": str(self.tools[name]), "sha256": file_digest(self.tools[name])["sha256"]}
                                 for name in environment.REVIEW_FILE_TOOLS},
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
        self.review["future_root"] = str(self.root / "device-backend/.local-tests/reference-fixture/run-r1")
        self.fixture = {"format_version": 1, "fixture": "device", "status": "ready", "sources": {"backend": {"head": "a" * 40}, "frontend": {"head": "b" * 40}},
                        "paths": {"backend": str(self.root / "device-backend"), "frontend": str(self.root / "device-frontend")}}
        self.maintenance = {"format_version": 1, "kind": "devex-clone-tool-build", "resources_modified": False, "artifacts":
                            {key: {"executable": str(self.tools["mysql"])} for key in ("reset", "migrate", "tenant-data")}}

    def test_plan_is_read_only_and_excludes_historical_data(self):
        result = environment.plan(self.backend, self.write("review.json", self.review), self.write("fixture.json", self.fixture), self.write("build.json", self.maintenance))
        self.assertFalse(result["historical_data_used"])
        self.assertEqual(result["remote_writes"], 0)
        self.assertEqual(result["side"], "seed")
        base = environment.plan(self.backend, self.write("base-review.json", self.review), self.write("base-fixture.json", self.fixture),
                                self.write("base-build.json", self.maintenance), "base")
        self.assertEqual((base["side"], base["scope_id"]), ("base", "fixture-base"))

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
        self.assertEqual(
            result["services"]["rustfs"]["process_receipt"],
            str(Path(result["future_root"]) / "service-run/rustfs/process.json"),
        )
        self.assertEqual(result["preflight"]["status"], "verified")
        self.assertEqual(set(result["preflight"]["tools"]), set(environment.REVIEW_TOOLS))
        environment._preflight_binding(result)
        with patch.object(environment, "_preflight", return_value=result["preflight"]["tools"]):
            self.assertEqual(environment.validate_current_review_tools(result), result["preflight"]["tools"])
        drifted_tools = copy.deepcopy(result["preflight"]["tools"])
        drifted_tools["rustfs"]["sha256"] = "f" * 64
        with (
            patch.object(environment, "_preflight", return_value=drifted_tools),
            self.assertRaisesRegex(ValueError, "当前工具"),
        ):
            environment.validate_current_review_tools(result)
        missing_dump = copy.deepcopy(result)
        del missing_dump["preflight"]["tools"]["mysqldump"]
        with self.assertRaisesRegex(ValueError, "预检"):
            environment._preflight_binding(missing_dump)
        self.assertNotIn("preflight", self.review)

    def test_revalidate_rejects_mysqldump_drift_before_publishing(self):
        review = self.write("drift-review.json", self.review)
        output = self.root / "drift-ready.json"
        self.tools["mysqldump"].write_bytes(b"changed")
        with patch.object(environment.shutil, "which") as which, self.assertRaisesRegex(ValueError, "mysqldump"):
            environment.revalidate(self.backend, review, output)
        which.assert_not_called()
        self.assertFalse(output.exists())

    def test_revalidate_rejects_predecessor_or_terminal_tool_drift(self):
        self.review["ready_for_execution"] = False
        observed = self.observed_tools()
        predecessor = self.write("mutable-pending.json", self.review)
        predecessor_output = self.root / "mutable-ready.json"

        calls = 0

        def run(arguments, **_):
            nonlocal calls
            calls += 1
            if calls == 1:
                changed = copy.deepcopy(self.review)
                changed["future_root"] += "-changed"
                predecessor.write_text(json.dumps(changed), encoding="utf-8")
            return type("Completed", (), {"stdout": b"unused"})()

        with (
            patch.object(environment, "_preflight", side_effect=lambda review, _run: (run([], stdout=None), observed)[1]),
            self.assertRaisesRegex(ValueError, "predecessor"),
        ):
            environment.revalidate(self.backend, predecessor, predecessor_output, run)
        self.assertFalse(predecessor_output.exists())

        stable = self.write("stable-pending.json", self.review)
        terminal_output = self.root / "terminal-ready.json"
        drifted = copy.deepcopy(observed)
        drifted["aws"]["sha256"] = "f" * 64
        with (
            patch.object(environment, "_preflight", side_effect=[observed, drifted]),
            self.assertRaisesRegex(ValueError, "当前工具"),
        ):
            environment.revalidate(self.backend, stable, terminal_output)
        self.assertFalse(terminal_output.exists())

    def test_revalidate_preserves_historical_tools_for_ready_successor(self):
        self.review["ready_for_execution"] = False
        self.review["tools"].update({
            name: {"path": str(self.tools["mysql"]), "sha256": file_digest(self.tools["mysql"])["sha256"]}
            for name in ("node", "mysqld_exporter")
        })
        pending = self.write("historical-pending.json", self.review)
        output = self.root / "historical-ready.json"
        observed = {name: copy.deepcopy(self.review["tools"][name]) for name in environment.REVIEW_FILE_TOOLS}
        observed.update({
            "redis_server": copy.deepcopy(self.review["tools"]["redis_server"]),
            "wsl": {"path": str(self.wsl), "sha256": file_digest(self.wsl)["sha256"]},
            "redis_python": {"distribution": "Ubuntu", "path": "/usr/bin/python3",
                             "resolved_path": "/usr/bin/python3.12", "sha256": "b" * 64},
        })
        with patch.object(environment, "_preflight", return_value=observed):
            ready = environment.revalidate(self.backend, pending, output)
        self.assertEqual(set(ready["tools"]), {*environment.REVIEW_TOOLS, "node", "mysqld_exporter"})
        self.assertEqual(set(ready["preflight"]["tools"]), set(environment.REVIEW_TOOLS))
        environment._preflight_binding(ready)
        observed_ready, _ = successor._ready_review(self.backend, output)
        self.assertEqual(observed_ready, ready)
        self.assertTrue(
            services._continued_review(self.backend, environment.bound(pending), output, ready)
        )
        expanded_preflight = copy.deepcopy(ready)
        expanded_preflight["preflight"]["tools"]["node"] = ready["tools"]["node"]
        with self.assertRaisesRegex(ValueError, "预检"):
            environment._preflight_binding(expanded_preflight)
        expanded_receipt = copy.deepcopy(ready)
        expanded_receipt["preflight"]["unexpected"] = True
        with self.assertRaisesRegex(ValueError, "预检"):
            environment._preflight_binding(expanded_receipt)

        for field in ("scope_id", "process_receipt"):
            with self.subTest(rustfs_field=field):
                drifted = copy.deepcopy(ready)
                drifted["services"]["rustfs"][field] += "-changed"
                drifted_file = self.write(f"rustfs-{field}.json", drifted)
                with self.assertRaisesRegex(ValueError, "语义不同"):
                    successor._ready_review(self.backend, drifted_file)
                self.assertFalse(
                    services._continued_review(
                        self.backend, environment.bound(pending), drifted_file, drifted
                    )
                )

        for name in ("node", "mysqld_exporter"):
            for field in ("path", "sha256"):
                with self.subTest(name=name, field=field):
                    drifted = copy.deepcopy(ready)
                    drifted["tools"][name][field] = "f" * 64
                    drifted_file = self.write(f"{name}-{field}.json", drifted)
                    with self.assertRaisesRegex(ValueError, "语义不同"):
                        successor._ready_review(self.backend, drifted_file)
                    self.assertFalse(
                        services._continued_review(
                            self.backend, environment.bound(pending), drifted_file, drifted
                        )
                    )

        invalid_previous = copy.deepcopy(self.review)
        invalid_previous["ready_for_execution"] = True
        invalid_path = self.write("invalid-previous.json", invalid_previous)
        invalid_binding = environment.bound(invalid_path)
        invalid_ready = copy.deepcopy(ready)
        invalid_ready["preflight"]["supersedes"] = invalid_binding
        self.assertFalse(
            services._continued_review(
                self.backend, invalid_binding, output, invalid_ready
            )
        )

    def test_review_requires_mysqldump_but_accepts_historical_extra_tools(self):
        for name in (*environment.REVIEW_FILE_TOOLS, "redis_server"):
            with self.subTest(missing=name):
                value = copy.deepcopy(self.review)
                del value["tools"][name]
                with self.assertRaisesRegex(ValueError, "工具"):
                    environment.validate_review(value)
        extra = copy.deepcopy(self.review)
        extra["tools"]["legacy-wrapper"] = {"path": "unused", "sha256": "f" * 64}
        environment.validate_review(extra)

    def test_revalidate_can_promote_a_structurally_valid_pending_plan(self):
        self.review["ready_for_execution"] = False
        review = self.write("pending-review.json", self.review)
        output = self.root / "review-r2.json"

        def run(arguments, **_):
            command = arguments[4:]
            values = {
                ("/usr/bin/readlink", "-f", "/usr/bin/redis-server"): "/usr/bin/redis-server",
                ("/usr/bin/sha256sum", "/usr/bin/redis-server"): "c" * 64 + "  /usr/bin/redis-server",
                ("/usr/bin/redis-server", "--version"): "Redis server v=7.0.15",
                ("/usr/bin/readlink", "-f", "/usr/bin/python3"): "/usr/bin/python3.12",
                ("/usr/bin/sha256sum", "/usr/bin/python3.12"): "d" * 64 + "  /usr/bin/python3.12",
            }
            return type("Completed", (), {"stdout": values[tuple(command)].encode()})()

        with patch.object(environment.shutil, "which", return_value=str(self.wsl)):
            result = environment.revalidate(self.backend, review, output, run)
        self.assertTrue(result["ready_for_execution"])
        self.assertEqual(result["preflight"]["status"], "verified")

    def test_preflight_binding_rejects_the_unreviewed_plan(self):
        with self.assertRaisesRegex(ValueError, "预检"):
            environment._preflight_binding(self.review)

    def test_prepare_rechecks_all_documents_from_the_original_plan(self):
        self.review["ready_for_execution"] = False
        pending = self.write("prepare-pending.json", self.review)
        ready = self.root / "prepare-ready.json"
        observed = self.observed_tools()
        with patch.object(environment, "_preflight", return_value=observed):
            environment.revalidate(self.backend, pending, ready)
        fixture = self.write("prepare-fixture.json", self.fixture)
        maintenance = self.write("prepare-build.json", self.maintenance)
        output = self.root / "prepared"

        def configuration(_execution):
            changed = copy.deepcopy(self.fixture)
            changed["status"] = "changed"
            fixture.write_text(json.dumps(changed), encoding="utf-8")
            return "c" * 64

        with (
            patch.object(environment, "_environment", return_value=({"APP_SCOPE_ID": "fixture-seed"}, {})),
            patch.object(environment, "configuration_digest", side_effect=configuration),
            patch.object(environment, "verify_tools", return_value={"source": {}}),
            self.assertRaisesRegex(ValueError, "Device 收据"),
        ):
            environment.prepare(self.backend, ready, fixture, maintenance, output)
        self.assertTrue((output / "failed.json").is_file())
        self.assertFalse((output / "bootstrap.json").exists())

    def test_service_run_rejects_roots_outside_the_device_fixture(self):
        self.assertEqual(
            service_run(self.review),
            Path(self.review["future_root"]) / "service-run",
        )
        for value in (str(self.root / "outside"), str(Path(self.review["future_root"]) / "nested")):
            with self.subTest(value=value):
                invalid = dict(self.review, future_root=value)
                with self.assertRaisesRegex(ValueError, "future_root"):
                    service_run(invalid)

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
            "reset-admin-password.txt": "Admin1!fixture",
            "reset-user-password.txt": "User1!fixture",
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
                   "sources": {"backend": {"head": "a" * 40}, "frontend": {"head": "b" * 40}},
                   "paths": {"backend": str(execution), "frontend": str(self.root / "device-frontend")},
                   "generated": {"backend": generated}}

        with patch.object(environment, "snapshot", return_value=(generated, b"")):
            values, files = environment._environment(self.backend, self.review, fixture, self.root / "output")

        self.assertEqual(values["APP_SCOPE_ID"], "fixture-seed")
        self.assertEqual(values["APP_DATABASE_NAME"], "fixture_seed_shared_control")
        self.assertEqual((values["APP_APP_HOST"], values["APP_APP_PORT"]), ("127.0.0.1", "18210"))
        self.assertEqual(values["APP_CORS_ALLOW_ORIGINS"], "http://127.0.0.1:4190")
        self.assertEqual((values["APP_JOBS_HEALTH_HOST"], values["APP_JOBS_HEALTH_PORT"]), ("127.0.0.1", "19210"))
        self.assertEqual(values["APP_DATABASE_TLS_MODE"], "disabled")
        self.assertTrue(all(item["tls_mode"] == "disabled" for item in json.loads(values["APP_TENANT_DATA_TARGETS"])))
        self.assertEqual(values["APP_OBJECT_STORAGE_ACCESS_KEY"], "access")
        self.assertEqual(values["APP_MONITOR_METRICS_BEARER_TOKEN"], "metrics")
        self.assertEqual(set(files), {"mysql-client.cnf", *names})

        secret_set = secrets.parent / "secrets-r10"
        fixture_file = self.write("device-fixture.json", fixture)
        with patch.object(environment, "snapshot", return_value=(generated, b"")):
            rotated = environment.rotate_secrets(self.backend, fixture_file, secret_set)
            rotated_values, rotated_files = environment._environment(
                self.backend, self.review, fixture, self.root / "rotated-output", secret_directory=secret_set)
        self.assertEqual(rotated["status"], "generated")
        self.assertEqual(set(rotated["files"]), set(environment.SECRET_FILES))
        self.assertEqual(set(rotated_files), {"mysql-client.cnf", *names})
        self.assertNotEqual(rotated_values["RYFRAME_RESET_ADMIN_PASSWORD"], values["RYFRAME_RESET_ADMIN_PASSWORD"])
        self.assertNotEqual(rotated_values["RYFRAME_RESET_USER_PASSWORD"], values["RYFRAME_RESET_USER_PASSWORD"])
        environment._validate_reset_passwords(rotated_values)

        target_execution = self.root / "new-device-backend"
        (target_execution / ".local-tests/reference-fixture").mkdir(parents=True)
        (target_execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        (target_execution / ".git").write_text("gitdir: fixture", encoding="utf-8")
        target_fixture = json.loads(json.dumps(fixture))
        target_fixture["paths"]["backend"] = str(target_execution)
        target_fixture_file = self.write("new-device-fixture.json", target_fixture)
        with patch.object(environment, "snapshot", return_value=(generated, b"")):
            bootstrapped = environment.bootstrap_secrets(
                self.backend, fixture_file, secret_set.relative_to(self.backend), target_fixture_file)
        self.assertEqual(bootstrapped["status"], "imported")
        self.assertEqual(set(bootstrapped["target_files"]), set(environment.SECRET_FILES))
        self.assertFalse(bootstrapped["services_started"])
        self.assertFalse(bootstrapped["remote_writes"])
        for name in environment.SECRET_FILES:
            self.assertEqual(
                {key: bootstrapped["target_files"][name][key] for key in ("bytes", "sha256")},
                {key: bootstrapped["source_files"][name][key] for key in ("bytes", "sha256")},
            )

        drift_execution = self.root / "drift-device-backend"
        (drift_execution / ".local-tests/reference-fixture").mkdir(parents=True)
        (drift_execution / "Cargo.toml").write_text("[workspace]", encoding="utf-8")
        (drift_execution / ".git").write_text("gitdir: fixture", encoding="utf-8")
        drift_fixture = json.loads(json.dumps(fixture))
        drift_fixture["paths"]["backend"] = str(drift_execution)
        drift_fixture_file = self.write("drift-device-fixture.json", drift_fixture)
        real_fsync = environment.os.fsync
        changed = False

        def mutate_source(file_descriptor):
            nonlocal changed
            real_fsync(file_descriptor)
            if not changed:
                changed = True
                (secret_set / "rustfs-access-key.txt").write_text("changed", encoding="utf-8")

        with (
            patch.object(environment, "snapshot", return_value=(generated, b"")),
            patch.object(environment.os, "fsync", side_effect=mutate_source),
            self.assertRaisesRegex(ValueError, "秘密导入源"),
        ):
            environment.bootstrap_secrets(
                self.backend, fixture_file, secret_set.relative_to(self.backend), drift_fixture_file)
        self.assertFalse((drift_execution / ".local-tests/reference-fixture/secrets/bootstrap.json").exists())
        self.assertTrue((drift_execution / ".local-tests/reference-fixture/secrets/failed.json").is_file())

        values["RYFRAME_RESET_USER_PASSWORD"] = "weak"
        with self.assertRaisesRegex(ValueError, "RYFRAME_RESET_USER_PASSWORD"):
            environment._validate_reset_passwords(values)

        base_mysql = self.root / "base/backend/.local-tests/reference-fixture/secrets/mysql-client.cnf"
        for item in self.review["scopes"]["base"]["databases"]:
            item["connection_file"] = str(base_mysql)
        with patch.object(environment, "snapshot", return_value=(generated, b"")):
            base_values, base_files = environment._environment(self.backend, self.review, fixture, self.root / "base-output", "base")
        self.assertEqual(base_values["APP_SCOPE_ID"], "fixture-base")
        self.assertEqual(base_values["APP_DATABASE_NAME"], "fixture_base_shared_control")
        self.assertEqual(base_mysql.read_bytes(), (secrets / "mysql-client.cnf").read_bytes())
        self.assertEqual(base_files["mysql-client.cnf"], {"path": str(base_mysql), **file_digest(base_mysql)})

        self.review["scopes"]["base"]["backend_dir"] = str(self.root / "other-device-backend")
        with patch.object(environment, "snapshot", return_value=(generated, b"")):
            with self.assertRaisesRegex(ValueError, "所选侧 Device"):
                environment._environment(self.backend, self.review, fixture, self.root / "base-output", "base")


if __name__ == "__main__":
    unittest.main()
