"""统一控制已登记的复制验收 API/Worker，保留每次生产者启动与失败证据。"""

from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import uuid
from contextlib import contextmanager
from pathlib import Path

from devex_clone_source_proof import require_closed_port, verify_api_address
from full_stack_process import (
    assert_identity, process_identity, read_process, record_process, terminate_owned_process, write_receipt,
)
from full_stack_runtime import verify_runtime
from process_sockets import endpoint, verify_listener
from runtime_control_lock import ControllerLockSpec, controller_lock as _controller_lock
from runtime_control_lock import reconcile_lock as _reconcile_controller_lock

LOCK = "worker-control.lock"
HISTORY = "producer-history.json"
LOCK_SPEC = ControllerLockSpec(
    lock_name=LOCK,
    guard_name="runtime-control.guard",
    owner_kind="devex-clone-runtime-lock",
    recovery_kind="devex-clone-runtime-lock-reconciliation",
    recovery_prefix="controller-reconcile",
)


def _read(path: Path) -> dict:
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("运行控制收据超过大小上限")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("运行控制收据必须是对象")
    return value


def _directory(path: Path) -> Path:
    if not path.is_absolute() or any(
            part.is_symlink() or (hasattr(part, "is_junction") and part.is_junction())
            for part in (path, *path.parents)):
        raise ValueError("运行目录必须是无链接的明确绝对路径")
    return path.resolve(strict=True)


@contextmanager
def controller_lock(directory: Path, operation: str):
    with _controller_lock(directory, operation, LOCK_SPEC):
        yield


def reconcile_lock(runtime_directory: Path) -> dict:
    """显式核验已退出的控制器，仅释放其精确锁；不停止进程或推定远端写入成功。"""
    result = _reconcile_controller_lock(_directory(runtime_directory), LOCK_SPEC)
    return {key: value for key, value in result.items() if key != "receipt"}


def _identity(directory: Path, receipt: dict, role: str) -> dict | None:
    if not (directory / f"{role}.json").exists():
        return None
    identity = read_process(directory, role, receipt["scope_id"])
    if Path(identity.get("executable", "")) != Path(receipt["artifacts"][role]["path"]):
        raise ValueError("进程收据与已登记的实际二进制不匹配")
    return identity if assert_identity(process_identity(identity.get("pid")), identity) else None


def _ready(url: str) -> bool:
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *_args, **_kwargs):
            return None

    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        with opener.open(url, timeout=1) as response:
            return response.status == 200
    except (OSError, urllib.error.URLError):
        return False


def _event(directory: Path, receipt: dict, event: dict) -> None:
    path = directory / HISTORY
    history = _read(path) if path.exists() else {
        "format_version": 1, "kind": "devex-clone-producer-history", "scope_id": receipt["scope_id"],
        "runtime_directory": str(directory), "events": [],
    }
    if (history.get("format_version") != 1 or history.get("kind") != "devex-clone-producer-history"
            or history.get("scope_id") != receipt["scope_id"]
            or history.get("runtime_directory") != str(directory) or not isinstance(history.get("events"), list)):
        raise ValueError("生产者历史与运行目录不一致，拒绝覆盖")
    history["events"].append({"sequence": len(history["events"]), "time_ns": time.time_ns(), **event})
    write_receipt(path, history)


