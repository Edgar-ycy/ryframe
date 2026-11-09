"""fresh 生命周期离线替身；不连接或创建真实服务资源。"""
from __future__ import annotations
from mysql_verification_fixture import mysql_input

import copy
import json
import os
from pathlib import Path
import subprocess
from unittest.mock import patch

import devex_clone_target_binding as binding
from artifact_digests import filesystem_path
import devex_clone_target_resources as resources
from devex_clone_run_state import binding as receipt_binding
from full_stack_runtime import configuration_digest
from restore_build import file_digest
from restore_reference_plan import BUCKETS, plan_hash


class Fixture:
    def __init__(self, root: Path, test):
        self.root, self.local = root, root / ".local-tests"
        self.local.mkdir()
        self.workspace = self.local / "fresh-workspace"
        self.workspace.mkdir()
        self.output = self.workspace / "target"
        (self.local / "manifest.json").write_text(
            json.dumps({"kind": "offline-run"}), encoding="utf-8"
        )
        self.scope, self.uuid = "perf-seed-fixture", "67cd8d4a-fe90-11ef-98fd-5847ca786499"
        self.paths = {}
        for role in ("mysql", "mysqldump", "rustfs", "wsl", "reset", "migrate", "tenant-data"):
            p = self.local / (role + ".exe"); p.write_bytes(role.encode()); self.paths[role] = p
        self.configuration = root / "config"; self.configuration.mkdir()
        configuration = (f'snowflake_worker_id=3\nscope_id="{self.scope}"\n[app]\nhost="127.0.0.1"\nport=18210\n'
                         '[jobs]\nmode="external"\nhealth_host="127.0.0.1"\nhealth_port=19210\n'
                         '[bootstrap]\nadmin_password="Fixture!Admin123"\n'
                         '[database]\nreplicas=[]\nsources=[]\n'
                         '[database.primary]\nhost="127.0.0.1"\nport=3306\n'
                         'database="fresh_seed_control"\nusername="root"\npassword="db-secret"\n'
                         'tls_mode="disabled"\n[object_storage]\nbackend="rustfs"\n'
                         'endpoint="http://127.0.0.1:29200"\nregion="us-east-1"\n'
                         'access_key="access"\nsecret_key="secret"\nuse_ssl=false\n'
                         '[redis]\nmode="required"\nhost="127.0.0.1"\nport=16390\n'
                         'password="redis-secret"\ndatabase=0\nmax_pool_size=16\ntimeout_secs=3\ntls=false\n'
                         '[reset]\nlegacy_mysql_exclusive=true\nlegacy_redis_exclusive=true\n'
                         'legacy_object_storage_exclusive=true\ncredential_version="fixture-v1"\n'
                         f'source_sha={json.dumps("1" * 40)}\n'
                         f'state_directory={json.dumps(str(self.output / "reset-state"))}\n'
                         'services_stopped=true\n'
                         f'redis_outside_sentinel_key="ryframe:devex-fresh:{self.scope}:sentinel"\n')
        defaults = self.local / "mysql.cnf"
        defaults.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=root\npassword=db-secret\nssl-mode=DISABLED\n", encoding="utf-8")
        self.review = {"kind": "review-only-perf-resource-plan-with-readonly-preflight", "ready_for_execution": True, "scopes": {},
                       "reference": {role: {"databases": [{"server_uuid": self.uuid, "database": "protected_" + role}]}
                                     for role in ("source", "protected_target")},
                       "tools": {role: {"path": str(self.paths[role]), "sha256": file_digest(self.paths[role])["sha256"]}
                                 for role in binding.REVIEW_FILE_TOOLS}}
        self.review["tools"]["redis_server"] = {"distribution": "Ubuntu-24.04", "resolved_path": "/usr/bin/redis-server", "sha256": "9" * 64}
        self.review["tools"]["wsl"] = {
            "path": str(self.paths["wsl"]),
            "sha256": file_digest(self.paths["wsl"])["sha256"],
        }
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
        for database in databases[1:]:
            target_config = {"key": database["key"], "kind": "mysql", "mode": database["mode"],
                             "host": "127.0.0.1", "port": 3306, "database": database["database"],
                             "username": "root", "password": "db-secret", "tls_mode": "disabled"}
            configuration += "[[tenant_data.targets]]\n" + "".join(
                f"{key}={json.dumps(value)}\n" for key, value in target_config.items()
            )
        (self.configuration / "app.toml").write_text(configuration, encoding="utf-8")
        (self.configuration / "app.test.toml").write_text(
            "# 测试环境由公共夹具配置提供。\n", encoding="utf-8"
        )
        self.maintenance = {"kind": "devex-clone-tool-build", "backend_root": str(root),
                            "source": {"snapshot": {"head": "1" * 40}, "worktree_fingerprint": "sha256:" + "2" * 64},
                            "artifacts": {role: {"executable": str(self.paths[role]), **file_digest(self.paths[role])}
                                          for role in ("reset", "migrate", "tenant-data")}}
        self.identity = {"pid": 100, "started": "12345", "executable": str(self.paths["rustfs"])}
        self.request = {"format_version": 1, "kind": "devex-clone-fresh-target", "id": "fresh-fixture", "side": "seed",
                        "review": self.bound(self.local / "review.json", self.review),
                        "target": {"scope_id": self.scope, "s3": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1",
                                    "access_key": "access", "secret_key": "secret"}, "databases": databases},
                        "maintenance_build": self.bound(self.local / "build.json", self.maintenance),
                        "tools": {"mysql": self.review["tools"]["mysql"]},
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
        credential_files = {}
        for field, value in (("access_key", "access"), ("secret_key", "secret")):
            path = self.storage_dir / (field + ".txt"); path.write_text(value, encoding="utf-8")
            credential_files[field] = {"path": str(path), **file_digest(path)}
        from devex_clone_storage_request import arguments
        self.actual_argv = arguments({"executable": {"path": str(self.paths["rustfs"])},
            "api_url": process["api_url"], "console_url": process["console_url"],
            "data_directory": {"path": str(data_dir)}, "credential_files": credential_files})
        launch = {"format_version": 1, "scope_id": "perf-storage-fixture", "identity": self.identity, "arguments": self.actual_argv,
                  "credential_files": credential_files}
        self.request["storage"]["rustfs"].update(process_receipt=self.bound(self.storage_dir / "process.json", process),
                                                 launch_receipt=self.bound(self.storage_dir / "launch.json", launch))
        config = redis_dir / "redis.conf"; config.write_text("bind 127.0.0.1\n", encoding="utf-8")
        self.request["storage"]["redis"]["configuration"] = {"path": str(config), **file_digest(config)}
        self.request["review"] = {**self.bound(self.local / "review.json", self.review), "canonical_sha256": plan_hash(self.review)}
        self.linux_path = lambda path: ("/mnt/" + path.drive[0].lower() + str(path)[2:].replace("\\", "/")) if path.drive else path.as_posix()
        self.redis_config = {"bind": "127.0.0.1", "port": "16390", "dir": self.linux_path(redis_dir), "databases": "1",
                             "protected-mode": "yes", "save": "", "appendonly": "no"}
        self.environment = {"APP_ENV": "test"}
        native_evidence = resources.evidence
        # 此夹具只提供离线编排输入；真实布局、参数和身份竞态由 Rust storage 行为测试核验。
        def resource_evidence(operation, backend, target, **fields):
            if operation == "storage-layout":
                return {"status": "storage_layout_verified", "remote_writes": 0}
            return native_evidence(operation, backend, target, **fields)
        self.patches = [patch.dict(os.environ, self.environment, clear=True),
                        patch.object(binding, "verify_tools", side_effect=lambda *_: copy.deepcopy(self.maintenance)),
                        patch.object(binding, "require_closed_port"),
                        patch.object(resources, "evidence", side_effect=resource_evidence),
                        patch.object(resources.Resources, "_redis", side_effect=self.redis)]
        self.mocks = [p.start() for p in self.patches]
        for p in self.patches: test.addCleanup(p.stop)
        self.request["config_dir"] = str(self.configuration)
        self.request["configuration_sha256"] = configuration_digest(root, self.configuration)
        from restore_reference_plan import configuration_binding
        self.object_configuration = configuration_binding(root, self.configuration)
        self.path = self.local / "request.json"
        self.save_request()
        self.databases, self.redis_values, self.extra_objects, self.calls = set(), {}, {}, []
        self.initialized = False
        self.storage_restarted = False
        self.prepared_files = None

    @staticmethod
    def bound(path, value):
        with open(filesystem_path(path), "w", encoding="utf-8") as stream:
            json.dump(value, stream)
        return {"path": str(path), **file_digest(path)}

    def save_request(self):
        self.path.write_text(json.dumps(self.request), encoding="utf-8")

    def prepared(self):
        """为仍保留的初始化行为提供固定有效前像；不执行或模拟 prepare 编排。"""
        from devex_clone_capture import write_json
        from devex_clone_target_evidence import evidence
        from devex_clone_target_state import now
        import uuid
        self.output.mkdir(exist_ok=True)
        request_copy = self.output / "request.json"
        if not request_copy.exists():
            write_json(request_copy, copy.deepcopy(self.request))
        elif json.loads(request_copy.read_text(encoding="utf-8")) != self.request:
            raise AssertionError("固定测试前像请求不能被重绑")
        generation = binding.generation(self.root, self.request, self.run)
        generation["storage"] = copy.deepcopy(self.request["storage"])
        absent = {"databases": [{"server_uuid": db["server_uuid"], "database": db["database"],
                                 "exists": False, "empty_checked": False}
                                for db in self.request["target"]["databases"]],
                  "objects": {bucket: {"keys": []} for bucket in BUCKETS},
                  "redis": {"keys": [], "owner": None, "sentinel": None}}
        descriptor = receipt_binding(self.path)
        result = evidence("prepare-document", self.root, self.output, arguments={"at": now(),
                          "request_descriptor": descriptor, "generation": generation, "absence": absent})
        write_json(self.output / "prepare.json", result)
        for db in self.request["target"]["databases"]:
            self.prepared_command("mysql-" + db["key"], [str(self.paths["mysql"]),
                "--defaults-file=" + db["defaults_file"], "--connect-timeout=5", "--batch", "--raw",
                "--skip-column-names", "--binary-mode", "--default-character-set=utf8mb4"], self.uuid + "\r\n")
        redis = self.request["storage"]["redis"]
        linux = self.linux_path(Path(redis["configuration"]["path"]))
        fields = ["1"] * 20
        fields[19] = redis["started"]
        prefix = [redis["wsl"]["path"], "--distribution", redis["distribution"], "--exec"]
        layout = {"process_receipt": self.request["storage"]["rustfs"]["process_receipt"],
                  "launch_receipt": self.request["storage"]["rustfs"]["launch_receipt"],
                  "actual_arguments_sha256": plan_hash(self.actual_argv),
                  "data_dir": self.review["services"]["rustfs"]["data_dir"],
                  "environment_proof": "受控启动收据与凭据文件；不声称从内核枚举进程环境"}
        for _ in range(2):
            for args, stdout in [
                (["/usr/bin/cat", f"/proc/{redis['pid']}/stat"], f"{redis['pid']} (redis-server) " + " ".join(fields) + "\r\n"),
                (["/usr/bin/readlink", "-f", f"/proc/{redis['pid']}/exe"], redis["executable"] + "\r\n"),
                (["/usr/bin/sha256sum", redis["executable"]], redis["sha256"] + "  " + redis["executable"] + "\r\n"),
                (["/usr/bin/readlink", "-f", linux], linux + "\r\n"),
                (["/usr/bin/sha256sum", linux], redis["configuration"]["sha256"] + "  " + linux + "\r\n"),
            ]:
                self.prepared_command("redis-kernel", prefix + args, stdout)
            write_json(self.output / f"storage-layout-{uuid.uuid4().hex}.json", layout)
        for stage, operation in (("bucket-readiness", "head-bucket"),
                                 ("bucket-versioning", "get-bucket-versioning"), ("scoped-list", "list-objects-v2")):
            for bucket in sorted(BUCKETS):
                listed = operation == "list-objects-v2"
                write_json(self.output / f"{stage}-{uuid.uuid4().hex}.command.json", {
                    "transport": "rust-s3-sigv4", "operation": operation, "bucket": bucket,
                    "returncode": 0, "error_type": None, "response": {"IsTruncated": False, "Contents": []} if listed else {},
                    "key": None, "destination": None, "remote_writes": 0, "scope_id": self.scope,
                    "endpoint": self.request["target"]["s3"]["endpoint"],
                    "configuration_sha256": plan_hash(self.request["target"]["s3"]),
                    "prefix": self.scope + "/" if listed else None, "continuation": None})
        return result

    def prepared_history(self):
        """为初始化续作测试提供已确认 prepare 谱系数据，不调用旧 prepare 实现。"""
        from devex_clone_capture import write_json
        from devex_clone_target_state import now
        import uuid
        self.output.mkdir()
        write_json(self.output / "request.json", copy.deepcopy(self.request))
        write_json(self.output / "failure.json", {"status": "needs_reconciliation",
                   "stage": "prepare_binding", "at": now(), "error_type": "FixtureInterruption",
                   "reason": "固定只读中断材料", "automatic_retry": False, "automatic_resource_cleanup": False,
                   "fresh_target_initialized": False, "clone_verified": False, "restore_qualified": False})
        attempt = uuid.uuid4().hex
        intent = self.output / f"resume-prepare-{attempt}.intent.json"
        before = {name: receipt_binding(self.output / name) for name in ("request.json", "failure.json")}
        write_json(intent, {"format_version": 1, "kind": "devex-clone-prepare-resume-intent",
                   "attempt": attempt, "at": now(), "request": receipt_binding(self.path), "files_before": before})
        result = self.prepared()
        write_json(self.output / f"resume-prepare-{attempt}.confirmed.json", {
            "format_version": 1, "kind": "devex-clone-prepare-resume-confirmed", "attempt": attempt,
            "at": now(), "intent": receipt_binding(intent), "prepare": receipt_binding(self.output / "prepare.json"),
            "request": receipt_binding(self.output / "request.json"), "remote_writes": 0})
        return result

    def prepared_command(self, stage, command, stdout):
        from devex_clone_capture import write_json
        import uuid
        write_json(self.output / f"{stage}-{uuid.uuid4().hex}.command.json", {
            "command": command, "returncode": 0, "error_type": None, "stdout": stdout, "stderr": ""})

    def publish_prepared_files(self, request_descriptor: dict | None = None) -> dict:
        """发布与真实 CLI 同形的完整 prepare 文件快照。"""
        descriptor = request_descriptor or {"path": str(self.path), **file_digest(self.path)}
        registration = self.workspace / "registration.json"
        storage_run = self.local / "fresh-storage"
        if not storage_run.exists():
            from devex_clone_run_state import initialize_state
            storage_run.mkdir()
            (storage_run / "manifest.json").write_text(json.dumps({"kind": "offline-run"}), encoding="utf-8")
            initialize_state(storage_run)
        private = self.local / "fresh-environment.json"
        if not private.exists():
            private.write_text(json.dumps({"environment": {"APP_ENV": "test", "TEMP": str(self.local),
                                                           "TMP": str(self.local)}}), encoding="utf-8")
        registered_value = {"format_version": 1, "kind": "devex-clone-fresh-target-registration",
                            "request": descriptor, "environment": receipt_binding(private),
                            "storage_run": {"path": str(storage_run),
                                            "manifest": receipt_binding(storage_run / "manifest.json"),
                                            "state": receipt_binding(storage_run / "state.json")},
                            "target_directory": str(self.output)}
        if not registration.exists():
            registration.write_text(json.dumps(registered_value), encoding="utf-8")
        elif json.loads(registration.read_text(encoding="utf-8")) != registered_value:
            raise AssertionError("测试前像登记不能被重新绑定")
        registered = receipt_binding(registration)
        snapshot = self.workspace / "prepared-files.json"
        snapshot.write_text(json.dumps({
            "format_version": 1, "kind": "devex-clone-fresh-target-prepared-files",
            "registration": registered, "predecessor": None,
            "files": binding.target_files(self.output),
        }), encoding="utf-8")
        self.prepared_files = {"descriptor": receipt_binding(snapshot),
                               "registration": registered, "predecessor": None}
        return self.prepared_files


    def redis(self, args):
        if args[:2] == ["INFO", "server"]:
            return "process_id:200\r\nrun_id:" + ("4" * 40 if self.storage_restarted else "3" * 40) + "\r\nconfig_file:" + self.linux_path(Path(self.request["storage"]["redis"]["configuration"]["path"]))
        if args[:2] == ["CONFIG", "GET"]: return [args[2], self.redis_config[args[2]]]
        if args[0] == "EVAL":
            if (len(args) != 8 or args[2] != "2"
                    or args[5] != self.review["scopes"]["seed"]["redis"]["namespace"] + "*"):
                raise AssertionError(args)
            owner_key, sentinel_key = args[3:5]
            if (any(key.startswith(args[5][:-1]) for key in self.redis_values)
                    or sentinel_key in self.redis_values):
                raise ValueError("fixture Redis 原子重绑前像不是空状态")
            self.redis_values[owner_key], self.redis_values[sentinel_key] = args[6:8]
            return 2
        if args[0] == "GET": return self.redis_values.get(args[1])
        if args[0] == "SET":
            if args[1] in self.redis_values: return None
            self.redis_values[args[1]] = args[2]; return "OK"
        if args[0] == "SCAN": return ["0", [key for key in self.redis_values if key.startswith(args[3][:-1])]]
        raise AssertionError(args)


    def run(self, command, **kwargs):
        self.calls.append(command)
        if "__s3-inspect" in command or "__s3-read" in command:
            request = json.loads(kwargs["input"])
            assert request["format_version"] == 1
            assert request["scope_id"] == self.scope
            assert request["configuration_binding"] == self.object_configuration
            assert not any(name.upper().startswith(("APP_", "AWS_")) for name in kwargs["env"])
            operation, bucket = request["operation"], request["bucket"]
            assert bucket in BUCKETS
            if "__s3-inspect" in command:
                assert request["kind"] == "ryframe-native-s3-inspect"
                assert request["prefix"] == (self.scope + "/" if operation == "list-objects-v2" else None)
                assert request["continuation"] is None
            else:
                assert request["kind"] == "ryframe-native-s3-read"
                assert request["key"] == self.scope + "/.ryframe-owner"
            value = {}
            if operation == "list-objects-v2":
                keys = self.extra_objects.get(bucket, []) + ([self.scope + "/.ryframe-owner"] if self.initialized else [])
                value = {"IsTruncated": False, "Contents": [{"Key": key} for key in keys]}
            elif operation == "get-object":
                Path(request["destination"]).write_bytes(
                    f"ryframe-owner:v1:{self.scope}:object-storage:{bucket}".encode()
                )
                value = {"ContentType": "text/plain", "Metadata": {}, "ETag": '"owner"',
                         "ContentLength": Path(request["destination"]).stat().st_size,
                         "LastModified": "2026-09-04T00:00:00+00:00"}
            elif operation not in ("head-bucket", "get-bucket-versioning"):
                raise AssertionError(operation)
            raw = json.dumps({"ok": True, "result": value}).encode()
            return subprocess.CompletedProcess(command, 0, raw, b"")
        role = Path(command[0]).stem; raw = b""
        if role == "mysql":
            sql = mysql_input(command, kwargs)
            if sql.startswith("SELECT @@"):
                name = sql.split("SCHEMA_NAME = '")[1].split("'")[0]
                rows = [self.uuid, *([name] if name in self.databases else [])]
                raw = ("\r\n".join(rows) + "\r\n").encode()
            else: raise AssertionError("只读夹具拒绝数据库写入")
        elif role == "wsl":
            operation = command[command.index("--exec") + 1]
            linux_config = self.linux_path(Path(self.request["storage"]["redis"]["configuration"]["path"]))
            raw = ({"/usr/bin/cat": "200 (redis-server) " + " ".join(["S"] + ["0"] * 18 + ["67890"]),
                    "/usr/bin/readlink": linux_config if command[-1] == linux_config else "/usr/bin/redis-server",
                    "/usr/bin/sha256sum": (self.request["storage"]["redis"]["configuration"]["sha256"] if command[-1] == linux_config else "9" * 64) + "  " + command[-1]}[operation] + "\r\n").encode()
        else: raise AssertionError(command)
        return subprocess.CompletedProcess(command, 0, raw, b"")
