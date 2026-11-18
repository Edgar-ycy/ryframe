"""恢复与导出核验共用的稳定文件读取、摘要和严格 JSON。"""

from __future__ import annotations

import hashlib
import json
import os
import stat
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from artifact_digests import filesystem_path

MAX_JSON_BYTES = 16 * 1024 * 1024


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
