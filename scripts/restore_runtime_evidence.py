"""恢复运行核验使用的稳定文件读取、摘要与权威上下文模型。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import stat
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path, PurePosixPath
from urllib.parse import urlsplit

from process_sockets import endpoint
from restore_identifiers import valid_identifier, valid_scope_identifier

MAX_JSON_BYTES = 16 * 1024 * 1024
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
    "backend_sha",
    "frontend_sha",
    "api_endpoint",
    "worker_endpoint",
    "frontend_endpoint",
}


def is_reparse(metadata: os.stat_result) -> bool:
    flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(getattr(metadata, "st_file_attributes", 0) & flag)


def reject_link_or_reparse(path: Path) -> None:
    current = path.absolute()
    while True:
        metadata = current.lstat()
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
            raise ValueError(f"恢复核验 JSON 包含重复字段：{key}")
        result[key] = value
    return result


def reject_json_constant(value: str) -> None:
    raise ValueError(f"恢复核验 JSON 包含非标准数值：{value}")


def decode_object(raw: bytes, label: str) -> dict:
    try:
        value = json.loads(
            raw.decode("utf-8", errors="strict"),
            object_pairs_hook=strict_object_pairs,
            parse_constant=reject_json_constant,
        )
    except UnicodeDecodeError as error:
        raise ValueError(f"{label}不是 UTF-8") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"{label}不是严格 JSON") from error
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
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode):
        raise ValueError(f"恢复核验输入必须是普通文件：{path}")
    if before.st_size == 0:
        raise ValueError(f"恢复核验输入不能为空：{path}")
    if before.st_size > MAX_JSON_BYTES:
        raise ValueError(f"恢复核验输入超过 16 MiB：{path}")
    with path.open("rb") as stream:
        opened = os.fstat(stream.fileno())
        if file_state(opened) != file_state(before) or not stat.S_ISREG(opened.st_mode):
            raise ValueError(f"恢复核验输入在打开前被替换：{path}")
        raw = stream.read(MAX_JSON_BYTES + 1)
        stream.seek(0)
        repeated = stream.read(MAX_JSON_BYTES + 1)
        after_read = os.fstat(stream.fileno())
    reject_link_or_reparse(path)
    after = path.lstat()
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
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode):
        raise ValueError(f"恢复核验产物必须是普通文件：{path}")
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        opened = os.fstat(stream.fileno())
        if file_state(opened) != file_state(before):
            raise ValueError(f"恢复核验产物在打开前被替换：{path}")
        while block := stream.read(1024 * 1024):
            digest.update(block)
        after_read = os.fstat(stream.fileno())
    reject_link_or_reparse(path)
    after = path.lstat()
    state = file_state(before)
    if file_state(after_read) != state or file_state(after) != state:
        raise ValueError(f"恢复核验产物在读取期间被替换或修改：{path}")
    return ArtifactSnapshot(path, before.st_size, digest.hexdigest(), state)


def directory(path: Path, label: str) -> Path:
    path = path.absolute()
    reject_link_or_reparse(path)
    if not path.is_dir():
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


def same_path(value: object, expected: Path, label: str) -> None:
    if not isinstance(value, str) or not Path(value).is_absolute() or Path(value).absolute() != expected.absolute():
        raise ValueError(f"恢复运行收据的 {label} 路径不匹配")


def digest_matches(value: object, document: JsonDocument, label: str) -> None:
    if not isinstance(value, str) or not HEX_64.fullmatch(value) or value != document.sha256:
        raise ValueError(f"恢复运行收据的 {label} 摘要不匹配")


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
    if authority["format_version"] != 1 or authority["kind"] != "restore-runtime-authority":
        raise ValueError("权威恢复上下文版本或类型不匹配")
    for field in ("restore_id", "backup_id"):
        if not valid_identifier(authority[field]):
            raise ValueError(f"权威恢复上下文的 {field} 无效")
    if not valid_scope_identifier(authority["scope_id"]):
        raise ValueError("权威恢复上下文的 scope_id 无效")
    for field, pattern in (("plan_hash", HEX_64), ("backend_sha", HEX_40), ("frontend_sha", HEX_40)):
        if not isinstance(authority[field], str) or not pattern.fullmatch(authority[field]):
            raise ValueError(f"权威恢复上下文的 {field} 无效")
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


def read_authority(stream=None) -> dict:
    source = stream if stream is not None else sys.stdin
    raw = source.buffer.read(MAX_JSON_BYTES + 1) if hasattr(source, "buffer") else source.read(MAX_JSON_BYTES + 1)
    if isinstance(raw, str):
        raw = raw.encode("utf-8")
    if not raw or len(raw) > MAX_JSON_BYTES:
        raise ValueError("verify 必须从 stdin 接收非空且不超过 16 MiB 的权威恢复上下文")
    return validate_authority(decode_object(raw, "stdin 权威恢复上下文"))


def validate_frontend_receipt(receipt: object) -> dict:
    value = exact_fields(receipt, {"format_version", "kind", "source", "files"}, "前端构建收据")
    if value["format_version"] != 1 or value["kind"] != "restore-frontend-build" or not isinstance(value["files"], list):
        raise ValueError("前端必须使用生产构建收据")
    previous = ""
    for item in value["files"]:
        item = exact_fields(item, {"path", "bytes", "sha256"}, "前端文件收据")
        relative = PurePosixPath(item["path"]) if isinstance(item["path"], str) else None
        if (
            relative is None
            or relative.is_absolute()
            or ".." in relative.parts
            or "\\" in item["path"]
            or relative.as_posix() != item["path"]
            or item["path"] <= previous
            or type(item["bytes"]) is not int
            or item["bytes"] < 0
            or not isinstance(item["sha256"], str)
            or not HEX_64.fullmatch(item["sha256"])
        ):
            raise ValueError("前端文件收据必须有序且包含有效摘要")
        previous = item["path"]
    return value


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
    if receipt["format_version"] != 1 or receipt["role"] != role or receipt["scope_id"] != scope:
        raise ValueError("进程收据的版本、角色或隔离 scope 不匹配")
    return document, process_identity_record(receipt["identity"], f"{role} 进程身份")


def validate_backend_receipt(receipt: object) -> dict:
    value = exact_fields(
        receipt,
        {"format_version", "kind", "source", "source_inventory", "artifacts"},
        "后端构建收据",
    )
    if value["format_version"] != 1 or value["kind"] != "restore-backend-build":
        raise ValueError("后端构建收据版本或类型不匹配")
    source = exact_fields(value["source"], {"head", "patch_sha256", "files", "clean"}, "后端源码快照")
    if (
        not isinstance(source["head"], str)
        or not HEX_40.fullmatch(source["head"])
        or not isinstance(source["patch_sha256"], str)
        or not HEX_64.fullmatch(source["patch_sha256"])
        or not isinstance(source["files"], list)
        or type(source["clean"]) is not bool
    ):
        raise ValueError("后端源码快照字段无效")
    artifacts = exact_fields(value["artifacts"], {"api", "worker"}, "后端构建产物")
    for role in ("api", "worker"):
        artifact = exact_fields(
            artifacts[role],
            {"executable", "command", "bytes", "sha256"},
            f"{role} 构建产物",
        )
        if (
            not isinstance(artifact["executable"], str)
            or not Path(artifact["executable"]).is_absolute()
            or not isinstance(artifact["command"], list)
            or type(artifact["bytes"]) is not int
            or artifact["bytes"] < 0
            or not isinstance(artifact["sha256"], str)
            or not HEX_64.fullmatch(artifact["sha256"])
        ):
            raise ValueError(f"{role} 构建产物字段无效")
    return value


def validate_runtime_receipt(receipt: object) -> dict:
    value = exact_fields(
        receipt,
        {"format_version", "kind", "restore", "paths", "digests", "source", "endpoints", "backend", "frontend", "processes"},
        "恢复运行收据",
    )
    if value["format_version"] != 2 or value["kind"] != "restore-runtime":
        raise ValueError("恢复运行收据版本或类型不匹配")
    restore = exact_fields(
        value["restore"],
        {"id", "backup_id", "plan_hash", "scope_id", "data_verified_at"},
        "恢复绑定",
    )
    paths = exact_fields(
        value["paths"],
        {"backend_root", "frontend_root", "runtime_dir", "bindings", "backend_build", "frontend_build"},
        "恢复路径绑定",
    )
    digests = exact_fields(value["digests"], {"bindings", "backend_build", "frontend_build"}, "恢复摘要绑定")
    source = exact_fields(value["source"], {"backend_sha", "frontend_sha"}, "恢复源码绑定")
    endpoints = exact_fields(value["endpoints"], {"api", "worker", "frontend"}, "恢复端点绑定")
    validate_authority(
        {
            "format_version": 1,
            "kind": "restore-runtime-authority",
            "restore_id": restore["id"],
            "backup_id": restore["backup_id"],
            "plan_hash": restore["plan_hash"],
            "scope_id": restore["scope_id"],
            "data_verified_at": restore["data_verified_at"],
            "backend_sha": source["backend_sha"],
            "frontend_sha": source["frontend_sha"],
            "api_endpoint": endpoints["api"],
            "worker_endpoint": endpoints["worker"],
            "frontend_endpoint": endpoints["frontend"],
        }
    )
    if any(not isinstance(path, str) or not Path(path).is_absolute() for path in paths.values()):
        raise ValueError("恢复运行收据必须保存绝对路径")
    if any(not isinstance(digest, str) or not HEX_64.fullmatch(digest) for digest in digests.values()):
        raise ValueError("恢复运行收据摘要无效")
    processes = exact_fields(value["processes"], {"api", "worker"}, "恢复进程绑定")
    for role in ("api", "worker"):
        item = exact_fields(processes[role], {"receipt_path", "receipt_sha256", "identity"}, f"{role} 进程绑定")
        if (
            not isinstance(item["receipt_path"], str)
            or not Path(item["receipt_path"]).is_absolute()
            or not isinstance(item["receipt_sha256"], str)
            or not HEX_64.fullmatch(item["receipt_sha256"])
        ):
            raise ValueError(f"{role} 进程收据路径或摘要无效")
        process_identity_record(item["identity"], f"{role} 进程身份")
    validate_backend_receipt(value["backend"])
    validate_frontend_receipt(value["frontend"])
    return value
