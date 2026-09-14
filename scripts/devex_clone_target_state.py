"""单侧新目标的一次性本地阶段记录；失败不重放资源写入，不执行远端清理。"""
from __future__ import annotations

from collections.abc import Callable
from contextlib import contextmanager
import datetime as dt
import os
from pathlib import Path
import uuid

from devex_clone_capture import write_json
from devex_clone_copy_locks import CopyOwner, GUARDS, record_copy_lock, release_copy_lock
from devex_clone_model import linked
from process_guard import process_guard
from restore_reference_io import redact_object_diagnostic

_ACTIVE_GENERATIONS = {}


def generation_guard_checkpoint(output: Path, lock_identity: int | None = None) -> int:
    """复核已锁 guard 路径仍指向本次打开的内核对象。"""
    root = output.resolve(strict=True)
    matches = [(key, lease) for key, lease in _ACTIVE_GENERATIONS.items()
               if key[0] == root]
    if len(matches) != 1:
        raise ValueError("当前进程没有唯一的 fresh 初始化锁租约")
    (key, identity), lease = matches[0]
    if lock_identity is not None and lock_identity != identity:
        raise ValueError("fresh 初始化锁租约身份不符")
    lease.check()
    return identity


def generation_checkpoint(output: Path, lock_identity: int | None = None) -> int:
    """复核 initialize 目录与已锁 guard 路径仍属于当前代次。"""
    root = output.resolve(strict=True)
    identity = generation_guard_checkpoint(root, lock_identity)
    lock = root / "initialize.lock"
    if linked(lock) or not lock.is_dir() or lock.stat().st_ino != identity:
        raise ValueError("初始化锁身份变化，拒绝继续访问资源")
    return identity


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


@contextmanager
def generation_lock(output: Path, *, copy_owner: CopyOwner | None = None,
                    after_initialize_unlock: list[Callable[[], None]] | None = None):
    """持有目标 guard；成功时先释放 initialize.lock，再在 guard 内执行唯一收尾。"""
    if after_initialize_unlock is not None and after_initialize_unlock:
        raise ValueError("初始化锁收尾列表必须从空列表开始")
    lock = output / "initialize.lock"
    if lock.exists():
        raise FileExistsError("初始化锁已存在，不能并发执行或自动抢锁")
    with process_guard(output, GUARDS["target"]) as guard:
        lock.mkdir()
        identity = lock.stat().st_ino
        if copy_owner is not None:
            record_copy_lock(lock, "target", copy_owner)
        key = (output.resolve(strict=True), identity)
        if key in _ACTIVE_GENERATIONS:
            raise ValueError("当前进程已有相同 fresh 初始化锁租约")
        _ACTIVE_GENERATIONS[key] = guard
        completed = False
        try:
            yield identity
            completed = True
        finally:
            try:
                generation_checkpoint(output, identity)
                if copy_owner is not None:
                    release_copy_lock(lock, "target", copy_owner)
                else:
                    lock.rmdir()
                if completed and after_initialize_unlock is not None:
                    if len(after_initialize_unlock) != 1:
                        raise ValueError("初始化成功必须登记唯一目标 guard 内收尾")
                    generation_guard_checkpoint(output, identity)
                    after_initialize_unlock[0]()
                    generation_guard_checkpoint(output, identity)
            finally:
                _ACTIVE_GENERATIONS.pop(key, None)


def failure(output: Path, stage: str, error: BaseException) -> None:
    path = output / "failure.json"
    if path.exists():
        path = output / ("failure-" + uuid.uuid4().hex + ".json")
    write_json(path, {
        "status": "needs_reconciliation", "stage": stage, "at": now(),
        "error_type": type(error).__name__,
        "reason": redact_object_diagnostic(str(error), os.environ) if isinstance(error, ValueError) else "阶段未完成，核对同目录诊断",
        "automatic_retry": False, "automatic_resource_cleanup": False,
        "fresh_target_initialized": False, "clone_verified": False, "restore_qualified": False,
    })


def intent(output: Path, stage: str, resource: dict) -> None:
    write_json(output / f"{stage}.intent.json", {"at": now(), "stage": stage, "resource": resource})


def confirmed(output: Path, stage: str, observed: dict) -> None:
    write_json(output / f"{stage}.confirmed.json", {"at": now(), "stage": stage, "observed": observed})
