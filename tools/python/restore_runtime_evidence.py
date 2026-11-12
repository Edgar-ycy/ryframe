"""恢复运行核验使用的稳定文件读取、摘要与权威上下文模型。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import stat
import sys
import uuid
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from urllib.parse import urlsplit

from process_sockets import endpoint
from artifact_digests import filesystem_path

MAX_JSON_BYTES = 16 * 1024 * 1024
IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}")
SCOPE_IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9_-]{0,46}[a-z0-9]")
HEX_40 = re.compile(r"[a-f0-9]{40}")
HEX_64 = re.compile(r"[a-f0-9]{64}")
AUTHORITY_FIELDS = {
    "format_version",
    "kind",
    "restore_id",
    "backup_id",
    "plan_hash",
    "scope_id",
    "data_verified_at",
    "backup_source_sha",
    "backend_product_sha",
    "backend_execution_sha",
    "backend_adapter_contract",
    "frontend_sha",
    "api_endpoint",
    "worker_endpoint",
    "frontend_endpoint",
}


def _canonical_json_bytes(value: dict) -> bytes:
    content = (
        json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    if len(content) > MAX_JSON_BYTES:
        raise ValueError("正式恢复输入不能超过 16 MiB")
    return content


def _verify_published_bytes(path: Path, value: dict, content: bytes) -> None:
    with path.open("rb") as stream:
        observed = stream.read(MAX_JSON_BYTES + 1)
    if observed != content:
        raise ValueError("正式恢复输入写后字节与规范结果不同")
    if json.loads(observed) != value:
        raise ValueError("正式恢复输入写后内容与规范结果不同")


def publish_json(target: Path, value: dict) -> None:
    """同目录落盘后以 create-new 语义发布规范 JSON，绝不覆盖已有结果。"""
    content = _canonical_json_bytes(value)
    pending = target.with_name(f".{target.name}.{uuid.uuid4().hex}.pending")
    published = False
    try:
        with pending.open("xb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        _verify_published_bytes(pending, value, content)
        try:
            os.link(pending, target)
        except FileExistsError as error:
            raise ValueError("正式恢复输入已被并发创建，禁止覆盖") from error
        published = True
        try:
            _verify_published_bytes(target, value, content)
            if target.resolve(strict=True) != target:
                raise ValueError("正式恢复输入写后规范路径发生变化")
        except (OSError, ValueError) as error:
            raise ValueError(
                "正式恢复输入已经创建但写后复核失败；必须保留并核对"
            ) from error
    finally:
        try:
            pending.unlink(missing_ok=True)
        except OSError as error:
            if not published:
                raise
            print(f"正式恢复输入已发布，但临时文件清理失败：{error}", file=sys.stderr)


def valid_identifier(value: object) -> bool:
    return isinstance(value, str) and bool(IDENTIFIER.fullmatch(value))


def valid_scope_identifier(value: object) -> bool:
    return isinstance(value, str) and bool(SCOPE_IDENTIFIER.fullmatch(value))


def is_reparse(metadata: os.stat_result) -> bool:
    flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(getattr(metadata, "st_file_attributes", 0) & flag)


def reject_link_or_reparse(path: Path) -> None:
    current = path.absolute()
    while True:
        # 深层验收目录可能超过 MAX_PATH；系统调用统一走扩展路径，返回的仍是原 Path。
        metadata = Path(filesystem_path(current)).lstat()
        if stat.S_ISLNK(metadata.st_mode) or is_reparse(metadata):
            raise ValueError(f"恢复核验输入不得经过符号链接或重解析点：{current}")
        if current.parent == current:
            return
        current = current.parent


def file_state(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns


def strict_object_pairs(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("恢复核验 JSON 包含重复字段；请删除重复字段")
        result[key] = value
    return result


def reject_json_constant(_value: str) -> None:
    raise ValueError("恢复核验 JSON 包含非标准数值；请使用严格 JSON")


def decode_object(raw: bytes, label: str) -> dict:
    try:
        value = json.loads(
            raw.decode("utf-8", errors="strict"),
            object_pairs_hook=strict_object_pairs,
            parse_constant=reject_json_constant,
        )
    except UnicodeDecodeError:
        raise ValueError(f"{label}不是 UTF-8；请修正文件编码") from None
    except json.JSONDecodeError as error:
        raise ValueError(f"{label}不是严格 JSON：line {error.lineno} column {error.colno}；请修正语法") from None
    except (ValueError, RecursionError):
        raise ValueError(f"{label}含重复字段、非标准数值或过深嵌套；请修正严格 JSON") from None
    if not isinstance(value, dict):
        raise ValueError(f"{label}必须是 JSON 对象")
    return value


@dataclass(frozen=True)
class JsonDocument:
    path: Path
    raw: bytes
    value: dict
    state: tuple[int, int, int, int]

    @property
    def sha256(self) -> str:
        return hashlib.sha256(self.raw).hexdigest()

    def assert_unchanged(self) -> None:
        current = read_json_document(self.path)
        if current.state != self.state or current.raw != self.raw:
            raise ValueError(f"核验期间 JSON 输入被替换或修改：{self.path}")


def read_json_document(path: Path) -> JsonDocument:
    path = path.absolute()
    reject_link_or_reparse(path)
    before = Path(filesystem_path(path)).lstat()
    if not stat.S_ISREG(before.st_mode):
        raise ValueError(f"恢复核验输入必须是普通文件：{path}")
    if before.st_size == 0:
        raise ValueError(f"恢复核验输入不能为空：{path}")
    if before.st_size > MAX_JSON_BYTES:
        raise ValueError(f"恢复核验输入超过 16 MiB：{path}")
    with open(filesystem_path(path), "rb") as stream:
        opened = os.fstat(stream.fileno())
        if file_state(opened) != file_state(before) or not stat.S_ISREG(opened.st_mode):
            raise ValueError(f"恢复核验输入在打开前被替换：{path}")
        raw = stream.read(MAX_JSON_BYTES + 1)
        stream.seek(0)
        repeated = stream.read(MAX_JSON_BYTES + 1)
        after_read = os.fstat(stream.fileno())
    reject_link_or_reparse(path)
    after = Path(filesystem_path(path)).lstat()
    state = file_state(before)
    if raw != repeated or len(raw) > MAX_JSON_BYTES or file_state(after_read) != state or file_state(after) != state:
        raise ValueError(f"恢复核验输入在读取期间被替换或修改：{path}")
    return JsonDocument(path, raw, decode_object(raw, f"恢复核验输入 {path}"), state)


def read_json(path: Path) -> dict:
    return read_json_document(path).value


@dataclass(frozen=True)
class ArtifactSnapshot:
    path: Path
    bytes: int
    sha256: str
    state: tuple[int, int, int, int]

    def descriptor(self) -> dict:
        return {"path": str(self.path), "bytes": self.bytes, "sha256": self.sha256}

    def assert_unchanged(self) -> None:
        current = artifact_snapshot(self.path)
        if current.state != self.state or current.bytes != self.bytes or current.sha256 != self.sha256:
            raise ValueError(f"核验期间产物被替换或修改：{self.path}")


def artifact_snapshot(path: Path) -> ArtifactSnapshot:
    path = path.absolute()
    reject_link_or_reparse(path)
    before = Path(filesystem_path(path)).lstat()
    if not stat.S_ISREG(before.st_mode):
        raise ValueError(f"恢复核验产物必须是普通文件：{path}")
    digest = hashlib.sha256()
    with open(filesystem_path(path), "rb") as stream:
        opened = os.fstat(stream.fileno())
        if file_state(opened) != file_state(before):
            raise ValueError(f"恢复核验产物在打开前被替换：{path}")
        while block := stream.read(1024 * 1024):
            digest.update(block)
        after_read = os.fstat(stream.fileno())
    reject_link_or_reparse(path)
    after = Path(filesystem_path(path)).lstat()
    state = file_state(before)
    if file_state(after_read) != state or file_state(after) != state:
        raise ValueError(f"恢复核验产物在读取期间被替换或修改：{path}")
    return ArtifactSnapshot(path, before.st_size, digest.hexdigest(), state)


def directory(path: Path, label: str) -> Path:
    path = path.absolute()
    reject_link_or_reparse(path)
    if not Path(filesystem_path(path)).is_dir():
        raise ValueError(f"{label}必须是普通目录")
    return path


def exact_fields(value: object, fields: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError(f"{label}字段必须精确匹配当前格式")
    return value


def timestamp(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label}必须是带时区的时间字符串")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError(f"{label}不是有效时间") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ValueError(f"{label}必须包含时区")
    return value


def canonical_endpoint(url: object, path: str, label: str) -> str:
    if not isinstance(url, str):
        raise ValueError(f"{label}必须是字符串")
    _family, host, port = endpoint(url)
    parsed = urlsplit(url)
    if parsed.port is None or parsed.path != path:
        raise ValueError(f"{label}必须使用显式端口和精确路径 {path or '/'}")
    authority = f"[{host}]" if ":" in host else host
    canonical = f"{parsed.scheme}://{authority}:{port}{path}"
    if url != canonical:
        raise ValueError(f"{label}必须使用规范化 loopback 地址")
    return canonical


def validate_authority(value: object) -> dict:
    authority = exact_fields(value, AUTHORITY_FIELDS, "权威恢复上下文")
    if authority["format_version"] != 2 or authority["kind"] != "restore-runtime-authority":
        raise ValueError("权威恢复上下文版本或类型不匹配")
    for field in ("restore_id", "backup_id"):
        if not valid_identifier(authority[field]):
            raise ValueError(f"权威恢复上下文的 {field} 无效")
    if not valid_scope_identifier(authority["scope_id"]):
        raise ValueError("权威恢复上下文的 scope_id 无效")
    for field, pattern in (
        ("plan_hash", HEX_64),
        ("backup_source_sha", HEX_40),
        ("backend_product_sha", HEX_40),
        ("backend_execution_sha", HEX_40),
        ("frontend_sha", HEX_40),
    ):
        if not isinstance(authority[field], str) or not pattern.fullmatch(authority[field]):
            raise ValueError(f"权威恢复上下文的 {field} 无效")
    adapter = authority["backend_adapter_contract"]
    same_backend = authority["backend_product_sha"] == authority["backend_execution_sha"]
    if (same_backend and adapter is not None) or (
        not same_backend and adapter != "legacy-stable-readiness-b0-v1"
    ):
        raise ValueError("权威恢复上下文的后端产品、执行来源与适配合同不一致")
    timestamp = authority["data_verified_at"]
    if not isinstance(timestamp, str):
        raise ValueError("权威恢复上下文缺少 data_verified_at")
    try:
        parsed_time = datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("权威恢复上下文的 data_verified_at 无效") from error
    if parsed_time.tzinfo is None or parsed_time.utcoffset() is None:
        raise ValueError("权威恢复上下文的 data_verified_at 必须包含时区")
    endpoints = {
        "api": canonical_endpoint(authority["api_endpoint"], "/readyz", "API 端点"),
        "worker": canonical_endpoint(authority["worker_endpoint"], "/readyz", "Worker 端点"),
        "frontend": canonical_endpoint(authority["frontend_endpoint"], "", "前端端点"),
    }
    if len({urlsplit(url).port for url in endpoints.values()}) != 3:
        raise ValueError("API、Worker 与前端必须使用三个互异的显式端口")
    return authority


def process_identity_record(value: object, label: str) -> dict:
    identity = exact_fields(value, {"pid", "started", "executable"}, label)
    if (
        type(identity["pid"]) is not int
        or identity["pid"] <= 1
        or not isinstance(identity["started"], str)
        or not identity["started"]
        or not isinstance(identity["executable"], str)
        or not Path(identity["executable"]).is_absolute()
    ):
        raise ValueError(f"{label}无效")
    return identity


def process_document(path: Path, role: str, scope: str) -> tuple[JsonDocument, dict]:
    document = read_json_document(path)
    receipt = exact_fields(document.value, {"format_version", "role", "scope_id", "identity"}, "进程收据")
    if (
        type(receipt["format_version"]) is not int
        or receipt["format_version"] != 1
        or receipt["role"] != role
        or receipt["scope_id"] != scope
    ):
        raise ValueError("进程收据的版本、角色或隔离 scope 不匹配")
    return document, process_identity_record(receipt["identity"], f"{role} 进程身份")
