"""保存并核验 external Worker 控制操作的本机原子收据。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import time
import uuid
from pathlib import Path

from full_stack_worker_native import validate as validate_native
from artifact_digests import file_digest
from full_stack_process import process_identity, write_receipt

LOCK = "worker-control.lock"
ARCHIVE = "worker-control-receipts"
OPERATIONS = frozenset({"start", "stop", "crash", "status", "reconcile"})
OPERATION_ID = re.compile(r"^[a-f0-9]{32}$")


def _unique_object(pairs: list[tuple[str, object]]) -> dict:
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("Worker 控制收据包含重复字段")
        value[key] = item
    return value


def _digest(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()


def _read(path: Path) -> dict:
    if (
        not path.is_file()
        or path.is_symlink()
        or not 0 < path.stat().st_size <= 64 * 1024
    ):
        raise ValueError("Worker 控制收据不存在、不是普通文件或大小无效")
    try:
        value = json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=_unique_object
        )
    except (json.JSONDecodeError, UnicodeError) as error:
        raise ValueError("Worker 控制收据不是有效 JSON") from error
    if not isinstance(value, dict):
        raise ValueError("Worker 控制收据必须是对象")
    return value


def _valid_identity(value: object, label: str) -> dict:
    if (
        not isinstance(value, dict)
        or set(value) != {"pid", "started", "executable"}
        or type(value["pid"]) is not int
        or value["pid"] <= 1
        or not isinstance(value["started"], str)
        or not value["started"].isdigit()
        or not isinstance(value["executable"], str)
        or not Path(value["executable"]).is_absolute()
    ):
        raise ValueError(f"{label}缺少有效的 PID 创建身份")
    return value


def _file_identity(path: Path) -> tuple[int, int]:
    stat = path.stat()
    return stat.st_dev, stat.st_ino


def _source_sha256(directory: Path) -> str:
    runtime = directory / "runtime.json"
    evidence = directory / "runtime-evidence.json"
    return _digest(
        {
            "runtime": file_digest(runtime),
            "runtime_evidence": file_digest(evidence) if evidence.is_file() else None,
        }
    )


def _common(kind: str, operation_id: str, directory: Path, source: str) -> dict:
    return {
        "format_version": 1,
        "kind": kind,
        "operation_id": operation_id,
        "runtime_directory": str(directory),
        "source_sha256": source,
    }


def _archive_directory(directory: Path) -> Path:
    archive = directory / ARCHIVE
    archive.mkdir(exist_ok=True)
    if not archive.is_dir() or archive.is_symlink():
        raise ValueError("Worker 控制历史目录无效")
    return archive


def _operation(directory: Path, source: str) -> dict:
    lock = directory / LOCK
    if not lock.is_dir() or lock.is_symlink():
        raise ValueError("Worker 控制锁不是可信目录")
    names = {path.name for path in lock.iterdir()}
    allowed = {
        "owner.json",
        "request.json",
        "candidate.json",
        "supervisor.json",
        "progress.json",
        "result.json",
    }
    if not {"owner.json", "request.json"} <= names or not names <= allowed:
        raise ValueError("Worker 控制锁缺少收据或包含未知文件")
    owner, request = _read(lock / "owner.json"), _read(lock / "request.json")
    validate_native("operation", {"path": lock, "owner": owner, "request": request}, source=source)
    return {
        "path": lock,
        "inode": _file_identity(lock),
        "owner": owner,
        "request": request,
    }


def _claim(
    operation: str,
    backend: Path,
    directory: Path,
    receipt: dict,
    source: str,
    timeout: float,
) -> dict:
    lock = directory / LOCK
    if lock.exists():
        raise ValueError("Worker 控制正在执行或等待核对")
    operation_id = uuid.uuid4().hex
    identity = process_identity(os.getpid())
    if identity is None:
        raise ValueError("无法取得 Worker 控制器的进程创建身份")
    identity = _valid_identity(identity, "Worker 控制器")
    owner = {
        **_common("full-stack-worker-control-owner", operation_id, directory, source),
        "identity": identity,
        "created_at_ns": time.time_ns(),
    }
    request = {
        **_common("full-stack-worker-control-request", operation_id, directory, source),
        "operation": operation,
        "backend_root": str(backend),
        "scope_id": receipt["scope_id"],
        "controller_identity": identity,
        "owner_sha256": _digest(owner),
        "runtime_receipt_sha256": file_digest(directory / "runtime.json")["sha256"],
        "worker_artifact": receipt["artifacts"]["worker"],
        "worker_ready_url": receipt["worker_ready_url"],
        "timeout_seconds": timeout,
        "log": f"worker-{operation_id}.log" if operation == "start" else None,
        "requested_at_ns": time.time_ns(),
    }
    temporary = directory / f".{LOCK}.{operation_id}.tmp"
    temporary.mkdir()
    try:
        write_receipt(temporary / "owner.json", owner)
        write_receipt(temporary / "request.json", request)
        temporary.rename(lock)
    except BaseException:
        for name in ("request.json", "owner.json"):
            (temporary / name).unlink(missing_ok=True)
        if temporary.exists():
            temporary.rmdir()
        raise
    return _operation(directory, source)


def _write_operation_receipt(
    operation: dict, name: str, receipt: dict, *, replace: bool = False
) -> None:
    if name not in {
        "candidate.json",
        "supervisor.json",
        "progress.json",
        "result.json",
    }:
        raise ValueError("未知 Worker 控制收据名称")
    target = operation["path"] / name
    if target.exists() and not replace:
        raise ValueError("Worker 控制收据已经存在")
    code = name[0]
    staged = operation["path"].parent / (
        f".wc-{operation['owner']['operation_id'][:8]}-{code}-"
        f"{uuid.uuid4().hex[:12]}.tmp"
    )
    try:
        write_receipt(staged, receipt)
        if (
            not operation["path"].is_dir()
            or operation["path"].is_symlink()
            or _file_identity(operation["path"]) != operation["inode"]
            or (target.exists() and not replace)
        ):
            raise ValueError("Worker 控制锁在发布收据前发生变化")
        staged.replace(target)
    finally:
        staged.unlink(missing_ok=True)


def _validate_request(operation: dict, backend: Path, receipt: dict) -> None:
    validate_native("request", operation, backend=str(backend), runtime=receipt)


def _child_receipt(operation: dict, kind: str, identity: dict, **extra: object) -> dict:
    owner, request = operation["owner"], operation["request"]
    return {
        **_common(
            kind,
            owner["operation_id"],
            operation["path"].parent,
            owner["source_sha256"],
        ),
        "identity": _valid_identity(identity, "Worker 启动监督进程"),
        "owner_sha256": _digest(owner),
        "request_sha256": _digest(request),
        **extra,
    }


def _validate_child(operation: dict, name: str, kind: str) -> dict:
    expected = {"candidate.json": "full-stack-worker-control-candidate",
                "supervisor.json": "full-stack-worker-control-supervisor"}
    if name not in expected or kind != expected[name]:
        raise ValueError("未知 Worker 启动监督收据")
    candidate = _read(operation["path"] / "candidate.json") if name == "supervisor.json" else None
    return validate_native("candidate" if name == "candidate.json" else "supervisor", operation,
                           value=_read(operation["path"] / name), candidate=candidate)


def _validate_progress(operation: dict) -> dict:
    return validate_native("progress", operation, value=_read(operation["path"] / "progress.json"),
                           candidate=_read(operation["path"] / "candidate.json"),
                           supervisor=_read(operation["path"] / "supervisor.json"))


def _result(
    operation: dict,
    outcome: str,
    state: str,
    identity: dict | None,
    **details: object,
) -> dict:
    owner, request = operation["owner"], operation["request"]
    return {
        **_common(
            "full-stack-worker-control-result",
            owner["operation_id"],
            operation["path"].parent,
            owner["source_sha256"],
        ),
        "operation": request["operation"],
        "scope_id": request["scope_id"],
        "controller_identity": owner["identity"],
        "owner_sha256": _digest(owner),
        "request_sha256": _digest(request),
        "outcome": outcome,
        "state": state,
        "identity": identity,
        "log": request["log"],
        "reconciled": bool(details.get("reconciled", False)),
        "error_type": details.get("error_type"),
        "error": details.get("error"),
        "completed_at_ns": time.time_ns(),
    }


def _validate_result(operation: dict) -> dict:
    return validate_native("result", operation, value=_read(operation["path"] / "result.json"))


def _archive(operation: dict) -> dict:
    source = operation["owner"]["source_sha256"]
    current = _operation(operation["path"].parent, source)
    if current["inode"] != operation["inode"] or current["owner"] != operation["owner"]:
        raise ValueError("Worker 控制锁在归档前发生变化")
    result = _validate_result(current)
    request = current["request"]
    optional = {
        name
        for name in ("candidate.json", "supervisor.json", "progress.json")
        if (current["path"] / name).is_file()
    }
    if request["operation"] != "start" and optional:
        raise ValueError("非 start 操作包含 Worker 启动监督收据")
    if "candidate.json" in optional:
        _validate_child(
            current, "candidate.json", "full-stack-worker-control-candidate"
        )
    if "supervisor.json" in optional:
        _validate_child(
            current, "supervisor.json", "full-stack-worker-control-supervisor"
        )
    progress = _validate_progress(current) if "progress.json" in optional else None
    if result["outcome"] == "succeeded" and request["operation"] == "start":
        if (
            progress is None
            or progress["phase"] != "ready"
            or progress["identity"] != result["identity"]
        ):
            raise ValueError("成功的 Worker start 缺少同一进程的就绪进度")
    target = (
        _archive_directory(operation["path"].parent)
        / operation["owner"]["operation_id"]
    )
    if target.exists():
        raise ValueError("Worker 控制操作 ID 已经归档")
    current["path"].rename(target)
    return result
