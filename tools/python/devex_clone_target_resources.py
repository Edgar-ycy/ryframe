"""新目标的固定 MySQL、S3、Redis 操作；不启动服务、不复制业务或删除资源。"""
from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import socket
import subprocess
import uuid

from devex_clone_capture import read_json, write_json
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import application_config_directory
from devex_clone_target_evidence import evidence
from native_s3_client import (NativeS3InvocationError, inspect as native_s3_inspect,
                              read as native_s3_read)
from rate_limit_client import load_app_table
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import BUCKETS, identifier, plan_hash, configuration_binding


def fixture_redis_tool_wsl(backend: Path, cache_binding: dict) -> dict:
    """读取夹具缓存代次请求中由账本签发的 WSL 工具指纹；绑定不符时失败关闭。"""
    outer = read_json(bound_file(backend, cache_binding["generation"]))
    generation = read_json(bound_file(backend, outer["generation"]))
    return read_json(bound_file(backend, generation["redis"]["request"]))["wsl"]


class Resources:
    def __init__(self, backend: Path, request: dict, output: Path, selected: dict, review: dict, run,
                 *, execution_backend: Path | None = None, storage_run: Path | None = None,
                 owned_lock_identity: int | None = None, lock_output: Path | None = None,
                 request_descriptor: dict | None = None):
        self.backend, self.request, self.output = backend, request, output
        self.request_descriptor = request_descriptor
        self.execution_backend = execution_backend or backend
        self.selected, self.review, self.runner = selected, review, run
        self.storage_run, self.storage_runtime_binding = storage_run, None
        self.cache_runtime_binding = None
        self.owned_lock_identity = owned_lock_identity
        self.lock_output = lock_output or output
        self.object_configuration = configuration_binding(self.execution_backend, application_config_directory(backend, request))
        self.environment = dict(os.environ)
        self.redaction = dict(self.environment)
        self.database_credentials = {"backend": str(backend), "databases": copy.deepcopy(request["target"]["databases"])}
        redact_object_diagnostic("", {}, database_credentials=self.database_credentials)
        for field in ("access_key", "secret_key"):
            self.redaction[field.upper()] = request["target"]["s3"][field]
        target = {**request["target"], "s3": {"configuration": self.object_configuration}}
        self.tools = ExternalTools({"target": target, "tools": request["tools"]}, output, run,
                                   before_execute=self.checkpoint)

    def checkpoint(self) -> None:
        """资源访问前复核当前调用方持有的初始化锁与目标 guard。"""
        if self.owned_lock_identity is not None:
            from devex_clone_target_state import generation_checkpoint

            generation_checkpoint(self.lock_output, self.owned_lock_identity)

    def command(self, stage: str, args: list[str], *, data=None, env=None, timeout=30,
                diagnostic: dict | None = None):
        if dict(os.environ) != self.environment:
            raise ValueError("初始化进程环境在执行期间变化")
        if diagnostic is not None:
            diagnostic["phase"] = "pre-command-checkpoint"
        self.checkpoint()
        path = self.output / f"{stage}-{uuid.uuid4().hex}.command.json"
        if diagnostic is not None:
            diagnostic.update({"phase": "command-running", "_receipt_path": str(path),
                               "receipt_path_length": len(str(path)),
                               "receipt_path_limit_risk": len(str(path)) >= 260})
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
            if diagnostic is not None:
                diagnostic["phase"] = "command-receipt-publish"
            write_json(path, {"command": args, "returncode": code, "error_type": error_type,
                             "stdout": redact_object_diagnostic(stdout, self.redaction, database_credentials=self.database_credentials),
                             "stderr": redact_object_diagnostic(stderr, self.redaction, database_credentials=self.database_credentials)})

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


    def drop_empty_database(self, db: dict) -> None:
        """只回收已由当前 fresh 代次确认为空的精确数据库。"""
        self.database_state(db, exists=True, empty=True)
        self._mysql(db, f"DROP DATABASE `{identifier(db['database'])}`;")
        self.database_state(db, exists=False)

    def _s3(self, stage: str, request: dict, invoke) -> dict:
        if dict(os.environ) != self.environment:
            raise ValueError("初始化进程环境在对象观察期间变化")
        self.checkpoint()
        path = self.output / f"{stage}-{uuid.uuid4().hex}.command.json"
        code, error_type, response = None, None, None
        try:
            result, invocation = invoke(request, self.runner)
            code, error_type = invocation["returncode"], invocation["error_type"]
            self.checkpoint()
            response = result
            return result
        except NativeS3InvocationError as error:
            code, error_type = error.returncode, error.error_type
            raise
        finally:
            destination = request.get("destination")
            write_json(path, {"transport": "rust-s3-sigv4", "operation": request["operation"],
                             "scope_id": request["scope_id"],
                             "endpoint": self.request["target"]["s3"]["endpoint"],
                             "configuration_sha256": plan_hash(self.request["target"]["s3"]),
                             "bucket": request["bucket"], "returncode": code,
                             "error_type": error_type, "response": response,
                             "key": request.get("key"),
                             "prefix": request.get("prefix"),
                             "continuation": request.get("continuation"),
                             "destination": None if destination is None else Path(destination).name,
                             "remote_writes": 0})

    def _s3_inspect(self, stage: str, operation: str, bucket: str, *, prefix=None,
                    continuation=None) -> dict:
        if bucket not in BUCKETS:
            raise ValueError("只允许五个明确对象桶")
        target = self.request["target"]
        return self._s3(stage, {
            "format_version": 1, "kind": "ryframe-native-s3-inspect",
            "scope_id": target["scope_id"], "configuration_binding": self.object_configuration,
            "bucket": bucket, "operation": operation, "prefix": prefix,
            "continuation": continuation,
        }, native_s3_inspect)

    def _s3_owner(self, bucket: str, key: str, destination: Path) -> dict:
        target = self.request["target"]
        return self._s3("owner-read", {
            "format_version": 1, "kind": "ryframe-native-s3-read",
            "scope_id": target["scope_id"], "configuration_binding": self.object_configuration,
            "bucket": bucket, "operation": "get-object", "key": key,
            "if_match": None, "version": None, "destination": str(destination),
            "max_bytes": 4096,
        }, native_s3_read)

    def object_keys(self, bucket: str) -> set[str]:
        scope = self.request["target"]["scope_id"] + "/"
        self._s3_inspect("bucket-readiness", "head-bucket", bucket)
        if self._s3_inspect("bucket-versioning", "get-bucket-versioning", bucket).get("Status") is not None:
            raise ValueError("fresh 初始化限定未启用版本的桶，不能隐藏旧对象版本")
        found, tokens, token = set(), set(), None
        for _ in range(10000):
            page = self._s3_inspect("scoped-list", "list-objects-v2", bucket,
                                    prefix=scope, continuation=token)
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
                self._s3_owner(bucket, scope + "/.ryframe-owner", body)
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
        self.checkpoint()
        with socket.create_connection(("127.0.0.1", self.request["storage"]["redis"]["port"]), timeout=5) as connection:
            with connection.makefile("rb") as stream:
                def send(values):
                    encoded = [value.encode() for value in values]
                    self.checkpoint()
                    connection.sendall(f"*{len(encoded)}\r\n".encode() + b"".join(f"${len(v)}\r\n".encode() + v + b"\r\n" for v in encoded))
                    return self._response(stream)
                password = getattr(self, "redis_password", None)
                if password is None:
                    table = load_app_table(
                        self.execution_backend,
                        {"APP_ENV": self.environment.get("APP_ENV")},
                        config_dir=Path(self.request["config_dir"]),
                    )
                    redis = table.get("redis", {})
                    password = redis.get("password") if isinstance(redis, dict) else None
                if not password or send(["AUTH", password]) != "OK" or send(["SELECT", "0"]) != "OK":
                    raise ValueError("Redis 必须使用所选配置文件中的密码和 database 0")
                return send(parts)

    def _cache_cleanup_native(self, observation: dict, comparisons: list[str], output: Path) -> dict:
        """固定写端口持有同一 Redis 连接；不拆分 WATCH/EXEC、不自动重试。"""
        from native_s3_client import _invoke, _read_response

        self.checkpoint()
        result = _read_response(*_invoke("__source-evidence", {
            "operation": "runtime-cache-cleanup", "write": True,
            "arguments": {"backend": str(self.backend), "request": self.request,
                          "directory": str(output.parent), "output": str(output),
                          "environment": {"APP_ENV": self.environment.get("APP_ENV")},
                          "observation": observation, "comparisons": comparisons},
        }, self.runner, may_write=True))[0]["value"]
        self.checkpoint()
        return result

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
        self.checkpoint()
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
            tools_wsl = None
            if fixture:
                from devex_clone_target_runtime_evidence import fixture_restart_generation

                fixture_restart_generation(
                    self.storage_runtime_binding, self.cache_runtime_binding
                )
                # 夹具服务代次可能按续签审阅重启：只允许该代次请求（已经历史校验）签发的
                # WSL 工具指纹随缓存代次更新，其余物理配置仍必须与本目标原始 Redis 一致。
                tools_wsl = fixture_redis_tool_wsl(self.backend, self.cache_runtime_binding)
            else:
                cache_request = json.loads(bound_file(
                    self.backend, self.cache_runtime_binding["request"]
                ).read_text(encoding="utf-8"))
                if cache_request["previous"] != redis:
                    raise ValueError("缓存恢复证明未绑定本目标的原始 Redis")
            resumed = self.cache_runtime_binding["redis"]
            allowed = {**redis, **{key: resumed[key] for key in ("pid", "started", "run_id")}}
            if tools_wsl is not None:
                allowed["wsl"] = tools_wsl
            elif not fixture and self.cache_runtime_binding.get("tools") is not None:
                from devex_clone_cache import generation_request

                output = bound_file(self.backend, self.cache_runtime_binding["tools"]).parent
                process, tools = generation_request(self.backend, self.storage_run, cache_request, output,
                                                    owned_lock_identity=self.owned_lock_identity)
                if tools != self.cache_runtime_binding["tools"]:
                    raise ValueError("缓存工具后继与已发布代次不符")
                allowed["wsl"] = process["wsl"]
            if resumed != allowed:
                raise ValueError("缓存恢复不能改变原 WSL、配置、产物、端口或发行版")
            redis = resumed
        if self.request_descriptor is None:
            raise ValueError("存储布局观察缺少原请求文件绑定")
        request_path = bound_file(self.backend, self.request_descriptor)
        if read_json(request_path) != self.request:
            raise ValueError("存储布局请求与当前绑定文件不同")
        evidence("storage-layout", self.backend, self.output, request=self.request_descriptor,
                 storage={"rustfs": rustfs, "redis": redis}, runtime=self.storage_runtime_binding,
                 cache_runtime=self.cache_runtime_binding)
        self.checkpoint()
        return {"rustfs": rustfs, "redis": redis}
