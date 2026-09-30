"""以不可覆盖的最终提交点发布监控成功收据。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import stat
import uuid

from restore_runtime_evidence import file_state, reject_link_or_reparse

MAX_RESULT_BYTES = 1024 * 1024
PENDING_NAME = re.compile(r"\.result\.json\.[a-f0-9]{32}\.pending")


def is_result_pending(name: str) -> bool:
    return PENDING_NAME.fullmatch(name) is not None


def _content(value: dict) -> bytes:
    content = (
        json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    if len(content) > MAX_RESULT_BYTES or json.loads(content) != value:
        raise ValueError("监控最终成功收据不能规范化为有效 JSON")
    return content


def _absent(path: Path) -> None:
    try:
        path.lstat()
    except FileNotFoundError:
        return
    raise ValueError("监控最终成功收据已存在，拒绝覆盖或重放")


def _regular_single_link(path: Path, expected_bytes: int):
    metadata = path.lstat()
    if (
        path.is_symlink()
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_nlink != 1
        or metadata.st_size != expected_bytes
        or path.resolve(strict=True) != path
    ):
        raise ValueError("监控最终成功收据临时文件身份无效")
    return metadata


def publish_terminal_success(directory: Path, value: dict) -> None:
    """完整落盘后 create-new 发布；pending 删除成功即为最终提交点。"""
    root = directory.resolve(strict=True)
    reject_link_or_reparse(directory)
    if root != directory or not directory.is_dir():
        raise ValueError("监控最终成功收据必须发布到规范普通 run 目录")
    if any(is_result_pending(item.name) for item in directory.iterdir()):
        raise ValueError("监控最终成功收据存在待核对 pending，拒绝重放")
    target = directory / "result.json"
    _absent(target)
    content = _content(value)
    pending = directory / f".result.json.{uuid.uuid4().hex}.pending"
    with pending.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    before = _regular_single_link(pending, len(content))
    observed = pending.read_bytes()
    after = pending.lstat()
    if observed != content or json.loads(observed) != value or file_state(after) != file_state(before):
        raise ValueError("监控最终成功收据临时文件在提交前发生变化")
    _absent(target)
    try:
        os.link(pending, target)
    except FileExistsError as error:
        raise ValueError("监控最终成功收据被并发创建，拒绝覆盖") from error
    pending_state, target_state = pending.lstat(), target.lstat()
    linked_state = file_state(pending_state)
    observed = target.read_bytes()
    pending_after, target_after = pending.lstat(), target.lstat()
    if (
        linked_state != file_state(target_state)
        or pending_state.st_nlink != 2
        or target_state.st_nlink != 2
        or file_state(pending_after) != linked_state
        or file_state(target_after) != linked_state
        or pending_after.st_nlink != 2
        or target_after.st_nlink != 2
        or observed != content
        or json.loads(observed) != value
    ):
        raise ValueError("监控最终成功收据提交后的文件身份或内容无效")
    pending.unlink()
    return
