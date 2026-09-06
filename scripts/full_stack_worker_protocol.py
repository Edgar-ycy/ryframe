"""保存并核验 external Worker 控制操作的本机原子收据。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import time
import uuid
from pathlib import Path

from artifact_digests import file_digest
from full_stack_process import process_identity, write_receipt

LOCK = "worker-control.lock"
ARCHIVE = "worker-control-receipts"
OPERATIONS = frozenset({"start", "stop", "crash", "status", "reconcile"})
OPERATION_ID = re.compile(r"^[a-f0-9]{32}$")
SHA256 = re.compile(r"^[a-f0-9]{64}$")
COMMON_FIELDS = {
    "format_version",
    "kind",
    "operation_id",
    "runtime_directory",
    "source_sha256",
}


def _unique_object(pairs: list[tuple[str, object]]) -> dict:
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("Worker 控制收据包含重复字段")
        value[key] = item
    return value


def _positive_integer(value: object) -> bool:
    return type(value) is int and value > 0


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


def _validate_common(value: dict, kind: str, directory: Path, source: str) -> None:
    if (
        value.get("format_version") != 1
        or value.get("kind") != kind
        or not isinstance(value.get("operation_id"), str)
        or OPERATION_ID.fullmatch(value["operation_id"]) is None
        or value.get("runtime_directory") != str(directory)
        or SHA256.fullmatch(source) is None
        or value.get("source_sha256") != source
    ):
        raise ValueError("Worker 控制收据与运行目录、操作或来源不匹配")


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
    _validate_common(owner, "full-stack-worker-control-owner", directory, source)
    _validate_common(request, "full-stack-worker-control-request", directory, source)
    operation_id = owner["operation_id"]
    identity = _valid_identity(owner.get("identity"), "Worker 控制器")
    if (
        set(owner) != COMMON_FIELDS | {"identity", "created_at_ns"}
        or not _positive_integer(owner.get("created_at_ns"))
        or set(request)
        != COMMON_FIELDS
        | {
            "operation",
            "backend_root",
            "scope_id",
            "controller_identity",
            "owner_sha256",
            "runtime_receipt_sha256",
            "worker_artifact",
            "worker_ready_url",
            "timeout_seconds",
            "log",
            "requested_at_ns",
        }
        or request.get("operation_id") != operation_id
        or request.get("owner_sha256") != _digest(owner)
        or request.get("controller_identity") != identity
        or request.get("operation") not in OPERATIONS
        or request.get("log")
        != (
            f"worker-{operation_id}.log"
            if request.get("operation") == "start"
            else None
        )
        or not _positive_integer(request.get("requested_at_ns"))
        or request.get("runtime_receipt_sha256")
        != file_digest(directory / "runtime.json")["sha256"]
    ):
        raise ValueError("Worker 控制请求没有绑定原子 owner 或当前 runtime 收据")
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
    request = operation["request"]
    timeout = request.get("timeout_seconds")
    if (
        request.get("backend_root") != str(backend)
        or request.get("scope_id") != receipt["scope_id"]
        or request.get("worker_artifact") != receipt["artifacts"]["worker"]
        or request.get("worker_ready_url") != receipt["worker_ready_url"]
        or not isinstance(request.get("scope_id"), str)
        or not request["scope_id"]
        or isinstance(timeout, bool)
        or not isinstance(timeout, (int, float))
        or not 0 < timeout <= 180
    ):
        raise ValueError("Worker 控制请求与当前后端、scope、产物或端点不匹配")


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
    value = _read(operation["path"] / name)
    owner, request = operation["owner"], operation["request"]
    _validate_common(value, kind, operation["path"].parent, owner["source_sha256"])
    identity = _valid_identity(value.get("identity"), "Worker 启动监督进程")
    extras = {
        "candidate.json": {"created_at_ns"},
        "supervisor.json": {"candidate_sha256", "authorized_at_ns"},
    }.get(name)
    if (
        extras is None
        or set(value)
        != COMMON_FIELDS | {"identity", "owner_sha256", "request_sha256"} | extras
        or value.get("operation_id") != owner["operation_id"]
        or value.get("owner_sha256") != _digest(owner)
        or value.get("request_sha256") != _digest(request)
        or not _positive_integer(
            value.get(
                "created_at_ns" if name == "candidate.json" else "authorized_at_ns"
            )
        )
    ):
        raise ValueError("Worker 启动监督收据没有绑定当前控制操作")
    if name == "supervisor.json":
        candidate = _validate_child(
            operation, "candidate.json", "full-stack-worker-control-candidate"
        )
        if value.get("candidate_sha256") != _digest(candidate):
            raise ValueError("Worker 启动监督授权没有绑定当前候选进程")
    return {**value, "identity": identity}


def _validate_progress(operation: dict) -> dict:
    value = _read(operation["path"] / "progress.json")
    owner, request = operation["owner"], operation["request"]
    _validate_common(
        value,
        "full-stack-worker-control-progress",
        operation["path"].parent,
        owner["source_sha256"],
    )
    phase = value.get("phase")
    identity = value.get("identity")
    supervisor = _validate_child(
        operation, "supervisor.json", "full-stack-worker-control-supervisor"
    )
    if (
        set(value)
        != COMMON_FIELDS
        | {
            "phase",
            "identity",
            "owner_sha256",
            "request_sha256",
            "supervisor_sha256",
            "updated_at_ns",
        }
        or phase not in {"launching", "started", "ready"}
        or (phase == "launching" and identity is not None)
        or (phase != "launching" and not isinstance(identity, dict))
        or value.get("owner_sha256") != _digest(owner)
        or value.get("request_sha256") != _digest(request)
        or value.get("supervisor_sha256") != _digest(supervisor)
        or not _positive_integer(value.get("updated_at_ns"))
    ):
        raise ValueError("Worker 启动进度没有绑定当前控制操作")
    if identity is not None:
        identity = _valid_identity(identity, "Worker 启动进度")
    return {**value, "identity": identity}


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
    value = _read(operation["path"] / "result.json")
    owner, request = operation["owner"], operation["request"]
    _validate_common(
        value,
        "full-stack-worker-control-result",
        operation["path"].parent,
        owner["source_sha256"],
    )
    identity = value.get("identity")
    outcome, state = value.get("outcome"), value.get("state")
    error_type, error = value.get("error_type"), value.get("error")
    if (
        set(value)
        != COMMON_FIELDS
        | {
            "operation",
            "scope_id",
            "controller_identity",
            "owner_sha256",
            "request_sha256",
            "outcome",
            "state",
            "identity",
            "log",
            "reconciled",
            "error_type",
            "error",
            "completed_at_ns",
        }
        or value.get("operation_id") != owner["operation_id"]
        or value.get("operation") != request["operation"]
        or value.get("scope_id") != request["scope_id"]
        or value.get("controller_identity") != owner["identity"]
        or value.get("owner_sha256") != _digest(owner)
        or value.get("request_sha256") != _digest(request)
        or value.get("log") != request["log"]
        or outcome not in {"succeeded", "failed"}
        or state not in {"running", "stopped", "unknown"}
        or type(value.get("reconciled")) is not bool
        or not _positive_integer(value.get("completed_at_ns"))
    ):
        raise ValueError("Worker 控制结果与当前请求不匹配")
    if identity is not None:
        identity = _valid_identity(identity, "Worker 控制结果")
    if outcome == "succeeded":
        if error_type is not None or error is not None or state == "unknown":
            raise ValueError("成功的 Worker 控制结果包含错误或未知状态")
        expected = {
            "start": {"running"},
            "stop": {"stopped"},
            "crash": {"stopped"},
            "status": {"running", "stopped"},
            "reconcile": {"running", "stopped"},
        }[request["operation"]]
        if state not in expected:
            raise ValueError("Worker 控制结果没有达到请求的目标状态")
    elif (
        state not in {"stopped", "unknown"}
        or not isinstance(error_type, str)
        or not error_type
        or not isinstance(error, str)
        or not error
    ):
        raise ValueError("失败的 Worker 控制结果缺少错误信息或包含错误状态")
    if (state == "running" and identity is None) or (
        state == "stopped" and identity is not None
    ):
        raise ValueError("Worker 控制结果状态与进程身份不一致")
    return {**value, "identity": identity}


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
