"""fresh 生命周期离线替身；不连接或创建真实服务资源。"""
from __future__ import annotations
from mysql_verification_fixture import mysql_input

import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
from types import SimpleNamespace
from unittest.mock import patch

import devex_clone_target as target
import devex_clone_target_binding as binding
import devex_clone_target_resources as resources
import devex_clone_target_storage as storage
from devex_clone_transfer import DatabaseObservation
from full_stack_runtime import configuration_digest
from restore_build import file_digest
from restore_reference_plan import BUCKETS, plan_hash


class Fixture:
    def __init__(self, root: Path, test):
        self.root, self.local = root, root / ".local-tests"
        self.local.mkdir()
        (self.local / "manifest.json").write_text(
            json.dumps({"kind": "offline-run"}), encoding="utf-8"
        )
        self.scope, self.uuid = "perf-seed-fixture", "67cd8d4a-fe90-11ef-98fd-5847ca786499"
        self.paths = {}
        for role in ("mysql", "aws", "rustfs", "wsl", "reset", "migrate", "tenant-data"):
            p = self.local / (role + ".exe"); p.write_bytes(role.encode()); self.paths[role] = p
        self.configuration = root / "config"; self.configuration.mkdir()
        (self.configuration / "app.toml").write_text('[app]\nhost="127.0.0.1"\nport=18210\n[database]\nreplicas=[]\nsources=[]\n[database.primary]\nhost="127.0.0.1"\nport=3306\ndatabase="fresh_seed_control"\nusername="root"\npassword="db-secret"\ntls_mode="disabled"\n[object_storage]\nbackend="rustfs"\nendpoint="http://127.0.0.1:29200"\nregion="us-east-1"\naccess_key="access"\nsecret_key="secret"\nuse_ssl=false\n', encoding="utf-8")
        defaults = self.local / "mysql.cnf"
        defaults.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=root\npassword=db-secret\nssl-mode=DISABLED\n", encoding="utf-8")
        self.review = {"kind": "review-only-perf-resource-plan-with-readonly-preflight", "ready_for_execution": True, "scopes": {},
                       "reference": {role: {"databases": [{"server_uuid": self.uuid, "database": "protected_" + role}]}
                                     for role in ("source", "protected_target")},
                       "tools": {role: {"path": str(self.paths[role]), "sha256": file_digest(self.paths[role])["sha256"]}
                                 for role in ("mysql", "aws", "rustfs")}}
        self.review["tools"]["redis_server"] = {"distribution": "Ubuntu-24.04", "resolved_path": "/usr/bin/redis-server", "sha256": "9" * 64}
        for role in ("seed", "base", "candidate"):
            scope = "perf-" + role + "-fixture"
            self.review["scopes"][role] = {"scope_id": scope, "runtime_dir": str(self.local / (role + "-runtime")),
                "identity_ledger": str(self.local / (role + "-identities")), "api_url": "http://127.0.0.1:18210",
                "worker_ready_url": "http://127.0.0.1:19210/readyz", "frontend_url": "http://127.0.0.1:4190",
                "objects": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1"},
                "redis": {"url": "redis://127.0.0.1:16390/0", "namespace": f"ryframe:{{{scope}}}:",
                          "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner", "ownership_value": f"ryframe-owner:v1:{scope}:redis"},
                "databases": [{"key": key, "database": f"fresh_{role}_" + ("control" if key == "shared-control" else key.replace("-", "_")),
                               "expected_server_uuid": self.uuid, "connection_file": str(defaults), "host": "127.0.0.1", "port": 3306} for key in binding.KEYS]}
        selected = self.review["scopes"]["seed"]
        databases = [{"key": db["key"], "kind": binding.KEYS[db["key"]][0], "mode": binding.KEYS[db["key"]][1],
                      "database": db["database"], "server_uuid": self.uuid, "defaults_file": str(defaults),
                      "defaults_sha256": file_digest(defaults)["sha256"]} for db in selected["databases"]]
        self.maintenance = {"backend_root": str(root), "source": {"snapshot": {"head": "1" * 40}, "worktree_fingerprint": "sha256:" + "2" * 64},
                            "artifacts": {role: {"executable": str(self.paths[role]), **file_digest(self.paths[role])}
                                          for role in ("reset", "migrate", "tenant-data")}}
        self.identity = {"pid": 100, "started": "12345", "executable": str(self.paths["rustfs"])}
        self.request = {"format_version": 1, "kind": "devex-clone-fresh-target", "id": "fresh-fixture", "side": "seed",
                        "review": self.bound(self.local / "review.json", self.review),
                        "target": {"scope_id": self.scope, "s3": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1",
                                    "access_key_env": "APP_OBJECT_STORAGE_ACCESS_KEY", "secret_key_env": "APP_OBJECT_STORAGE_SECRET_KEY"}, "databases": databases},
                        "maintenance_build": self.bound(self.local / "build.json", self.maintenance),
                        "tools": {key: self.review["tools"][key] for key in ("mysql", "aws")},
                        "storage": {"rustfs": {"identity": self.identity, "sha256": file_digest(self.paths["rustfs"])["sha256"]},
                                    "redis": {"port": 16390, "wsl": {"path": str(self.paths["wsl"]), "sha256": file_digest(self.paths["wsl"])["sha256"]},
                                              "distribution": "Ubuntu-24.04", "pid": 200, "started": "67890", "executable": "/usr/bin/redis-server", "sha256": "9" * 64, "run_id": "3" * 40}},
                        "reset": {"legacy_ownership": {key: True for key in binding.EXCLUSIVE}, "credential_version": "fixture-v1",
                                  "sentinel_key": f"ryframe:devex-fresh:{self.scope}:sentinel", "sentinel_value": "devex-fresh:fresh-fixture"}}
        self.request["review"]["canonical_sha256"] = plan_hash(self.review)
        self.storage_dir = self.local / "storage"; self.storage_dir.mkdir()
        data_dir = self.storage_dir / "data"; data_dir.mkdir()
        redis_dir = self.storage_dir / "redis"; redis_dir.mkdir()
        self.review["services"] = {"rustfs": {"scope_id": "perf-storage-fixture", "data_dir": str(data_dir),
            "process_receipt": str(self.storage_dir / "process.json"), "api": "http://127.0.0.1:29200", "console": "http://127.0.0.1:29201"},
            "redis": {"directory": str(redis_dir)}}
        process = {"format_version": 1, "role": "rustfs", "scope_id": "perf-storage-fixture", "lifecycle": "running", "identity": self.identity,
                   "api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201", "data_dir": str(data_dir)}
        self.actual_argv = [str(self.paths["rustfs"]), "server", "--address", "127.0.0.1:29200", "--console-address", "127.0.0.1:29201", str(data_dir)]
        credential_files = {}
        for field, value in (("access_key", "access"), ("secret_key", "secret")):
            path = self.storage_dir / (field + ".txt"); path.write_text(value, encoding="utf-8")
            credential_files[field] = {"path": str(path), **file_digest(path)}
        launch = {"format_version": 1, "scope_id": "perf-storage-fixture", "identity": self.identity, "arguments": self.actual_argv,
                  "environment": {"RUSTFS_CONSOLE_ENABLE": "true", **{"RUSTFS_" + key.upper() + "_FILE": value["path"] for key, value in credential_files.items()}},
                  "credential_files": credential_files}
        self.request["storage"]["rustfs"].update(process_receipt=self.bound(self.storage_dir / "process.json", process),
                                                 launch_receipt=self.bound(self.storage_dir / "launch.json", launch))
        config = redis_dir / "redis.conf"; config.write_text("bind 127.0.0.1\n", encoding="utf-8")
        self.request["storage"]["redis"]["configuration"] = {"path": str(config), **file_digest(config)}
        self.request["review"] = {**self.bound(self.local / "review.json", self.review), "canonical_sha256": plan_hash(self.review)}
        self.linux_path = lambda path: "/fixture/" + path.name
        self.redis_config = {"bind": "127.0.0.1", "port": "16390", "dir": self.linux_path(redis_dir), "databases": "1",
                             "protected-mode": "yes", "save": "", "appendonly": "no"}
        targets = [{"key": db["key"], "kind": "mysql", "mode": db["mode"], "host": "127.0.0.1", "port": 3306,
                    "database": db["database"], "username": "root", "password_env": "APP_DB_PASSWORD", "tls_mode": "disabled"}
                   for db in databases if db["key"] != "shared-control"]
        self.environment = {"APP_ENV": "test", "APP_SCOPE_ID": self.scope, "APP_CONFIG_DIR": str(self.configuration),
            "APP_JOBS_MODE": "external", "APP_JOBS_HEALTH_HOST": "127.0.0.1", "APP_JOBS_HEALTH_PORT": "19210",
            "APP_DB_PASSWORD": "db-secret", "APP_TENANT_DATA_TARGETS": json.dumps(targets), "APP_OBJECT_STORAGE_ACCESS_KEY": "access",
            "APP_OBJECT_STORAGE_SECRET_KEY": "secret", "APP_REDIS_PASSWORD": "redis-secret", "APP_REDIS_HOST": "127.0.0.1",
            "APP_REDIS_PORT": "16390", "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false",
            "APP_RESET_CREDENTIAL_VERSION": "fixture-v1", "APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY": self.request["reset"]["sentinel_key"],
            "RYFRAME_RESET_ADMIN_PASSWORD": "admin-secret", "RYFRAME_RESET_USER_PASSWORD": "user-secret",
            **{"APP_RESET_LEGACY_" + key.upper(): "true" for key in binding.EXCLUSIVE}}
        self.patches = [patch.dict(os.environ, self.environment, clear=True),
                        patch.object(binding, "verify_tools", side_effect=lambda *_: copy.deepcopy(self.maintenance)),
                        patch.object(binding, "require_closed_port"), patch.object(resources, "process_identity", side_effect=lambda _: self.identity),
                        patch.object(resources, "verify_listener"), patch.object(resources.Resources, "_redis", side_effect=self.redis),
                        patch.object(storage, "actual_windows_argv", side_effect=lambda *_: self.actual_argv),
                        patch.object(storage, "verify_listener"), patch.object(storage, "wsl_path", side_effect=self.linux_path),
                        patch.object(target, "capture_side_inventory", side_effect=self.inventory)]
        self.mocks = [p.start() for p in self.patches]
        for p in self.patches: test.addCleanup(p.stop)
        self.request["configuration_sha256"] = configuration_digest(root)
        self.path, self.output = self.local / "request.json", self.local / "new-generation"
        self.save_request()
        self.databases, self.redis_values, self.extra_objects, self.calls = set(), {}, {}, []
        self.initialized = False; self.unknown_create = False; self.reset_status = "completed"
        self.plan_changed = False; self.plan_count = 0; self.storage_restarted = False

    @staticmethod
    def bound(path, value):
        path.write_text(json.dumps(value), encoding="utf-8")
        return {"path": str(path), **file_digest(path)}

    def save_request(self):
        self.path.write_text(json.dumps(self.request), encoding="utf-8")

    def redis(self, args):
        if args[:2] == ["INFO", "server"]:
            return "process_id:200\r\nrun_id:" + ("4" * 40 if self.storage_restarted else "3" * 40) + "\r\nconfig_file:/fixture/redis.conf"
        if args[:2] == ["CONFIG", "GET"]: return [args[2], self.redis_config[args[2]]]
        if args[0] == "GET": return self.redis_values.get(args[1])
        if args[0] == "SET":
            if args[1] in self.redis_values: return None
            self.redis_values[args[1]] = args[2]; return "OK"
        if args[0] == "SCAN": return ["0", [key for key in self.redis_values if key.startswith(args[3][:-1])]]
        raise AssertionError(args)

    def manifest(self):
        result = {"manifest_version": 4, "environment": "test", "scope_id": self.scope, "code_sha": "1" * 40,
                  "config_sha": "5" * 64, "credential_version": "fixture-v1", "confirmation_phrase": "RESET-RYFRAME-test-" + self.scope,
                  "legacy_ownership": self.request["reset"]["legacy_ownership"], "databases": []}
        for db in self.request["target"]["databases"]:
            kinds = ("control", "tenant-data") if db["kind"] == "combined" else ("tenant-data",)
            result["databases"].append({"host": "127.0.0.1", "port": 3306, "database": db["database"],
                "target_keys": [db["key"]], "control_baseline": db["kind"] == "combined", "tenant_baseline": True,
                "ownership_markers": {kind: f"ryframe-owner:v1:{self.scope}:{kind}" for kind in kinds},
                "connection": {"tls_mode": "disabled", "username": "root"}})
        result["object_storage"] = {"backend": "rustfs", "endpoint": "http://127.0.0.1:29200", "region": "us-east-1", "use_ssl": False,
            "prefixes": [{"bucket": bucket, "prefix": self.scope + "/", "ownership_marker_key": self.scope + "/.ryframe-owner",
                          "ownership_marker": f"ryframe-owner:v1:{self.scope}:object-storage:{bucket}"} for bucket in sorted(BUCKETS)]}
        redis = self.review["scopes"]["seed"]["redis"]
        result["redis"] = {"host": "127.0.0.1", "port": 16390, "database": 0, "tls": False, "namespace": redis["namespace"],
            "ownership_marker_key": redis["ownership_key"], "ownership_marker": redis["ownership_value"],
            "outside_sentinel_key_sha256": hashlib.sha256(self.request["reset"]["sentinel_key"].encode()).hexdigest()}
        return result

    def run(self, command, **kwargs):
        self.calls.append(command)
        role = Path(command[0]).stem; raw = b""
        if role == "mysql":
            sql = mysql_input(command, kwargs)
            if sql.startswith("SELECT @@"):
                name = sql.split("SCHEMA_NAME = '")[1].split("'")[0]
                raw = (self.uuid + ("\n" + name if name in self.databases else "")).encode()
            elif sql.startswith("CREATE DATABASE"):
                name = sql.split('`')[1]
                if name in self.databases: raise AssertionError("duplicate CREATE")
                self.databases.add(name)
                if self.unknown_create: raise subprocess.TimeoutExpired(command, 30, stderr=b"db-secret timeout")
            elif not sql.startswith("SELECT TABLE_NAME"): raise AssertionError(sql)
        elif role == "aws":
            operation = command[command.index("s3api") + 1]; bucket = command[command.index("--bucket") + 1]
            value = {}
            if operation == "list-objects-v2":
                keys = self.extra_objects.get(bucket, []) + ([self.scope + "/.ryframe-owner"] if self.initialized else [])
                value = {"IsTruncated": False, "Contents": [{"Key": key} for key in keys]}
            elif operation == "get-object": Path(command[-1]).write_bytes(f"ryframe-owner:v1:{self.scope}:object-storage:{bucket}".encode())
            elif operation not in ("head-bucket", "get-bucket-versioning"): raise AssertionError(operation)
            raw = json.dumps(value).encode()
        elif role == "wsl":
            operation = command[command.index("--exec") + 1]
            raw = ({"/usr/bin/cat": "200 (redis-server) " + " ".join(["S"] + ["0"] * 18 + ["67890"]),
                    "/usr/bin/readlink": "/fixture/redis.conf" if command[-1] == "/fixture/redis.conf" else "/usr/bin/redis-server",
                    "/usr/bin/sha256sum": (self.request["storage"]["redis"]["configuration"]["sha256"] if command[-1] == "/fixture/redis.conf" else "9" * 64) + "  file"}[operation]).encode()
        elif role == "reset":
            manifest = self.manifest()
            if command[1] == "plan":
                self.plan_count += 1
                if self.plan_changed and self.plan_count == 2: manifest["config_sha"] = "6" * 64
                text = json.dumps(manifest, indent=2); raw = (text + "\nplan_hash=" + hashlib.sha256(text.encode()).hexdigest() + "\n").encode()
            else:
                self.initialized = True
                redis = self.review["scopes"]["seed"]["redis"]; self.redis_values[redis["ownership_key"]] = redis["ownership_value"]
                sha = command[command.index("--plan-hash") + 1]; directory = Path(kwargs["env"]["RYFRAME_RESET_STATE_DIR"])
                phases = {phase: {"status": "complete", "completed_at": "2026-09-04T00:00:00Z"} for phase in target.PHASES}
                common = {**{key: manifest[key] for key in ("environment", "scope_id", "code_sha", "config_sha", "credential_version")},
                          "plan_hash": sha, "phases": phases, "resources": {"resource": {"status": "complete"}}}
                self.bound(directory / f"test-{self.scope}-{sha}.ledger.json", {**common, "ledger_version": 4})
                self.bound(directory / f"test-{self.scope}-{sha}.report.json", {**common, "report_version": 2, "status": self.reset_status,
                    "failed_phase": None, "completed_at": phases["release"]["completed_at"],
                    "databases": [f"127.0.0.1:3306/{db['database']}" for db in self.request["target"]["databases"]],
                    "redis_namespace": redis["namespace"], "object_prefixes": [bucket + ":" + self.scope + "/" for bucket in BUCKETS]})
        elif role != "migrate": raise AssertionError(command)
        return subprocess.CompletedProcess(command, 0, raw, b"")

    def inventory(self, backend, side, tools, receipt, output, *, environment):
        output.mkdir()
        observations = []
        for db in self.request["target"]["databases"]:
            tables = {"biz_tenant_fence": {"rows": 0, "sha256": "a" * 64}, "biz_tenant_target_slot": {"rows": 1, "sha256": "b" * 64}}
            observations.append(DatabaseObservation({"database": db["database"]}, "7" * 64, tables, {}, tuple(tables), ()))
        value = {"status": "offline_fixture_only", "inventories": {}}
        return SimpleNamespace(observations=tuple(observations), receipt_file=self.bound(output / "inventory.json", value))
