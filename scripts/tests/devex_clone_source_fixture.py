"""源导出的离线协议模型；复用完整控制/租户 DDL 与合法关系数据，不启动服务。"""
import copy
from mysql_verification_fixture import mysql_input, mysql_output
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import subprocess
from unittest.mock import patch

import test_devex_clone as clone_fixture
import devex_clone_export as export
import devex_clone_source_proof as proof
from restore_build import ROLES, file_digest
from restore_reference_io import DATA_HEADER
from restore_reference_plan import BUCKETS, plan_hash


class SourceFixture:
    def __init__(self, test):
        self.case = clone_fixture.CloneTests()
        self.case.setUp()
        test.addCleanup(self.case.doCleanups)
        self.backend, self.data = self.case.backend, self.case.data
        self.local = self.backend / ".local-tests"
        self.output = self.local / "source-export"
        self.runtime_dir = self.local / "runtime"
        self.runtime_dir.mkdir()
        self.scope = self.case.value["source"]["scope_id"]
        self.snapshot = {"head": "a" * 40, "clean": False, "files": []}
        self.fingerprint = "sha256:" + "b" * 64
        client = self.local / "client.cnf"
        client.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=fixture-secret\nssl-mode=DISABLED\n")
        source = {"scope_id": self.scope, "runtime_dir": str(self.runtime_dir), "api_url": "http://127.0.0.1:18210",
                  "s3": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1", "access_key_env": "TEST_ACCESS",
                         "secret_key_env": "TEST_SECRET"}, "databases": []}
        for row in self.case.value["source"]["databases"]:
            source["databases"].append({**{key: row[key] for key in ("key", "kind", "mode", "server_uuid", "database")},
                                        "defaults_file": str(client), "defaults_sha256": file_digest(client)["sha256"]})
        self.write_config(source)
        tool = self.local / "external.exe"
        tool.write_bytes(b"fixed-external-fixture")
        self.build = {"format_version": 1, "kind": "restore-backend-build", "source": self.snapshot, "artifacts": {}}
        identities = {}
        for number, (role, (feature, executable)) in enumerate(ROLES.items(), 100):
            path = self.local / (executable + ".exe")
            path.write_bytes(executable.encode())
            self.build["artifacts"][role] = {"executable": str(path), **file_digest(path),
                "command": ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features", "--features", feature,
                            "--bin", executable, "--message-format=json"]}
            identities[role] = {"pid": number, "started": "12345", "executable": str(path)}
        self.runtime = {"scope_id": self.scope, "worker_ready_url": "http://127.0.0.1:19210/readyz",
                        "artifacts": {role: {"path": item["executable"], "sha256": item["sha256"]} for role, item in self.build["artifacts"].items()}}
        self.maintenance = {"source": {"snapshot": self.snapshot, "worktree_fingerprint": self.fingerprint}, "artifacts": {}}
        for role in ("reset", "migrate", "tenant-data"):
            path = self.local / ("ryframe-" + role + ".exe")
            path.write_bytes(role.encode())
            self.maintenance["artifacts"][role] = {"executable": str(path), **file_digest(path)}
        self.request = {"format_version": 1, "kind": "devex-clone-source-export", "id": "fixture-source-export",
                        "source": source, "tools": {name: {"path": str(tool), "sha256": file_digest(tool)["sha256"]}
                                                     for name in ("mysql", "mysqldump", "aws", "node")},
                        "backend_build": self.bind(self.local / "backend-build.json", self.build),
                        "maintenance_build": self.bind(self.local / "maintenance-build.json", self.maintenance),
                        "worktree_fingerprint": self.fingerprint, "runtime": self.bind(self.runtime_dir / "runtime.json", self.runtime),
                        "processes": {}, "max_object_bytes": 1024}
        for role, identity in identities.items():
            self.request["processes"][role] = self.bind(self.runtime_dir / f"{role}.json",
                {"format_version": 1, "role": role, "scope_id": self.scope, "identity": identity})
        self.registry = {"format_version": 1, "scope_id": self.scope, "runtime_sha256": self.request["runtime"]["sha256"],
                         "processes": [{"name": role, "identity": identity} for role, identity in identities.items()]
                                      + [{"name": "dataset", "identity": {"pid": 102, "started": "12345", "executable": str(tool)}}]}
        self.request["producers_registry"] = self.bind(self.local / "producers.json", self.registry)
        self.request_path = self.local / "request.json"
        self.save_request()
        self.models = export.schema_models(self.backend)
        self.calls, self.inventory_count = [], 0
        self.uuid_failure = self.owner_failure = self.unknown_table = self.inventory_change = False
        self.unknown_column = False
        self.ledger_sets = {}
        self.missing_target = self.omit_empty_table = False
        self.environment = {"APP_ENV": "test", "APP_SCOPE_ID": self.scope, "APP_JOBS_MODE": "external",
                            "APP_SOURCE_PASSWORD": "fixture-secret", "APP_CONFIG_DIR": str(self.backend / "config"),
                            "TEST_ACCESS": "access-fixture", "TEST_SECRET": "secret-fixture"}
        self.patches = [patch.dict(os.environ, self.environment, clear=True),
                        patch.object(proof, "verify_source", side_effect=self.verify_source),
                        patch.object(proof, "verify_tools", side_effect=lambda *_: copy.deepcopy(self.maintenance)),
                        patch.object(proof, "verify_runtime", side_effect=lambda *_: copy.deepcopy(self.runtime)),
                        patch.object(proof, "process_identity", return_value=None), patch.object(proof, "require_closed_port")]
        self.mocks = [value.start() for value in self.patches]
        for value in self.patches:
            test.addCleanup(value.stop)

    def verify_source(self, _backend, build, fingerprint):
        if build["source"] != self.snapshot or fingerprint != self.fingerprint:
            raise ValueError("fixture source changed")
        return copy.deepcopy(self.snapshot)

    @staticmethod
    def bind(path, value):
        path.write_text(json.dumps(value), encoding="utf-8")
        return {"path": str(path), **file_digest(path)}

    def save_request(self):
        self.request_path.write_text(json.dumps(self.request), encoding="utf-8")

    def write_config(self, source):
        config = self.backend / "config"
        config.mkdir(exist_ok=True)
        control = source["databases"][0]
        content = ('[app]\nhost="127.0.0.1"\nport=18210\n[database.primary]\nhost="127.0.0.1"\nport=3306\n'
                   f'database="{control["database"]}"\nusername="fixture"\npassword="fixture-secret"\ntls_mode="disabled"\n'
                   '[object_storage]\nbackend="rustfs"\nendpoint="http://127.0.0.1:29200"\nregion="us-east-1"\n'
                   'access_key="access-fixture"\nsecret_key="secret-fixture"\nuse_ssl=false\n')
        for database in source["databases"][1:]:
            target = {"key": database["key"], "kind": "mysql", "mode": database["mode"], "host": "127.0.0.1", "port": 3306,
                      "database": database["database"], "username": "fixture", "password_env": "APP_SOURCE_PASSWORD", "tls_mode": "disabled"}
            content += "[[tenant_data.targets]]\n" + "".join(f"{key}={json.dumps(value)}\n" for key, value in target.items())
        (config / "app.toml").write_text(content, encoding="utf-8")

    def inventory(self, command):
        self.inventory_count += 1
        result = {"scope_id": self.scope, "source_sha": self.snapshot["head"],
                  "quiesced_at": command[command.index("--quiesced-at") + 1],
                  "captured_at": dt.datetime.now(dt.timezone.utc).isoformat(), "control_schema_fingerprint": "c" * 16,
                  "tenant_schema_fingerprint": "d" * 64, "databases": [], "objects": []}
        for database in self.request["source"]["databases"]:
            tables = set(self.models[1]) | (set(self.models[2]) - {"ryframe_resource_ownership", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}
                                           if database["kind"] == "combined" else set())
            counts = dict.fromkeys(tables, 0)
            for table, _ in self.data[database["key"]]:
                counts[table] += 1
            digests = [{"table": table, "rows": count, "sha256": plan_hash({"fixture_table": table, "rows": count})} for table, count in sorted(counts.items())]
            if self.omit_empty_table:
                digests = [table for table in digests if table["rows"]]
            if self.inventory_change and self.inventory_count == 2:
                digests[0]["sha256"] = "f" * 64
            placements = [{"tenant_id": row["tenant_id"], "generation": int(row["placement_generation"]), "switch_token": row["switch_token"]}
                          for table, row in self.data["shared-control"] if table == "sys_tenant_data_placement" and row["current_target_key"] == database["key"]]
            result["databases"].append({**{field: database[field] for field in ("key", "kind", "database", "server_uuid")},
                                        "shared": database["mode"] == "shared", "placements": placements, "tables": digests})
        if self.missing_target:
            result["databases"].pop()
        for bucket in sorted(BUCKETS):
            entries = [] if bucket != "uploads" else [{"key": self.scope + "/system/file.txt", "bytes": 4,
                                                        "sha256": hashlib.sha256(b"data").hexdigest()}]
            result["objects"].append({"bucket": bucket, "prefix": self.scope + "/", "entries": entries})
        return result

    def __call__(self, command, **kwargs):
        self.calls.append(command)
        if "s3api" in command:
            return self.aws(command)
        if "backup-inventory" in command:
            Path(command[command.index("--output") + 1]).write_text(json.dumps(self.inventory(command)), encoding="utf-8")
            return subprocess.CompletedProcess(command, 0, stdout=b"inventory written", stderr=b"")
        if "verify" in command:
            return subprocess.CompletedProcess(command, 0, stdout=b"schema=verified", stderr=b"")
        if "--no-create-info" in command:
            database = next(item for item in self.request["source"]["databases"] if item["database"] in command)
            raw = (self.case.root / (database["key"] + ".sql")).read_bytes().removeprefix(DATA_HEADER.encode())
            kwargs["stdout"].write(raw)
            return subprocess.CompletedProcess(command, 0, stderr=b"")
        database = next(item for item in self.request["source"]["databases"] if "--database=" + item["database"] in command)
        sql = mysql_input(command, kwargs)
        if "@@server_uuid" in sql:
            output = ("wrong-uuid" if self.uuid_failure else database["server_uuid"]) + "\t" + database["database"]
        elif "ryframe_resource_ownership" in sql:
            kinds = ("control", "tenant-data") if database["kind"] == "combined" else ("tenant-data",)
            output = "\n".join(f"{kind}\t{self.scope}\twrong" if self.owner_failure else
                               f"{kind}\t{self.scope}\tryframe-owner:v1:{self.scope}:{kind}" for kind in kinds)
        elif "information_schema.COLUMNS" in sql:
            ledgers = self.ledger_sets.get(database["key"],
                {"seaql_tenant_data_migrations"} | ({"seaql_migrations"} if database["kind"] == "combined" else set()))
            declared = {**(self.models[2] if database["kind"] == "combined" else {}), **self.models[3],
                        **{table: {"version": "VARCHAR", "applied_at": "BIGINT"} for table in sorted(ledgers)}}
            output = "\n".join(f"{table}\t{column}\t{kind}" for table, fields in declared.items() for column, kind in fields.items())
            if self.unknown_table:
                output += "\nunknown_business\tid\tBIGINT"
            if self.unknown_column:
                output += "\nbiz_tenant_fence\tunknown_column\tVARCHAR"
        else:
            raise AssertionError("未声明的 SQL 请求")
        return mysql_output(command, kwargs, output.encode())

    def aws(self, command):
        operation = command[command.index("s3api") + 1]
        assert operation in {"head-object", "get-object"}
        bucket, key = command[command.index("--bucket") + 1], command[command.index("--key") + 1]
        assert key.startswith(self.scope + "/")
        body = (f"ryframe-owner:v1:{self.scope}:object-storage:{bucket}".encode()
                if key.endswith("/.ryframe-owner") else b"data")
        if operation == "get-object":
            Path(command[-1]).write_bytes(body)
        result = {"ContentType": "text/plain", "Metadata": {}, "ETag": '"fixture-etag"', "ContentLength": len(body),
                  "LastModified": "2026-09-04T00:00:00+00:00"}
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps(result).encode(), stderr=b"")
