"""用外部 MySQL/AWS CLI 精确复制声明资源，不扫描服务器或替代产品备份引擎。"""

from __future__ import annotations

import configparser
import datetime as dt
import hashlib
import json
import os
import re
import subprocess
import uuid
from pathlib import Path
from urllib.parse import quote, quote_plus

from artifact_digests import filesystem_path
from restore_build import file_digest
from restore_reference_plan import BUCKETS, identifier, safe_file

DATA_HEADER = "-- ryframe-reference-data-v1\n"
COPY_METADATA_FIELDS = {"ContentType", "CacheControl", "ContentDisposition", "ContentEncoding",
                        "ContentLanguage", "Expires", "WebsiteRedirectLocation", "Metadata"}
HEADER_TOKEN = r"[!#$%&'*+.^_`|~0-9A-Za-z-]+"
MIME_PARAMETER = rf"; *({HEADER_TOKEN})=(?:{HEADER_TOKEN}|\"(?:[ !#-\[\]-~]|\\[ -~])*\")"
MIME_TYPE = re.compile(rf"({HEADER_TOKEN})/({HEADER_TOKEN})(?:{MIME_PARAMETER})*")


def copy_object_metadata(value: dict) -> dict:
    """显式保留业务响应头和自定义元数据；缺失与空值不能混为默认值。"""
    if not isinstance(value, dict) or set(value) != COPY_METADATA_FIELDS:
        raise ValueError("复制对象必须完整声明 ContentType、业务响应头和 Metadata")
    result = dict(value)
    for field in COPY_METADATA_FIELDS - {"Metadata"}:
        text = value[field]
        if text is None and field != "ContentType":
            continue
        if (not isinstance(text, str) or not text or text.strip() != text
                or any(ord(char) < 32 or ord(char) > 126 for char in text)):
            raise ValueError("复制对象响应头必须是明确的可打印 ASCII 值，禁止控制字符")
    match = MIME_TYPE.fullmatch(value["ContentType"])
    if not match or "*" in match[1] or "*" in match[2]:
        raise ValueError("复制对象 ContentType 必须是完整媒体类型")
    parameters = [match[1].lower() for match in re.finditer(MIME_PARAMETER, value["ContentType"])]
    if len(parameters) != len(set(parameters)):
        raise ValueError("复制对象 ContentType 参数重复，不能猜测其含义")
    if value["Expires"] is not None:
        text = value["Expires"]
        if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})", text):
            raise ValueError("复制对象 Expires 必须是带时区且精确到秒的 RFC3339")
        instant = dt.datetime.fromisoformat(text.replace("Z", "+00:00"))
        result["Expires"] = instant.astimezone(dt.timezone.utc).isoformat()
    custom = value["Metadata"]
    if not isinstance(custom, dict):
        raise ValueError("复制对象 Metadata 必须显式为空字典或完整键值字典")
    for key, text in custom.items():
        if (not isinstance(key, str) or not re.fullmatch(HEADER_TOKEN, key) or key != key.lower()
                or not isinstance(text, str) or any(ord(char) < 32 or ord(char) > 126 for char in text)):
            raise ValueError("复制对象 Metadata 必须使用唯一小写头名称和可打印 ASCII 值")
    if sum(len(key) + len(text) for key, text in custom.items()) > 2048:
        raise ValueError("复制对象自定义 Metadata 超过 2 KiB")
    result["Metadata"] = dict(sorted(custom.items()))
    return result


class ObjectCreateError(RuntimeError):
    """条件创建没有取得可确认的成功响应；调用方必须保留失败或核对未知结果。"""
    def __init__(self, outcome: str, request: Path, diagnostic: Path):
        self.outcome = outcome
        self.request_file = str(request)
        self.diagnostic_file = str(diagnostic)
        super().__init__(f"对象条件创建未确认：{outcome}；禁止降级或自动重试")


