"""登记复制锁的内核持有人并显式回收死锁；不核对、重放或清理远端数据。"""
from __future__ import annotations

from contextlib import ExitStack
from dataclasses import dataclass
import json
import os
from pathlib import Path
import uuid

from devex_clone_model import digest, exact, linked, local_path, name
from full_stack_process import process_identity, write_receipt
from process_guard import process_guard
from restore_build import file_digest

GUARDS = {"target": "copy-target.guard", "ledger": "copy-ledger.guard"}


@dataclass(frozen=True)
class CopyOwner:
    backend: Path
    binding: dict
    session_id: str


def _read(path: Path) -> dict:
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("复制锁收据超过大小上限")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("复制锁收据必须是对象")
    return value


def _bound(backend: Path, binding: dict) -> Path:
    exact(binding, {"path", "bytes", "sha256"})
    path = local_path(backend, binding["path"])
    if file_digest(path) != {key: binding[key] for key in ("bytes", "sha256")}:
        raise ValueError("复制锁来源收据已变化")
    return path


def _binding(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def register_copy_owner(backend: Path, copy_directory: Path) -> CopyOwner:
    root = local_path(backend, str(copy_directory))
    session = root / "session.json"
    saved = _read(session)
    for field in ("plan_sha256", "generation_sha256"):
        digest(saved[field])
    _bound(backend, saved["source_export"])
    target = _bound(backend, saved["initialized"])
    if target.name != "initialized.json":
        raise ValueError("复制锁必须绑定精确初始化收据")
    identity = process_identity(os.getpid())
    if identity is None:
        raise ValueError("无法取得复制控制器的内核创建身份")
    session_id = "session-" + uuid.uuid4().hex
    value = {"format_version": 1, "kind": "devex-copy-owner", "backend_root": str(backend.resolve()),
             "copy_directory": str(root), "session": _binding(session), "session_id": session_id,
             "source_export": saved["source_export"], "initialized": saved["initialized"],
             "plan_sha256": saved["plan_sha256"], "generation_sha256": saved["generation_sha256"],
             "controller": identity}
    path = root / ("copy-owner-" + session_id.removeprefix("session-") + ".json")
    write_receipt(path, value)
    return CopyOwner(backend.resolve(), _binding(path), session_id)


def read_copy_owner(owner: CopyOwner) -> tuple[dict, dict[str, Path]]:
    value = _read(_bound(owner.backend, owner.binding))
    exact(value, {"format_version", "kind", "backend_root", "copy_directory", "session", "session_id",
                  "source_export", "initialized", "plan_sha256", "generation_sha256", "controller"})
    root = local_path(owner.backend, value["copy_directory"])
    if (value["format_version"] != 1 or value["kind"] != "devex-copy-owner"
            or value["backend_root"] != str(owner.backend.resolve()) or name(value["session_id"]) != owner.session_id
            or _bound(owner.backend, value["session"]) != root / "session.json"
            or Path(owner.binding["path"]).parent != root):
        raise ValueError("复制锁持有人不属于明确的目录与会话")
    saved = _read(root / "session.json")
    if any(saved[field] != value[field] for field in ("source_export", "initialized", "plan_sha256", "generation_sha256")):
        raise ValueError("复制锁来源、目标或计划与原会话不匹配")
    _bound(owner.backend, value["source_export"])
    initialized = _bound(owner.backend, value["initialized"])
    if initialized.name != "initialized.json":
        raise ValueError("复制锁目标未绑定初始化收据")
    identity = value["controller"]
    exact(identity, {"pid", "started", "executable"})
    if (type(identity["pid"]) is not int or identity["pid"] <= 1 or not isinstance(identity["started"], str)
            or not identity["started"].isdigit() or not Path(identity["executable"]).is_absolute()):
        raise ValueError("复制控制器创建身份无效")
    return value, {"target": initialized.parent / "initialize.lock", "ledger": root / "ledger/clone.lock"}


def _lock_record(lock: Path, role: str, owner: CopyOwner) -> dict:
    value, paths = read_copy_owner(owner)
    if role not in paths or lock != paths[role]:
        raise ValueError("复制锁路径不属于登记的明确源目标会话")
    stat = lock.stat()
    return {"format_version": 1, "kind": "devex-owned-copy-lock", "role": role,
            "owner_receipt": owner.binding, "controller": value["controller"],
            "path": str(lock), "device": stat.st_dev, "inode": stat.st_ino}


def record_copy_lock(lock: Path, role: str, owner: CopyOwner) -> None:
    record = _lock_record(lock, role, owner)
    if process_identity(os.getpid()) != record["controller"] or any(lock.iterdir()):
        raise ValueError("只有登记的活控制器可登记刚取得的空复制锁")
    write_receipt(lock / "owner.json", record)


def release_copy_lock(lock: Path, role: str, owner: CopyOwner) -> None:
    expected = _lock_record(lock, role, owner)
    if {path.name for path in lock.iterdir()} != {"owner.json"} or _read(lock / "owner.json") != expected:
        raise ValueError("复制锁创建身份或目录内容变化，保留现场")
    (lock / "owner.json").unlink()
    try:
        lock.rmdir()
    except BaseException:
        # rmdir 失败时保留同 inode 的持有人证据，避免把已知锁退化为裸锁。
        if (not linked(lock) and lock.is_dir() and lock.stat().st_dev == expected["device"]
                and lock.stat().st_ino == expected["inode"] and not any(lock.iterdir())):
            write_receipt(lock / "owner.json", expected)
        raise


def _commands_complete(root: Path, session_id: str) -> None:
    for path in root.glob(f"command-{name(session_id)}-*.json"):
        if linked(path):
            raise ValueError("复制命令记录不能经过链接")
        value = _read(path)
        if type(value.get("returncode")) is not int:
            raise ValueError("复制命令未登记退出结果，不能排除遗留子进程；必须先核实现场")


def recover_copy_locks(backend: Path, copy_directory: Path, owner_receipt: dict, *,
                       source_export: dict, initialized: dict) -> dict:
    """只回收本次新登记的死控制器锁；裸锁、未知命令和远端未知写入都不自动处理。"""
    backend = backend.resolve(strict=True)
    root = local_path(backend, str(copy_directory))
    owner_data = _read(_bound(backend, owner_receipt))
    owner = CopyOwner(backend, owner_receipt, owner_data["session_id"])
    value, paths = read_copy_owner(owner)
    if (value["copy_directory"] != str(root) or value["source_export"] != source_export
            or value["initialized"] != initialized):
        raise ValueError("复制锁恢复必须绑定调用方固定的来源、目标与复制目录")
    with ExitStack() as guards:
        for role, lock in paths.items():
            if lock.parent.is_dir():
                guards.enter_context(process_guard(lock.parent, GUARDS[role]))
        actual = process_identity(value["controller"]["pid"])
        if actual == value["controller"]:
            raise ValueError("登记的复制控制器仍在运行，禁止回收锁")
        _commands_complete(root, owner.session_id)
        found = []
        for role, lock in paths.items():
            if lock.exists():
                expected = _lock_record(lock, role, owner)
                if ({path.name for path in lock.iterdir()} != {"owner.json"}
                        or _read(lock / "owner.json") != expected):
                    raise ValueError("复制锁没有匹配的登记持有人；裸锁或其他会话锁必须核实现场")
                found.append((role, lock, expected))
        result = {"format_version": 1, "kind": "devex-copy-lock-recovery", "owner_receipt": owner_receipt,
                  "observed_controller": actual, "locks": [record for _, _, record in found],
                  "remote_writes": 0, "remote_writes_reconciled": False, "target_ready": False,
                  "descendant_scope": "仅检查已登记直接命令的退出记录；不证明未登记后代已退出",
                  "requires_reconcile": True, "status": "prepared"}
        receipt = root / ("copy-lock-recovery-" + uuid.uuid4().hex + ".json")
        write_receipt(receipt, result)
        for role, lock, _ in found:
            release_copy_lock(lock, role, owner)
        result["status"] = "owned_locks_released"
        write_receipt(receipt, result)
        return {**result, "receipt": _binding(receipt)}
