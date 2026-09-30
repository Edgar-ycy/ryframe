"""为本机运行控制器提供创建身份绑定的通用互斥与死亡恢复。"""

from __future__ import annotations

import json
import os
import re
import uuid
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from full_stack_process import process_identity, write_receipt
from process_guard import process_guard


@dataclass(frozen=True)
class ControllerLockSpec:
    lock_name: str
    guard_name: str
    owner_kind: str
    recovery_kind: str
    recovery_prefix: str

    def validate(self) -> None:
        names = (self.lock_name, self.guard_name, self.recovery_prefix)
        if any(not name or Path(name).name != name or name in {".", ".."} for name in names):
            raise ValueError("运行控制锁名称必须是单一文件名")
        if any(not value for value in (self.owner_kind, self.recovery_kind)):
            raise ValueError("运行控制锁类型不能为空")


def _directory(path: Path) -> Path:
    if not path.is_absolute() or not path.is_dir() or any(
        part.is_symlink() or (hasattr(part, "is_junction") and part.is_junction())
        for part in (path, *path.parents)
    ):
        raise ValueError("运行目录必须是无链接的明确绝对路径")
    return path.resolve(strict=True)


def _read(path: Path) -> dict:
    if not path.exists():
        raise FileNotFoundError(path)
    if not path.is_file() or path.is_symlink() or path.stat().st_size > 64 * 1024:
        raise ValueError("运行控制锁缺少可信 owner 文件")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("运行控制锁 owner 必须是对象")
    return value


def _identity(path: Path) -> tuple[int, int]:
    metadata = path.stat()
    return metadata.st_dev, metadata.st_ino


def _remove(path: Path, identity: tuple[int, int], owner: dict) -> None:
    if (
        _identity(path) != identity
        or _read(path / "owner.json") != owner
        or {entry.name for entry in path.iterdir()} != {"owner.json"}
    ):
        raise ValueError("运行控制锁已变化，保留现场并拒绝清理")
    (path / "owner.json").unlink()
    path.rmdir()


@contextmanager
def controller_lock(directory: Path, operation: str, spec: ControllerLockSpec):
    spec.validate()
    directory = _directory(directory)
    if not operation:
        raise ValueError("运行控制操作不能为空")
    with process_guard(directory, spec.guard_name):
        path = directory / spec.lock_name
        identity = process_identity(os.getpid())
        if identity is None:
            raise ValueError("无法取得控制器的进程创建身份")
        owner = {
            "format_version": 1,
            "kind": spec.owner_kind,
            "identity": identity,
            "runtime_directory": str(directory),
            "operation": operation,
            "token": uuid.uuid4().hex,
        }
        try:
            path.mkdir()
        except FileExistsError as error:
            raise ValueError("运行控制正在执行或异常退出；拒绝并发操作，请先核实控制锁") from error
        lock_identity = _identity(path)
        write_receipt(path / "owner.json", owner)
        try:
            yield owner
        except BaseException as error:
            try:
                _remove(path, lock_identity, owner)
            except BaseException as cleanup:
                error.add_note("运行控制失败后 ownership 锁清理也失败：" + str(cleanup))
            raise
        else:
            _remove(path, lock_identity, owner)


def assert_controller_lock(
    directory: Path,
    owner: dict,
    spec: ControllerLockSpec,
) -> None:
    """供锁内阶段在外部写入前复核同一控制器仍持有原 ownership。"""
    spec.validate()
    directory = _directory(directory)
    path = _directory(directory / spec.lock_name)
    if (
        {entry.name for entry in path.iterdir()} != {"owner.json"}
        or _read(path / "owner.json") != owner
        or owner.get("runtime_directory") != str(directory)
        or process_identity(owner.get("identity", {}).get("pid", 0)) != owner.get("identity")
    ):
        raise ValueError("运行控制锁不再属于当前创建身份")


def reconcile_lock(
    directory: Path,
    spec: ControllerLockSpec,
    *,
    expected_owner: dict | None = None,
) -> dict:
    spec.validate()
    directory = _directory(directory)
    with process_guard(directory, spec.guard_name):
        path = directory / spec.lock_name
        lock_identity = _identity(path)
        owner = _read(path / "owner.json")
        if expected_owner is not None and owner != expected_owner:
            raise ValueError("调用方绑定的控制器 owner 与现场不一致")
        if (
            set(owner) != {"format_version", "kind", "identity", "runtime_directory", "operation", "token"}
            or owner["format_version"] != 1
            or owner["kind"] != spec.owner_kind
            or owner["runtime_directory"] != str(directory)
            or not isinstance(owner["identity"], dict)
            or not isinstance(owner["operation"], str)
            or not owner["operation"]
            or not isinstance(owner["token"], str)
            or re.fullmatch(r"[a-f0-9]{32}", owner["token"]) is None
        ):
            raise ValueError("控制锁缺少已登记的创建身份，必须核实现场")
        identity = owner["identity"]
        if (
            set(identity) != {"pid", "started", "executable"}
            or type(identity.get("pid")) is not int
            or identity["pid"] <= 1
            or not isinstance(identity.get("started"), str)
            or not identity["started"].isdigit()
            or not Path(identity.get("executable", "")).is_absolute()
        ):
            raise ValueError("控制锁的进程创建身份无效")
        actual = process_identity(identity["pid"])
        if actual == identity:
            raise ValueError("控制器仍在运行，禁止释放运行控制锁")
        result = {
            "format_version": 1,
            "kind": spec.recovery_kind,
            "owner": owner,
            "observed_identity": actual,
            "remote_writes_reconciled": False,
        }
        output = directory / f"{spec.recovery_prefix}-{uuid.uuid4().hex}.json"
        write_receipt(output, result)
        _remove(path, lock_identity, owner)
        return {**result, "receipt": str(output)}