def _start_one(backend: Path, directory: Path, receipt: dict, role: str, url: str, timeout: float) -> dict:
    token = uuid.uuid4().hex
    executable = receipt["artifacts"][role]["path"]
    log_path = directory / f"{role}-{token}.log"
    # 意图先于 Popen 持久化。即使启动失败，该数据目标也不能重新认定为从未启动。
    _event(directory, receipt, {"event": "start-intent", "role": role, "token": token,
                               "artifact": receipt["artifacts"][role], "ready_url": url,
                               "configuration_sha256": receipt["configuration_sha256"], "log": log_path.name})
    process, identity = None, None
    try:
        with log_path.open("xb") as log:
            process = subprocess.Popen(
                [executable], cwd=backend, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                env={**os.environ, "SNOWFLAKE_WORKER_ID": "1" if role == "api" else "2"},
                start_new_session=True, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
        identity = record_process(directory, role, process.pid, executable, receipt["scope_id"])["identity"]
        _event(directory, receipt, {"event": "started", "role": role, "token": token, "identity": identity})
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"{role} 就绪前退出；日志：{log_path.name}")
            if _ready(url):
                if not assert_identity(process_identity(process.pid), identity):
                    raise RuntimeError(f"{role} 就绪校验时退出；日志：{log_path.name}")
                verify_listener(process.pid, url)
                return {"state": "running", "identity": identity, "ready": True, "log": log_path.name}
            time.sleep(0.1)
        raise TimeoutError(f"{role} 就绪超时；日志：{log_path.name}")
    except BaseException as error:
        if process is not None:
            if identity is not None:
                terminate_owned_process(identity)
            elif process.poll() is None:
                # 启动调用保存的 Popen 内核句柄仅用于尚未写出收据的自身子进程。
                process.kill()
            process.wait(timeout=5)
        _event(directory, receipt, {"event": "start-failed", "role": role, "token": token,
                                   "error_type": type(error).__name__, "log": log_path.name})
        raise


def _start(backend: Path, directory: Path, receipt: dict, roles: tuple[str, ...], urls: dict, timeout: float) -> dict:
    for role in roles:
        if _identity(directory, receipt, role) is not None:
            raise ValueError("已登记进程仍在运行，拒绝重复启动")
        require_closed_port(urls[role])
    started = {}
    try:
        for role in roles:
            started[role] = _start_one(backend, directory, receipt, role, urls[role], timeout)
        return started
    except BaseException:
        for role, result in reversed(list(started.items())):
            terminate_owned_process(result["identity"])
            _event(directory, receipt, {"event": "startup-rollback", "role": role, "identity": result["identity"]})
        raise


def _observe(directory: Path, receipt: dict, roles: tuple[str, ...], urls: dict, stop: bool) -> dict:
    # 先核验全部身份；发现任何异常时不会先停止另一个角色。
    identities = {role: _identity(directory, receipt, role) for role in roles}
    results = {}
    for role in reversed(roles) if stop else roles:
        identity = identities[role]
        if stop and identity is not None:
            terminate_owned_process(identity)
            _event(directory, receipt, {"event": "stopped", "role": role, "identity": identity})
            identity = None
        if identity is None:
            require_closed_port(urls[role])
        else:
            verify_listener(identity["pid"], urls[role])
        results[role] = {"state": "running" if identity else "stopped", "identity": identity,
                         "ready": _ready(urls[role]) if identity else False}
    return results


def control(backend: Path, runtime_directory: Path, operation: str, roles: tuple[str, ...],
            api_url: str, timeout: float = 60) -> dict:
    """使用调用方注入的测试环境；不读取全源码指纹，不改写源停止或新库初始化证明。"""
    if operation not in {"start", "stop", "status"}:
        raise ValueError("未知运行控制操作")
    if not roles or len(set(roles)) != len(roles) or not set(roles) <= {"api", "worker"}:
        raise ValueError("运行控制角色必须是明确且不重复的 API/Worker")
    if isinstance(timeout, bool) or not 0 < timeout <= 180:
        raise ValueError("运行就绪超时必须在 0 到 180 秒之间")
    backend, directory = _directory(backend), _directory(runtime_directory)
    endpoint(api_url)
    with controller_lock(directory, operation):
        receipt = verify_runtime(backend, directory)
        verify_api_address(backend, api_url)
        urls = {"api": api_url.rstrip("/") + "/readyz", "worker": receipt["worker_ready_url"]}
        if endpoint(urls["api"]) == endpoint(urls["worker"]):
            raise ValueError("API 和 Worker 必须使用不同的明确监听端口")
        results = (_start(backend, directory, receipt, roles, urls, timeout) if operation == "start"
                   else _observe(directory, receipt, roles, urls, operation == "stop"))
        return {"format_version": 1, "kind": "devex-clone-runtime-control", "operation": operation,
                "scope_id": receipt["scope_id"], "runtime_directory": str(directory), "processes": results,
                "producer_history": str(directory / HISTORY) if (directory / HISTORY).exists() else None}
