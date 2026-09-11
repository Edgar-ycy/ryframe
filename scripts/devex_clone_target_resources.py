"""新目标的固定 MySQL、S3、Redis 操作；不启动服务、不复制业务或删除资源。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import socket
import subprocess
import uuid

from devex_clone_capture import read_json, unique_object, write_json
from devex_clone_model import digest, exact
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import external_file
from devex_clone_target_storage import redis_layout, rustfs_layout
from full_stack_process import process_identity
from process_sockets import verify_listener
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import BUCKETS, identifier
from restore_source_binding import defaults_connection


class Resources:
    def __init__(self, backend: Path, request: dict, output: Path, selected: dict, review: dict, run,
                 *, execution_backend: Path | None = None, storage_run: Path | None = None,
                 owned_lock_identity: int | None = None):
        self.backend, self.request, self.output = backend, request, output
        self.execution_backend = execution_backend or backend
        self.selected, self.review, self.runner = selected, review, run
        self.storage_run, self.storage_runtime_binding = storage_run, None
        self.cache_runtime_binding = None
        self.owned_lock_identity = owned_lock_identity
        self.environment = dict(os.environ)
        self.redaction = dict(self.environment)
        for db in request["target"]["databases"]:
            self.redaction["DB_PASSWORD_" + db["key"]] = defaults_connection(backend, db)["password"]
        for field in ("access_key", "secret_key"):
            self.redaction[field.upper()] = self.environment.get(request["target"]["s3"][field + "_env"], "")
        self.tools = ExternalTools({"target": request["target"], "tools": request["tools"]}, output, run)

    def command(self, stage: str, args: list[str], *, data=None, env=None, timeout=30):
        if dict(os.environ) != self.environment:
            raise ValueError("初始化进程环境在执行期间变化")
        path = self.output / f"{stage}-{uuid.uuid4().hex}.command.json"
        stdout, stderr, code, error_type = b"", b"", None, None
        try:
            result = self.runner(args, cwd=self.execution_backend, input=data, env=env or self.environment,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=timeout,
                                 creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            stdout, stderr, code = result.stdout, result.stderr, result.returncode
            return result
        except (OSError, subprocess.SubprocessError) as error:
            stdout, stderr = getattr(error, "stdout", b""), getattr(error, "stderr", b"")
            code, error_type = getattr(error, "returncode", None), type(error).__name__
            raise
        finally:
            write_json(path, {"command": args, "returncode": code, "error_type": error_type,
                             "stdout": redact_object_diagnostic(stdout, self.redaction),
                             "stderr": redact_object_diagnostic(stderr, self.redaction)})

    def _mysql(self, db: dict, sql: str) -> str:
        self.tools.verify_defaults(db)
        self.tools.validate_defaults(Path(db["defaults_file"]))
        args = [*self.tools.command("mysql"), f"--defaults-file={db['defaults_file']}", "--connect-timeout=5",
                "--batch", "--raw", "--skip-column-names", "--binary-mode", "--default-character-set=utf8mb4"]
        # 不指定 database，才能在 CREATE 前观察明确名称；SQL 只来自本模块固定操作。
        return self.command("mysql-" + db["key"], args, data=sql.encode(), env=self.tools.mysql_environment()).stdout.decode("utf-8").strip()

    def database_state(self, db: dict, *, exists: bool, empty: bool = False) -> dict:
        name = identifier(db["database"])
        raw = self._mysql(db, f"SELECT @@server_uuid; SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '{name}';")
        expected = [db["server_uuid"], name] if exists else [db["server_uuid"]]
        if raw.splitlines() != expected:
            raise ValueError("目标 MySQL UUID 或精确数据库存在性不符")
        if empty:
            tables = self._mysql(db, f"SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = '{name}' ORDER BY TABLE_NAME;")
            if tables:
                raise ValueError("本次新库出现非空表或 view，拒绝 reset 接管")
        return {"server_uuid": db["server_uuid"], "database": name, "exists": exists, "empty_checked": empty}

    def create_database(self, db: dict) -> None:
        self.database_state(db, exists=False)
        self._mysql(db, f"CREATE DATABASE `{identifier(db['database'])}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;")

    def drop_empty_database(self, db: dict) -> None:
        """只回收已由当前 fresh 代次确认为空的精确数据库。"""
        self.database_state(db, exists=True, empty=True)
        self._mysql(db, f"DROP DATABASE `{identifier(db['database'])}`;")
        self.database_state(db, exists=False)

    def _aws(self, stage: str, operation: str, bucket: str, *extra: str) -> dict:
        if bucket not in BUCKETS:
            raise ValueError("只允许五个明确对象桶")
        config, env = self.tools.aws_context("target")
        args = [*self.tools.command("aws"), "--endpoint-url", config["endpoint"], "--region", config["region"],
                "--no-paginate", "s3api", operation, "--bucket", bucket, *extra]
        result = self.command(stage, args, env=env)
        return json.loads(result.stdout or b"{}", object_pairs_hook=unique_object)

    def object_keys(self, bucket: str) -> set[str]:
        scope = self.request["target"]["scope_id"] + "/"
        self._aws("bucket-readiness", "head-bucket", bucket)
        if self._aws("bucket-versioning", "get-bucket-versioning", bucket).get("Status") is not None:
            raise ValueError("fresh 初始化限定未启用版本的桶，不能隐藏旧对象版本")
        found, tokens, token = set(), set(), None
        for _ in range(10000):
            extra = ["--prefix", scope, "--max-keys", "1000"]
            if token:
                extra += ["--continuation-token", token]
            page = self._aws("scoped-list", "list-objects-v2", bucket, *extra)
            if page.get("IsTruncated") not in (True, False):
                raise ValueError("对象分页缺少明确完成状态")
            for item in page.get("Contents", []):
                key = item.get("Key")
                if not isinstance(key, str) or not key.startswith(scope) or key in found:
                    raise ValueError("对象列表返回越界或重复 key")
                found.add(key)
            if not page["IsTruncated"]:
                return found
            token = page.get("NextContinuationToken")
            if not isinstance(token, str) or not token or token in tokens:
                raise ValueError("对象分页无法确认完整范围")
            tokens.add(token)
        raise ValueError("对象分页超出有界观察，不能判为空")

    def objects(self, *, initialized: bool) -> dict:
        scope = self.request["target"]["scope_id"]
        result = {}
        for bucket in sorted(BUCKETS):
            keys = self.object_keys(bucket)
            expected = {scope + "/.ryframe-owner"} if initialized else set()
            if keys != expected:
                raise ValueError("目标 scoped 前缀不是本代次预期的空范围或唯一 owner")
            if initialized:
                body = self.output / f"owner-{bucket}-{uuid.uuid4().hex}.bin"
                self._aws("owner-read", "get-object", bucket, "--key", scope + "/.ryframe-owner", str(body))
                expected_owner = f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode()
                if not body.is_file() or body.read_bytes() != expected_owner:
                    raise ValueError("目标对象 owner 字节不符")
                result[bucket] = {"keys": sorted(keys), "owner": file_digest(body)}
            else:
                result[bucket] = {"keys": []}
        return result

    @staticmethod
    def _response(stream, depth=0):
        if depth > 2:
            raise ValueError("Redis 响应嵌套超界")
        line = stream.readline(1048577)
        if not line.endswith(b"\r\n") or len(line) > 1048576:
            raise ValueError("Redis 响应未完整读取")
        kind, value = line[:1], line[1:-2]
        if kind == b"-":
            raise ValueError("Redis 命令失败，不能推断资源缺失")
        if kind == b"+":
            return value.decode("utf-8")
        if kind == b":":
            return int(value)
        if kind == b"$":
            length = int(value)
            if length == -1:
                return None
            if not 0 <= length <= 1048576:
                raise ValueError("Redis 响应长度无效")
            body = stream.read(length + 2)
            if len(body) != length + 2 or not body.endswith(b"\r\n"):
                raise ValueError("Redis 响应截断")
            return body[:-2].decode("utf-8")
        if kind == b"*":
            count = int(value)
            if not 0 <= count <= 10000:
                raise ValueError("Redis 数组长度无效")
            return [Resources._response(stream, depth + 1) for _ in range(count)]
        raise ValueError("未知 Redis 响应")

    def _redis(self, parts: list[str]):
        with socket.create_connection(("127.0.0.1", self.request["storage"]["redis"]["port"]), timeout=5) as connection:
            with connection.makefile("rb") as stream:
                def send(values):
                    encoded = [value.encode() for value in values]
                    connection.sendall(f"*{len(encoded)}\r\n".encode() + b"".join(f"${len(v)}\r\n".encode() + v + b"\r\n" for v in encoded))
                    return self._response(stream)
                password = self.environment.get("APP_REDIS_PASSWORD")
                if not password or send(["AUTH", password]) != "OK" or send(["SELECT", "0"]) != "OK":
                    raise ValueError("Redis 必须使用本侧显式认证和 database 0")
                return send(parts)

    def redis_state(self, *, initialized: bool, sentinel: bool) -> dict:
        redis, reset = self.selected["redis"], self.request["reset"]
        found, cursors, cursor = set(), set(), "0"
        for _ in range(10000):
            page = self._redis(["SCAN", cursor, "MATCH", redis["namespace"] + "*", "COUNT", "1000"])
            if (not isinstance(page, list) or len(page) != 2 or not isinstance(page[0], str)
                    or not page[0].isdigit() or not isinstance(page[1], list)):
                raise ValueError("Redis scoped 分页格式错误")
            for key in page[1]:
                if not isinstance(key, str) or not key.startswith(redis["namespace"]):
                    raise ValueError("Redis scoped 分页越界")
                found.add(key)
            cursor = page[0]
            if cursor == "0":
                break
            if cursor in cursors:
                raise ValueError("Redis scoped 分页无法完成")
            cursors.add(cursor)
        else:
            raise ValueError("Redis scoped 分页超限")
        if found != ({redis["ownership_key"]} if initialized else set()):
            raise ValueError("Redis namespace 出现未知或旧状态")
        owner = self._redis(["GET", redis["ownership_key"]])
        marker = self._redis(["GET", reset["sentinel_key"]])
        if owner != (redis["ownership_value"] if initialized else None) or marker != (reset["sentinel_value"] if sentinel else None):
            raise ValueError("Redis owner 或精确 sentinel 与新代次不符")
        return {"keys": sorted(found), "owner": owner, "sentinel": marker}

    def create_sentinel(self) -> None:
        reset = self.request["reset"]
        if self._redis(["SET", reset["sentinel_key"], reset["sentinel_value"], "NX"]) != "OK":
            raise ValueError("Redis sentinel NX 未确认创建，禁止覆盖或重放")

    def remove_sentinel(self) -> None:
        reset = self.request["reset"]
        if self.redis_state(initialized=False, sentinel=True)["sentinel"] != reset["sentinel_value"]:
            raise ValueError("Redis sentinel 不属于本次 fresh 目标，拒绝删除")
        if self._redis(["DEL", reset["sentinel_key"]]) != 1:
            raise ValueError("Redis sentinel 删除未确认")
        self.redis_state(initialized=False, sentinel=False)

    def _cache_owned_lock_identity(self, cache_request: dict) -> int | None:
        initialized = bound_file(self.backend, cache_request["initialized"])
        prepared = read_json(initialized.parent / "prepare.json")
        original = read_json(bound_file(self.backend, prepared["request"]))
        return self.owned_lock_identity if original == self.request else None

    def storage_identity(self) -> dict:
        rustfs = self.request["storage"]["rustfs"]
        fixture = False
        if self.storage_run is not None:
            manifest = read_json(self.storage_run / "manifest.json")
            fixture = manifest.get("kind") == "reference-fixture-service-run"
            if fixture:
                from reference_fixture_service_context import runtime_transition

                expected = {"storage": self.request["storage"]["rustfs"],
                            "redis": self.request["storage"]["redis"]}
                self.storage_runtime_binding, self.cache_runtime_binding = runtime_transition(
                    self.backend, self.storage_run, expected)
            else:
                from devex_clone_storage import registered_storage_binding

                self.storage_runtime_binding = registered_storage_binding(self.backend, self.storage_run, "target")
            if self.storage_runtime_binding is not None:
                if not fixture:
                    runtime_request = json.loads(bound_file(
                        self.backend, self.storage_runtime_binding["request"]).read_text(encoding="utf-8"))
                    original = self.request["storage"]["rustfs"]
                    if (runtime_request["previous"]["identity"] != original["identity"]
                            or runtime_request["previous"]["process_receipt"] != original["process_receipt"]
                            or runtime_request["previous"]["launch_receipt"] != original["launch_receipt"]):
                        raise ValueError("存储重启证明未绑定本目标的原始 RustFS 收据")
                rustfs = self.storage_runtime_binding["storage"]
        exact(rustfs, {"identity", "sha256", "process_receipt", "launch_receipt"})
        exact(rustfs["identity"], {"pid", "started", "executable"})
        if (rustfs["identity"]["executable"] != self.review["tools"]["rustfs"]["path"]
                or rustfs["sha256"] != self.review["tools"]["rustfs"]["sha256"]
                or file_digest(Path(rustfs["identity"]["executable"]))["sha256"] != digest(rustfs["sha256"])
                or process_identity(rustfs["identity"]["pid"]) != rustfs["identity"]):
            raise ValueError("RustFS 实际二进制或内核进程代次变化")
        verify_listener(rustfs["identity"]["pid"], self.request["target"]["s3"]["endpoint"])
        rustfs_layout(self, rustfs, runtime=self.storage_runtime_binding)
        redis = self.request["storage"]["redis"]
        if self.storage_run is not None and not fixture and self.cache_runtime_binding is None:
            from devex_clone_cache import registered_cache_binding, registration

            cache_lock_identity = None
            if self.owned_lock_identity is not None and (self.storage_run / "cache-target").exists():
                cache_request, _ = registration(self.backend, self.storage_run)
                cache_lock_identity = self._cache_owned_lock_identity(cache_request)
            self.cache_runtime_binding = registered_cache_binding(self.backend, self.storage_run,
                                                                 owned_lock_identity=cache_lock_identity)
            if self.cache_runtime_binding is not None:
                cache_request = json.loads(bound_file(self.backend, self.cache_runtime_binding["request"]).read_text(encoding="utf-8"))
                if cache_request["previous"] != redis:
                    raise ValueError("缓存恢复证明未绑定本目标的原始 Redis")
                resumed = self.cache_runtime_binding["redis"]
                allowed = {**redis, **{key: resumed[key] for key in ("pid", "started", "run_id")}}
                if resumed != allowed:
                    raise ValueError("缓存恢复不能改变原 WSL、配置、产物、端口或发行版")
                redis = resumed
        exact(redis, {"port", "wsl", "distribution", "pid", "started", "executable", "sha256", "run_id", "configuration"})
        external_file(redis["wsl"])
        expected = self.review["tools"]["redis_server"]
        if (redis["distribution"] != expected["distribution"] or redis["executable"] != expected["resolved_path"]
                or redis["sha256"] != expected["sha256"] or type(redis["pid"]) is not int or redis["pid"] <= 1
                or not isinstance(redis["started"], str) or not redis["started"].isdigit()
                or not re.fullmatch(r"[a-f0-9]{40}", redis["run_id"])
                or self.selected["redis"]["url"] != f"redis://127.0.0.1:{redis['port']}/0"):
            raise ValueError("Redis 必须是当前明确 WSL 进程与固定工具")
        prefix = [redis["wsl"]["path"], "--distribution", redis["distribution"], "--exec"]
        def observe(args):
            return self.command("redis-kernel", [*prefix, *args]).stdout.decode("utf-8").strip()
        stat = observe(["/usr/bin/cat", f"/proc/{redis['pid']}/stat"]).rpartition(")")[2].split()
        actual_exe = observe(["/usr/bin/readlink", "-f", f"/proc/{redis['pid']}/exe"])
        executable_hash = observe(["/usr/bin/sha256sum", redis["executable"]]).split()[0]
        info = self._redis(["INFO", "server"])
        values = {}
        for line in info.splitlines():
            if ":" in line and not line.startswith("#"):
                key, value = line.split(":", 1)
                if key in values:
                    raise ValueError("Redis INFO 身份字段重复")
                values[key] = value
        if (len(stat) < 20 or stat[19] != redis["started"] or actual_exe != redis["executable"]
                or executable_hash != redis["sha256"] or values.get("run_id") != redis["run_id"]
                or values.get("process_id") != str(redis["pid"])):
            raise ValueError("Redis 端点或内核进程已重启/变化")
        redis_layout(self, redis, values, observe)
        if process_identity(rustfs["identity"]["pid"]) != rustfs["identity"]:
            raise ValueError("存储观察结束时 RustFS 进程身份变化")
        return {"rustfs": rustfs, "redis": redis}
