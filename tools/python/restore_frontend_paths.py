"""配置读取的普通路径与稳定字节检查，保留原始 .. 之前的每一层。"""
from __future__ import annotations

import errno
import hashlib
import json
import os
from pathlib import Path
import stat

from artifact_digests import filesystem_path

MAX_BYTES = 16 * 1024 * 1024


def failure(label: str, reason: str) -> ValueError:
    return ValueError(f"{label}：{reason}；请修正所指文件、字段、文件类型及权限后重建")


def ordinary(raw: str, *, directory: bool = False, optional: bool = False,
             missing_parents: bool = False) -> Path | None:
    if not isinstance(raw, str) or not raw or any(char in raw for char in "\0\r\n"):
        raise failure("前端配置路径", "必须非空且不含 NUL/CR/LF")
    drive, tail = os.path.splitdrive(raw)
    if drive and not tail.startswith(("/", "\\")):
        raise failure("前端配置路径", "禁止隐式盘符相对目录")
    absolute = raw if os.path.isabs(raw) else str(Path.cwd()) + os.sep + raw
    anchor = Path(absolute).anchor
    parts = absolute[len(anchor):].replace("\\", "/").split("/") if os.name == "nt" else absolute[len(anchor):].split("/")
    parts = [part for part in parts if part not in ("", ".")]
    current = Path(anchor)
    for index, part in enumerate([None, *parts]):
        if part is not None:
            current = current.parent if part == ".." else current / part
        leaf = index == len(parts)
        try:
            info = os.lstat(filesystem_path(current))
        except OSError as error:
            if optional and not directory and (leaf or missing_parents) and error.errno == errno.ENOENT:
                return None
            code = error.winerror if getattr(error, "winerror", None) is not None else error.errno
            raise failure(str(current), f"IO code {code}，必需路径不可读取") from None
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise failure(str(current), "禁止符号链接和所有重解析点")
        expected = stat.S_ISDIR if not leaf or directory else stat.S_ISREG
        if not expected(info.st_mode):
            raise failure(str(current), "必须是普通目录或所选普通文件")
        try:
            resolved = os.path.realpath(current, strict=True)
        except OSError as error:
            raise failure(str(current), f"IO code {error.errno}，无法确认真实来源") from None
        if os.path.normcase(resolved) != os.path.normcase(str(current)):
            raise failure(str(current), "真实路径与普通路径不一致")
    return current


def read_bytes(raw: str, *, optional: bool = False, missing_parents: bool = False) -> bytes | None:
    path = ordinary(raw, optional=optional, missing_parents=missing_parents)
    if path is None:
        return None
    try:
        before = os.stat(filesystem_path(path), follow_symlinks=False)
        with open(filesystem_path(path), "rb") as stream:
            opened = os.fstat(stream.fileno())
            content = stream.read(MAX_BYTES + 1)
            stream.seek(0)
            again = stream.read(MAX_BYTES + 1)
            closed = os.fstat(stream.fileno())
        ordinary(raw)
        after = os.stat(filesystem_path(path), follow_symlinks=False)
    except OSError as error:
        raise failure(str(path), f"IO code {error.errno}，读取未完成") from None
    state = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns)
    if len(content) > MAX_BYTES or len({state(item) for item in (before, opened, closed, after)}) != 1 or content != again:
        raise failure(str(path), "文件超过 16 MiB 或读取期间发生漂移")
    return content


def object_pairs(pairs: list) -> dict:
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("重复 JSON 键")
        value[key] = item
    return value


def json_object(raw: bytes, label: str) -> dict:
    def constant(_value):
        raise ValueError("非法 JSON 数值")

    try:
        value = json.loads(raw.decode("utf-8", errors="strict"), object_pairs_hook=object_pairs,
                           parse_constant=constant)
    except UnicodeError:
        raise failure(label, "UTF8 无效") from None
    except json.JSONDecodeError as error:
        raise failure(label, f"JSON 语法错误 line {error.lineno} column {error.colno}") from None
    except (ValueError, RecursionError):
        raise failure(label, "JSON 重复键或非法数值") from None
    if not isinstance(value, dict):
        raise failure(label, "JSON 必须是对象")
    return value


def descriptor(root: Path, file: Path, raw: bytes) -> dict:
    try:
        name = file.relative_to(root).as_posix()
    except ValueError:
        name = file.as_posix()
    return {"path": name, "sha256": hashlib.sha256(raw).hexdigest()}
