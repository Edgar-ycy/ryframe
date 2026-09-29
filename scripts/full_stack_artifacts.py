"""核对全栈导出物理对象，保存可追溯验收证据。"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.parse
import urllib.request

from reference_fixture_control_protocol import run_private


PROTOCOL_SCHEMAS = {
    "snapshot": (("runtime_dir", "job_id", "receipt"), (), True),
    "verify-deleted": (("runtime_dir", "job_id", "receipt"), (), False),
}


def verify_runtime(backend: Path, runtime: Path) -> dict:
    """延迟导入，避免运行收据与导出物理检查形成模块环。"""
    from full_stack_runtime import verify_runtime as verify

    return verify(backend, runtime)


def required(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise ValueError(f"缺少显式验收配置 {name}")
    return value


def mysql(sql: str) -> str:
    host, port, database = (required(name) for name in (
        "APP_DATABASE_HOST", "APP_DATABASE_PORT", "APP_DATABASE_NAME"
    ))
    if host not in {"127.0.0.1", "localhost"} or not port.isdecimal() or not 1 <= int(port) <= 65535:
        raise ValueError("对象验收数据库必须是明确的本机测试端点")
    if not re.fullmatch(r"[a-zA-Z0-9_]{1,64}", database):
        raise ValueError("测试数据库名称无效")
    tls = required("APP_DATABASE_TLS_MODE")
    if tls not in {"disabled", "preferred", "required"}:
        raise ValueError("对象验收尚不支持需要 CA/主机名参数的 MySQL TLS 模式")
    container = os.environ.get("RYFRAME_CI_MYSQL_CONTAINER_ID")
    if container:
        if os.environ.get("GITHUB_ACTIONS") != "true" or not re.fullmatch(r"[a-f0-9]{12,64}", container):
            raise ValueError("只接受当前 CI 的明确 MySQL 容器")
        command = ["docker", "exec", "-i", "--env", "MYSQL_PWD", container, "mysql"]
        port = "3306"
    else:
        client = Path(required("RYFRAME_E2E_MYSQL_CLIENT"))
        if not client.is_absolute() or not client.is_file():
            raise ValueError("必须提供实际 MySQL 客户端绝对路径")
        command = [str(client)]
    command += [
        "--no-defaults", "--protocol=TCP", "--connect-timeout=5", f"--ssl-mode={tls.upper()}",
        f"--host={host}", f"--port={port}", f"--user={required('APP_DATABASE_USERNAME')}",
        f"--database={database}", "--default-character-set=utf8mb4", "--batch", "--raw",
        "--skip-column-names",
    ]
    result = subprocess.run(
        command,
        input=sql,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=15,
        env={**os.environ, "MYSQL_PWD": required("APP_DATABASE_PASSWORD")},
    )
    if result.returncode:
        raise ValueError("隔离 MySQL 只读验收查询失败")
    return result.stdout.strip()


def database_identity(scope: str) -> str:
    row = mysql(
        "SELECT JSON_OBJECT('server_uuid',@@server_uuid,'scope_id',scope_id,'marker',marker) "
        "FROM ryframe_resource_ownership WHERE resource_kind='control';"
    )
    try:
        owner = json.loads(row)
    except (ValueError, TypeError):
        raise ValueError("控制库 ownership 记录缺失或不唯一") from None
    if (
        not isinstance(owner, dict)
        or owner.get("scope_id") != scope
        or owner.get("marker") != f"ryframe-owner:v1:{scope}:control"
        or not re.fullmatch(r"[a-f0-9-]{36}", owner.get("server_uuid", ""))
    ):
        raise ValueError("控制库 ownership 与测试 scope 不匹配")
    return owner["server_uuid"]


def identifier(value: str) -> str:
    if not re.fullmatch(r"[1-9][0-9]{0,18}", value) or int(value) > 2**63 - 1:
        raise ValueError("必须提供有效的导出 Snowflake ID")
    return value


def export_record(job_id: str) -> dict:
    encoded = mysql(
        "SELECT HEX(JSON_OBJECT('job_id',CAST(e.id AS CHAR),'file_id',CAST(f.id AS CHAR),"
        "'key',f.storage_path,'bucket',f.bucket,'sha256',f.file_sha256,'bytes',f.file_size)) "
        "FROM sys_export_job e JOIN sys_file f ON f.id=e.result_file_id "
        f"WHERE e.id={identifier(job_id)} AND e.status='succeeded' AND e.delete_pending_at IS NULL;"
    )
    if not encoded or "\n" in encoded:
        raise ValueError("导出必须对应唯一的已完成文件")
    return json.loads(bytes.fromhex(encoded).decode("utf-8"))


def validate_key(key: str) -> None:
    if not isinstance(key, str) or not key or any(part in {"", ".", ".."} for part in key.split("/")):
        raise ValueError("对象键包含无效路径")
    if any(value in key for value in ("\\", ":", "\x00")):
        raise ValueError("对象键必须是安全的相对路径")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        raise ValueError("隔离对象请求不得重定向")


def signed_request(method: str, bucket: str, key: str) -> urllib.request.Request:
    endpoint = urllib.parse.urlsplit(required("APP_OBJECT_STORAGE_ENDPOINT"))
    if (
        endpoint.scheme not in {"http", "https"}
        or endpoint.hostname not in {"127.0.0.1", "localhost"}
        or endpoint.path not in {"", "/"}
        or endpoint.query
        or endpoint.fragment
        or endpoint.username
    ):
        raise ValueError("对象验收只接受明确的本机 S3 测试端点")
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    date, region = stamp[:8], os.environ.get("APP_OBJECT_STORAGE_REGION", "us-east-1")
    uri = urllib.parse.quote(f"/{bucket}/{key}", safe="/-_.~")
    empty = hashlib.sha256(b"").hexdigest()
    names = "host;x-amz-content-sha256;x-amz-date"
    headers = f"host:{endpoint.netloc}\nx-amz-content-sha256:{empty}\nx-amz-date:{stamp}\n"
    canonical = f"{method}\n{uri}\n\n{headers}\n{names}\n{empty}"
    scope = f"{date}/{region}/s3/aws4_request"
    signing = f"AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}"
    secret = ("AWS4" + required("APP_OBJECT_STORAGE_SECRET_KEY")).encode()
    for part in (date, region, "s3", "aws4_request"):
        secret = hmac.digest(secret, part.encode(), "sha256")
    signature = hmac.new(secret, signing.encode(), hashlib.sha256).hexdigest()
    authorization = (
        f"AWS4-HMAC-SHA256 Credential={required('APP_OBJECT_STORAGE_ACCESS_KEY')}/{scope}, "
        f"SignedHeaders={names}, Signature={signature}"
    )
    return urllib.request.Request(
        f"{endpoint.scheme}://{endpoint.netloc}{uri}",
        method=method,
        headers={"Authorization": authorization, "X-Amz-Date": stamp, "X-Amz-Content-SHA256": empty},
    )


def object_bytes(scope: str, key: str) -> bytes | None:
    validate_key(key)
    if os.environ.get("APP_OBJECT_STORAGE_BACKEND", "local") == "local":
        base = Path(required("APP_OBJECT_STORAGE_LOCAL_BASE_DIR")).resolve()
        target = (base / "exports" / scope / key).resolve()
        if not target.is_relative_to(base / "exports" / scope):
            raise ValueError("对象路径越出当前 scope")
        if not target.exists():
            return None
        with target.open("rb") as source:
            content = source.read(64 * 1024 * 1024 + 1)
    else:
        request = signed_request("GET", "exports", f"{scope}/{key}")
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        try:
            with opener.open(request, timeout=15) as response:
                content = response.read(64 * 1024 * 1024 + 1)
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return None
            raise ValueError(f"S3 精确对象读取失败（HTTP {error.code}）") from None
    if len(content) > 64 * 1024 * 1024:
        raise ValueError("浏览器验收对象超过 64 MiB 上限")
    return content


def inspect(operation: str, backend: Path, runtime: Path, job_id: str, receipt_path: Path) -> dict:
    from devex_clone_model import local_path

    if operation not in {"snapshot", "verify-deleted"}:
        raise ValueError("未知导出物理对象验收操作")
    backend = backend.resolve(strict=True)
    requested = receipt_path if receipt_path.is_absolute() else backend / receipt_path
    receipt_path = local_path(backend, str(requested))
    if operation == "snapshot" and receipt_path.exists():
        raise FileExistsError(f"导出物理对象收据已存在：{receipt_path}")
    contract = verify_runtime(backend, runtime)
    scope = contract["scope_id"]
    server = database_identity(scope)
    owner = f"ryframe-owner:v1:{scope}:object-storage:exports".encode()
    if object_bytes(scope, ".ryframe-owner") != owner:
        raise ValueError("对象存储 ownership 不匹配")
    if operation == "snapshot":
        record = export_record(job_id)
        if record["bucket"] != "exports":
            raise ValueError("导出产物不属于 exports 桶")
        content = object_bytes(scope, record["key"])
        if (
            content is None
            or len(content) != record["bytes"]
            or hashlib.sha256(content).hexdigest() != record["sha256"]
        ):
            raise ValueError("导出物理对象与文件登记的大小/摘要不匹配")
        receipt = {
            "scope_id": scope,
            "server_uuid": server,
            "database": required("APP_DATABASE_NAME"),
            **record,
        }
        with receipt_path.open("x", encoding="utf-8") as output:
            output.write(json.dumps(receipt, indent=2) + "\n")
        return {"state": "present", "job_id": job_id}
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    bindings = {
        "scope_id": scope,
        "server_uuid": server,
        "database": required("APP_DATABASE_NAME"),
        "job_id": identifier(job_id),
    }
    if any(receipt.get(key) != value for key, value in bindings.items()):
        raise ValueError("删除验收收据与当前资源不匹配")
    remaining = mysql(
        f"SELECT (SELECT COUNT(*) FROM sys_export_job WHERE id={identifier(job_id)}) + "
        f"(SELECT COUNT(*) FROM sys_file WHERE id={identifier(receipt['file_id'])});"
    )
    missing = object_bytes(scope, receipt["key"]) is None
    return {"state": "deleted" if remaining == "0" and missing else "pending", "job_id": job_id}


def main(arguments: list[str]) -> None:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("operation", choices=("snapshot", "verify-deleted"))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--runtime-dir", type=Path, required=True)
    parser.add_argument("--job-id", required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args(arguments)
    if args.operation == "snapshot" and not args.write:
        parser.error("snapshot 私有请求缺少写入授权")
    if args.operation == "verify-deleted" and args.write:
        parser.error("verify-deleted 是只读操作，不接受写入授权")
    print(json.dumps(inspect(args.operation, args.backend_dir, args.runtime_dir, args.job_id, args.receipt)))


if __name__ == "__main__":
    raise SystemExit(
        run_private("artifact", PROTOCOL_SCHEMAS, main, positional_operation=True)
    )
