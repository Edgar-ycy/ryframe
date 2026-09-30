"""以最终 create-new 提交点发布 Device 浏览器成功收据。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import stat
from typing import Callable
import uuid

from devex_clone_model import linked
from restore_runtime_evidence import file_state


MAX_RECEIPT_BYTES = 16 * 1024 * 1024
RESULT_NAME = re.compile(r"browser-[a-z0-9][a-z0-9-]{0,63}-result\.json")


def _content(value: dict) -> bytes:
    content = (
        json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    if len(content) > MAX_RECEIPT_BYTES:
        raise ValueError("Device 浏览器成功收据不能超过 16 MiB")
    if json.loads(content) != value:
        raise ValueError("Device 浏览器成功收据不能规范化为相同 JSON")
    return content


def _absent(path: Path) -> None:
    try:
        path.lstat()
    except FileNotFoundError:
        return
    raise ValueError("Device 浏览器成功收据已存在，拒绝覆盖或重放")


def _pending(target: Path, content: bytes) -> Path:
    pending = target.with_name(f"{target.name}.pending-{uuid.uuid4().hex}")
    with pending.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    before = pending.lstat()
    if (linked(pending) or not stat.S_ISREG(before.st_mode) or before.st_nlink != 1
            or pending.resolve(strict=True) != pending or before.st_size != len(content)):
        raise ValueError("Device 浏览器成功收据临时文件身份无效")
    observed = pending.read_bytes()
    after = pending.lstat()
    if (observed != content or json.loads(observed) != json.loads(content)
            or file_state(after) != file_state(before) or after.st_nlink != 1):
        raise ValueError("Device 浏览器成功收据临时文件在提交前发生变化")
    return pending


def _unlink_pending(path: Path) -> None:
    path.unlink()


def publish_terminal_success(directory: Path, target: Path, value: dict,
                             on_failure: Callable[[BaseException], None]) -> None:
    """发布成功后只删除同目录 pending；删除完成即为最终提交点。"""
    try:
        root = directory.resolve(strict=True)
        if (linked(directory) or not root.is_dir() or not target.is_absolute()
                or target.parent.resolve(strict=True) != root or target.parent != directory
                or RESULT_NAME.fullmatch(target.name) is None):
            raise ValueError("Device 浏览器成功收据路径不属于当前普通运行目录")
        pending = _pending(target, _content(value))
        if pending.parent != directory or pending.parent.resolve(strict=True) != root:
            raise ValueError("Device 浏览器成功收据临时文件不在同一运行目录")
        _absent(target)
        try:
            os.link(pending, target)
        except FileExistsError as error:
            raise ValueError("Device 浏览器成功收据被并发创建，拒绝覆盖") from error
    except BaseException as error:
        on_failure(error)
        raise
    try:
        _unlink_pending(pending)
    except OSError as error:
        on_failure(error)
        raise
    return
