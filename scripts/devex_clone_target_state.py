"""单侧新目标的一次性本地阶段记录；失败不重放资源写入，不执行远端清理。"""
from __future__ import annotations

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


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


@contextmanager
def generation_lock(output: Path, *, copy_owner: CopyOwner | None = None):
    lock = output / "initialize.lock"
    if lock.exists():
        raise FileExistsError("初始化锁已存在，不能并发执行或自动抢锁")
    with process_guard(output, GUARDS["target"]):
        lock.mkdir()
        identity = lock.stat().st_ino
        if copy_owner is not None:
            record_copy_lock(lock, "target", copy_owner)
        try:
            yield identity
        finally:
            if linked(lock) or not lock.is_dir() or lock.stat().st_ino != identity:
                raise ValueError("初始化锁身份变化，保留现场，拒绝清理其他锁")
            if copy_owner is not None:
                release_copy_lock(lock, "target", copy_owner)
            else:
                lock.rmdir()


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