def object_create_outcome(stderr: str) -> str:
    matches = re.findall(r"^(?:aws: \[ERROR\]: )?An error occurred \(([^()\r\n]+)\) when calling the PutObject operation"
                         r"(?: \(reached max retries: [0-9]+\))?:", stderr, re.MULTILINE)
    if len(matches) != 1:
        return "needs_reconciliation"
    return {"PreconditionFailed": "already_exists", "412": "already_exists",
            "ConditionalRequestConflict": "conflict", "409": "conflict",
            "AccessDenied": "denied", "403": "denied"}.get(matches[0], "needs_reconciliation")


def redact_object_diagnostic(stderr, env: dict) -> str:
    text = stderr.decode("utf-8", errors="replace") if isinstance(stderr, bytes) else stderr or ""
    secrets = {value for name, value in env.items() if value and
               re.search(r"KEY|PASSWORD|PASSWD|SECRET|TOKEN|CREDENTIAL|MYSQL_PWD", name, re.IGNORECASE)}
    variants = set()
    for value in secrets:
        variants.update((value, quote(value, safe=""), quote_plus(value, safe=""),
                         json.dumps(value, ensure_ascii=False)[1:-1], json.dumps(value, ensure_ascii=True)[1:-1]))
    for value in sorted(variants, key=len, reverse=True):
        text = text.replace(value, "[REDACTED]")
    return text


class DatabaseVerificationError(ValueError):
    """数据库核验的安全诊断；不携带响应内容、SQL 或连接信息。"""
    def __init__(self, check: str, target_key: str, expected_rows: int, response: str | bytes,
                 *, returncode: int | None = None):
        if check not in {"identity", "ownership"}:
            raise ValueError("数据库诊断类别无效")
        self.check, self.target_key = check, identifier(target_key)
        self.expected_rows, self.actual_rows = expected_rows, len(response.splitlines())
        self.response_bytes = len(response if isinstance(response, bytes) else response.encode("utf-8"))
        self.response_measurement = "raw_stdout" if isinstance(response, bytes) else "mysql_utf8_after_strip"
        self.returncode = returncode
        suffix = "command_failed" if returncode is not None else "mismatch"
        self.code = f"database_{check}_{suffix}"
        super().__init__(f"数据库 {check} 核验失败：{self.code}")

    def safe_details(self) -> dict:
        return {"code": self.code, "check": self.check, "target_key": self.target_key,
                "expected_rows": self.expected_rows, "actual_rows": self.actual_rows,
                "response_bytes": self.response_bytes, "response_measurement": self.response_measurement,
                "returncode": self.returncode}


class VerificationStdout:
    """固定只读 MySQL 的独占输出证据；始终读取原始打开句柄，不按路径重新打开。"""
    def __init__(self, work: Path, check: str, target_key: str):
        self.path = work / ("mysql-" + check + "-" + uuid.uuid4().hex + ".stdout")
        self.check, self.target_key = check, identifier(target_key)
        self.observed, self.failure = None, None
        self._paths()
        self.stream = open(filesystem_path(self.path), "x+b", buffering=0)
        try:
            self.identity = self._state()[:2]
        except BaseException as error:
            try:
                self.stream.close()
            except Exception as failure:
                error.add_note("MySQL 输出构造收尾失败：" + type(failure).__name__)
            raise

    def _paths(self):
        if not self.path.is_absolute() or any(self._linked_or_reparse(path) for path in (self.path, *self.path.parents)):
            raise ValueError("MySQL 输出文件必须使用无链接的明确绝对路径")

    @staticmethod
    def _linked_or_reparse(path: Path) -> bool:
        try:
            state = os.lstat(filesystem_path(path))
        except FileNotFoundError:
            return False
        return os.path.islink(filesystem_path(path)) or bool(getattr(state, "st_file_attributes", 0) & 0x400)

    def _state(self):
        self._paths()
        actual, declared = os.fstat(self.stream.fileno()), os.stat(filesystem_path(self.path))
        # Windows 的 fstat 与路径 stat 对 ctime 的含义不同；绑定文件 ID、大小与修改时间。
        def state(value):
            return value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns
        if state(actual) != state(declared):
            raise ValueError("MySQL 输出路径已不属于原始打开文件")
        return state(actual)

    def fileno(self):
        return self.stream.fileno()

    def write(self, data):
        return self.stream.write(data)

    def snapshot(self):
        if self.failure is not None:
            raise self.failure
        try:
            self.stream.flush()
            os.fsync(self.stream.fileno())
            before = self._state()
            if before[:2] != self.identity:
                raise ValueError("MySQL 输出文件身份发生变化")
            self.stream.seek(0)
            raw = self.stream.read()
            self.stream.seek(0)
            repeated = self.stream.read()
            after = self._state()
            digest = hashlib.sha256(raw).hexdigest()
            if before != after or len(raw) != before[2] or digest != hashlib.sha256(repeated).hexdigest():
                raise ValueError("MySQL 输出文件在读取期间变化")
            artifact = {"path": str(self.path), "bytes": len(raw), "sha256": digest}
            if self.observed is not None and artifact != self.observed:
                raise ValueError("MySQL 输出与已登记的外层命令快照不同")
            self.observed = artifact
            return raw, dict(artifact)
        except Exception as error:
            self.failure = error
            raise

    def receipt(self, returncode, error=None, stderr=None):
        _, diagnostic = verification_stdout(self, None)
        stderr_bytes = len(stderr) if isinstance(stderr, bytes) else None
        stderr_sha256 = hashlib.sha256(stderr).hexdigest() if isinstance(stderr, bytes) else None
        result = {"format_version": 1, "kind": "mysql-verification-output", "check": self.check,
                  "target_key": self.target_key, "returncode": returncode,
                  "error_type": type(error).__name__ if error else None,
                  "stderr_bytes": stderr_bytes, "stderr_sha256": stderr_sha256, **diagnostic}
        receipt = self.path.with_suffix(".json")
        try:
            with open(filesystem_path(receipt), "x", encoding="utf-8", newline="\n") as stream:
                json.dump(result, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            descriptor = {"path": str(receipt), **file_digest(receipt)}
        except Exception as failure:
            if error is None:
                raise
            error.add_note("MySQL 输出收据保存失败：" + type(failure).__name__)
            descriptor = None
        if error is None and self.failure is not None:
            raise self.failure
        return descriptor

    def __enter__(self):
        return self

    def __exit__(self, _kind, error, _traceback):
        try:
            self.stream.close()
        except Exception as failure:
            if error is None:
                raise
            error.add_note("MySQL 输出句柄关闭失败：" + type(failure).__name__)


def verification_stdout(output, fallback):
    """外层诊断共享同一文件快照；采集失败显式登记，不能伪造空 stdout。"""
    if not isinstance(output, VerificationStdout):
        return fallback, {}
    try:
        raw, artifact = output.snapshot()
        return raw, {"stdout_file": artifact}
    except Exception as error:
        output.failure = error
        return None, {"stdout_file": {"path": str(output.path)}, "stdout_capture_error": type(error).__name__}


class ExternalTools:
    def __init__(self, plan: dict, work: Path, run=subprocess.run):
        self.plan, self.work, self.run = plan, work, run

    def command(self, name: str) -> list[str]:
        tool = self.plan["tools"][name]
        if file_digest(Path(tool["path"]))["sha256"] != tool["sha256"]:
            raise ValueError("外部工具在计划确认后发生变化")
        return [tool["path"]]

    def execute(self, command: list[str], *, data: bytes | None = None, output=None, env=None, timeout=1800,
                closed_stdin: bool = False):
        if closed_stdin and data is not None:
            raise ValueError("关闭 stdin 的固定查询不能同时提供输入数据")
        input_stream = {"stdin": subprocess.DEVNULL} if closed_stdin else {}
        return self.run(command, input=data, stdout=output or subprocess.PIPE, stderr=subprocess.PIPE,
                        env=env, timeout=timeout, check=True,
                        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0, **input_stream)

    def _mysql_command(self, database: dict) -> tuple[list[str], dict]:
        defaults = Path(database["defaults_file"])
        self.verify_defaults(database)
        self.validate_defaults(defaults)
        command = [*self.command("mysql"), f"--defaults-file={defaults}",
                   f"--database={identifier(database['database'])}", "--batch", "--raw",
                   "--skip-column-names", "--binary-mode", "--default-character-set=utf8mb4"]
        return command, self.mysql_environment()

    def mysql(self, database: dict, sql: str) -> str:
        command, environment = self._mysql_command(database)
        result = self.execute(command, data=sql.encode(), env=environment)
        return result.stdout.decode("utf-8").strip()

    @staticmethod
    def verify_defaults(database: dict) -> None:
        if file_digest(Path(database["defaults_file"]))["sha256"] != database["defaults_sha256"]:
            raise ValueError("MySQL 连接文件在计划确认后发生变化")

    @staticmethod
    def validate_defaults(path: Path) -> None:
        config = configparser.ConfigParser(interpolation=None)
        try:
            config.read(path, encoding="utf-8")
        except configparser.Error:
            raise ValueError("MySQL 凭据文件格式无效") from None
        allowed = {"host", "port", "user", "password", "ssl-mode", "ssl-ca", "ssl-cert", "ssl-key"}
        if (config.sections() != ["client"] or config.defaults() or set(config["client"]) - allowed
                or config["client"].get("host") not in ("127.0.0.1", "::1")
                or not config["client"].get("user") or not {"password", "port"}.issubset(config["client"])):
            raise ValueError("MySQL 凭据文件只允许本机连接、认证和 TLS 字段")

    def mysql_environment(self) -> dict:
        login_file = self.work / "unused-login.cnf"
        if login_file.exists():
            raise ValueError("参考工具不读取隐式 MySQL login-path 文件")
        return {**{key: value for key, value in os.environ.items() if key != "MYSQL_PWD"},
                "MYSQL_TEST_LOGIN_FILE": str(login_file)}

    def _database_verification_attempt(self, database: dict, check: str, statement: str,
                                       expected_rows: int, command: list[str], environment: dict,
                                       receipts: list[dict]) -> tuple[str, bytes, bytes | None]:
        with VerificationStdout(self.work, check, database["key"]) as output:
            code, stderr = None, None
            try:
                result = self.execute([*command, "--execute", statement], output=output,
                                      env=environment, closed_stdin=True)
                code, stderr = result.returncode, result.stderr
                if code != 0:
                    raise subprocess.CalledProcessError(code, command, stderr=result.stderr)
                raw, _ = output.snapshot()
                response = raw.decode("utf-8").strip()
            except BaseException as error:
                stderr = getattr(error, "stderr", stderr)
                receipt = output.receipt(getattr(error, "returncode", code), error, stderr)
                if receipt is not None:
                    receipts.append(receipt)
                if isinstance(error, subprocess.CalledProcessError) and output.failure is None:
                    raw, _ = output.snapshot()
                    raise DatabaseVerificationError(check, database["key"], expected_rows,
                                                    raw, returncode=error.returncode) from None
                raise
            receipts.append(output.receipt(code, stderr=stderr))
            return response, raw, stderr

    def _write_database_retry(self, database: dict, check: str, receipts: list[dict], error=None) -> None:
        try:
            if len(receipts) != 2:
                raise ValueError("MySQL identity 重试缺少完整的两次输出收据")
            result = {"format_version": 1, "kind": "mysql-verification-retry", "check": check,
                      "target_key": identifier(database["key"]),
                      "reason": "first_returncode_zero_stdout_stderr_exactly_empty",
                      "first": receipts[0], "second": receipts[1]}
            path = self.work / (f"mysql-{check}-retry-" + uuid.uuid4().hex + ".json")
            with path.open("x", encoding="utf-8", newline="\n") as stream:
                json.dump(result, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
        except Exception as failure:
            if error is None:
                raise
            error.add_note("MySQL identity 重试收据保存失败：" + type(failure).__name__)

    def database_verification_response(self, database: dict, check: str) -> str:
        statements = {"identity": "SELECT @@server_uuid, DATABASE();",
                      "ownership": "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;"}
        if not isinstance(check, str) or check not in statements:
            raise ValueError("数据库核验只允许固定 identity 或 ownership 查询")
        expected_rows = 2 if check == "ownership" and database["kind"] == "combined" else 1
        command, environment = self._mysql_command(database)
        # 仅固定只读核验的首次成功空输出重试一次；非空错误、业务 SQL 和写操作不重试。
        receipts = []
        response, raw, stderr = self._database_verification_attempt(
            database, check, statements[check], expected_rows, command, environment, receipts)
        if raw != b"" or stderr != b"":
            return response
        try:
            response, raw, _ = self._database_verification_attempt(
                database, check, statements[check], expected_rows, command, environment, receipts)
        except BaseException as error:
            self._write_database_retry(database, check, receipts, error)
            raise
        if raw == b"":
            error = DatabaseVerificationError(check, database["key"], expected_rows, response)
            self._write_database_retry(database, check, receipts, error)
            raise error
        self._write_database_retry(database, check, receipts)
        return response

    def verify_databases(self, side: str) -> None:
        config = self.plan[side]
        for database in config["databases"]:
            response = self.database_verification_response(database, "identity")
            actual = response.split("\t")
            if actual != [database["server_uuid"], database["database"]]:
                raise DatabaseVerificationError("identity", database["key"], 1, response)
            rows = self.database_verification_response(database, "ownership")
            owners = {tuple(row.split("\t")) for row in rows.splitlines()}
            kinds = {"tenant-data", "control"} if database["kind"] == "combined" else {"tenant-data"}
            expected = {(kind, config["scope_id"], f"ryframe-owner:v1:{config['scope_id']}:{kind}") for kind in kinds}
            if owners != expected:
                raise DatabaseVerificationError("ownership", database["key"], len(expected), rows)

    def aws_context(self, side: str) -> tuple[dict, dict]:
        config = self.plan[side]["s3"]
        access, secret = os.environ.get(config["access_key_env"]), os.environ.get(config["secret_key_env"])
        if not access or not secret:
            raise ValueError("缺少显式外部对象工具凭据环境变量")
        env = {key: value for key, value in os.environ.items()
               if not key.upper().startswith("AWS_") and key.upper() not in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")}
        env.update(AWS_ACCESS_KEY_ID=access, AWS_SECRET_ACCESS_KEY=secret, AWS_DEFAULT_REGION=config["region"],
                   AWS_EC2_METADATA_DISABLED="true", AWS_CONFIG_FILE=os.devnull, AWS_SHARED_CREDENTIALS_FILE=os.devnull,
                   AWS_PAGER="", AWS_MAX_ATTEMPTS="1")
        return config, env

    def aws(self, side: str, operation: str, bucket: str, key: str, destination: Path, *, content_type=None) -> dict:
        scope = self.plan[side]["scope_id"]
        if bucket not in BUCKETS or not key.startswith(scope + "/"):
            raise ValueError("对象操作越过明确的桶和 scope")
        config, env = self.aws_context(side)
        command = [*self.command("aws"), "--endpoint-url", config["endpoint"], "--region", config["region"],
                   "s3api", operation, "--bucket", bucket, "--key", key]
        if operation == "get-object":
            if destination.exists() or destination.is_symlink():
                raise ValueError("外部对象复制不能覆盖已有本地文件")
            command.append(str(destination))
        elif operation == "put-object":
            command.extend(["--body", str(destination), "--content-type", content_type or "application/octet-stream"])
        else:
            raise ValueError("参考对象复制只支持 get-object 和 put-object")
        return json.loads(self.execute(command, env=env).stdout)

    def create_object_if_absent(self, bucket: str, key: str, source: Path, metadata: dict, expected: dict) -> dict:
        """只向目标精确 key 条件创建一次；返回未复验状态，不承担复制事务或重试。"""
        scope = self.plan["target"]["scope_id"]
        if not isinstance(scope, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{2,47}", scope):
            raise ValueError("条件创建必须使用明确有效的目标 scope")
        logical = key.removeprefix(scope + "/") if isinstance(key, str) else ""
        if (bucket not in BUCKETS or not isinstance(key, str) or not key.startswith(scope + "/")
                or logical == ".ryframe-owner" or "\\" in logical
                or any(part in {"", ".", ".."} for part in logical.split("/"))
                or any(ord(char) < 32 or ord(char) == 127 for char in logical)
                or len(key.encode("utf-8")) > 1024):
            raise ValueError("条件创建只能使用目标桶内精确业务 key，不能修改 owner 或越界")
        metadata = copy_object_metadata(metadata)
        if (not isinstance(expected, dict) or set(expected) != {"bytes", "sha256"}
                or type(expected["bytes"]) is not int or expected["bytes"] < 0
                or not isinstance(expected["sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", expected["sha256"])):
            raise ValueError("条件创建必须绑定本地对象的字节数与 SHA")
        for path in (source, *source.parents, self.work, *self.work.parents):
            if path.is_symlink() or (path.exists() and getattr(path.lstat(), "st_file_attributes", 0) & 0x400):
                raise ValueError("条件创建文件和证据目录不能经过链接")
        if not source.is_absolute() or file_digest(source) != expected:
            raise ValueError("条件创建本地对象与计划摘要不同")
        config, env = self.aws_context("target")
        request = self.work / f"object-create-{uuid.uuid4().hex}.json"
        with request.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump({key: value for key, value in metadata.items() if value is not None}, stream,
                      ensure_ascii=False, sort_keys=True)
            stream.write("\n")
        command = [*self.command("aws"), "--endpoint-url", config["endpoint"], "--region", config["region"],
                   "s3api", "put-object", "--bucket", bucket, "--key", key, "--if-none-match", "*",
                   "--body", str(source), "--cli-input-json", "file://" + str(request)]
        diagnostic = request.with_suffix(".diagnostic.json")
        outcome, stderr, returncode, error_type = "needs_reconciliation", b"", None, None
        # 先排他创建诊断文件；无法保存证据时不发出写请求。
        with diagnostic.open("x", encoding="utf-8", newline="\n") as stream:
            try:
                result = self.execute(command, env=env)
                stderr, returncode = result.stderr, result.returncode
                response = json.loads(result.stdout)
                if not isinstance(response, dict) or not isinstance(response.get("ETag"), str) or not response["ETag"]:
                    raise ValueError("条件写响应未确认")
                if file_digest(source) != expected:
                    raise ValueError("条件写期间本地对象变化")
                outcome = "created_unverified"
            except subprocess.CalledProcessError as error:
                stderr, returncode, error_type = error.stderr, error.returncode, type(error).__name__
                text = stderr.decode("utf-8", errors="replace") if isinstance(stderr, bytes) else stderr or ""
                outcome = object_create_outcome(text)
            except (subprocess.TimeoutExpired, OSError, ValueError) as error:
                stderr = getattr(error, "stderr", stderr)
                error_type = type(error).__name__
            json.dump({"format_version": 1, "request_file": str(request), "outcome": outcome,
                       "returncode": returncode, "error_type": error_type,
                       "stderr": redact_object_diagnostic(stderr, env)}, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        if outcome != "created_unverified":
            raise ObjectCreateError(outcome, request, diagnostic) from None
        return {"status": outcome, "response": response, "request_file": str(request), "diagnostic_file": str(diagnostic)}

    def verify_objects(self, side: str) -> None:
        scope = self.plan[side]["scope_id"]
        for bucket in sorted(BUCKETS):
            destination = self.work / f"owner-{side}-{bucket}-{uuid.uuid4().hex}.txt"
            self.aws(side, "get-object", bucket, f"{scope}/.ryframe-owner", destination)
            if destination.read_bytes() != f"ryframe-owner:v1:{scope}:object-storage:{bucket}".encode():
                raise ValueError("对象 ownership 不属于本次明确隔离 scope")

    def dump(self, database: dict, tables: list[str], output: Path) -> None:
        self.verify_defaults(database)
        self.validate_defaults(Path(database["defaults_file"]))
        raw = output.with_suffix(".raw.sql")
        command = [*self.command("mysqldump"), f"--defaults-file={database['defaults_file']}",
                   "--no-create-info", "--skip-triggers", "--skip-lock-tables", "--skip-add-locks",
                   "--skip-comments", "--skip-disable-keys", "--skip-extended-insert", "--complete-insert",
                   "--hex-blob", "--set-gtid-purged=OFF", "--no-tablespaces", "--default-character-set=utf8mb4",
                   identifier(database["database"]), *[identifier(table) for table in tables]]
        with raw.open("xb") as stream:
            self.execute(command, output=stream, env=self.mysql_environment())
        normalize_dump(raw, output, set(tables))
        raw.unlink()

    def restore_database(self, database: dict, tables: list[str], source: Path) -> None:
        validate_dump(source, set(tables))
        # 初始化后的目标种子与业务数据由该备份替换；ownership 与登记表不在 tables 中。
        sql = "SET SESSION FOREIGN_KEY_CHECKS=0; SET SESSION time_zone='+00:00'; "
        sql += "SET SESSION SQL_MODE='NO_AUTO_VALUE_ON_ZERO'; START TRANSACTION;\n"
        sql += "\n".join(f"DELETE FROM `{identifier(table)}`;" for table in tables) + "\n"
        sql += source.read_text(encoding="utf-8") + "\nCOMMIT;\n"
        self.mysql(database, sql)


def validate_insert(line: str, tables: set[str]) -> None:
    match = re.match(r"INSERT INTO `([a-z0-9_-]+)` \((?:`[a-z0-9_]+`(?:, )?)+\) VALUES \(", line)
    if not match or match[1] not in tables or not line.endswith(");"):
        raise ValueError("备份 SQL 只能向清单中的表插入显式列和值")
    quoted, escaped = False, False
    plain = []
    for character in line[match.end():-2]:
        if quoted:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == "'":
                quoted = False
        elif character == "'":
            quoted = True
        else:
            plain.append(character)
    if quoted or re.sub(r"(?:NULL|_binary|0x[0-9A-Fa-f]*|[0-9eE+.,\s-])+", "", "".join(plain)):
        raise ValueError("备份 SQL 包含表达式、多语句或无效字符串")


def normalize_dump(raw: Path, output: Path, tables: set[str]) -> None:
    with raw.open(encoding="utf-8") as source, output.open("x", encoding="utf-8", newline="\n") as target:
        target.write(DATA_HEADER)
        for line in source:
            line = line.rstrip("\r\n")
            if not line or re.fullmatch(r"/\*!\d+ SET .* \*/;", line):
                continue
            validate_insert(line, tables)
            target.write(line + "\n")


def validate_dump(path: Path, tables: set[str]) -> None:
    with path.open(encoding="utf-8") as source:
        if source.readline() != DATA_HEADER:
            raise ValueError("SQL 产物不是已验证的参考数据转储")
        for line in source:
            validate_insert(line.rstrip("\r\n"), tables)


def object_index(root: Path, objects: dict, artifacts: list[dict]) -> dict:
    name = f"objects/{objects['bucket']}/index.json"
    index = json.loads(safe_file(root, name).read_text(encoding="utf-8"))
    expected = {entry["key"]: entry for entry in objects["entries"]}
    if len(index["entries"]) != len(expected) or {entry["key"] for entry in index["entries"]} != set(expected):
        raise ValueError("对象产物索引没有完整对应备份清单")
    declared = {artifact["relative_path"]: artifact for artifact in artifacts}
    for entry in index["entries"]:
        if (entry["file"] not in declared or declared[entry["file"]]["resource"] != f"objects:{objects['bucket']}"
                or any(entry[key] != expected[entry["key"]][key] for key in ("bytes", "sha256"))):
            raise ValueError("对象产物索引与原对象或文件摘要不一致")
        if any(file_digest(safe_file(root, entry["file"]))[key] != entry[key] for key in ("bytes", "sha256")):
            raise ValueError("对象数据文件被篡改")
    return index
