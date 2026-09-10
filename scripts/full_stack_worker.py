"""控制已经登记的隔离 external Worker；不暴露产品测试接口。"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from collections.abc import Callable
from pathlib import Path

from full_stack_process import (
    assert_identity,
    process_identity,
    read_process,
    record_process,
    terminate_owned_process,
    write_receipt,
)
from full_stack_process_tree import (
    enter_supervision,
    finish_supervision,
    read_process_tree,
    record_process_tree,
    supervise_product,
    terminate_owned_process_tree,
)
from full_stack_runtime import verify_runtime
from full_stack_worker_protocol import (
    ARCHIVE as ARCHIVE,
    LOCK,
    OPERATIONS,
    OPERATION_ID,
    _archive,
    _archive_directory,
    _child_receipt,
    _claim,
    _common,
    _digest,
    _file_identity,
    _operation,
    _result,
    _source_sha256,
    _valid_identity,
    _validate_child,
    _validate_progress,
    _validate_request,
    _write_operation_receipt,
)
from process_guard import process_guard
from process_sockets import verify_listener


class _PendingStart(RuntimeError):
    pass


def _control_checkpoint(_phase: str) -> None:
    """为进程级故障测试保留的纯 Python 注入点；生产执行为空操作。"""


def _guarded_update(directory: Path, update: Callable[[], None]) -> None:
    deadline = time.monotonic() + 10
    while True:
        try:
            with process_guard(directory, "worker-control.guard"):
                update()
            return
        except ValueError as error:
            if (
                str(error) != "控制或锁恢复正在执行，拒绝并发操作"
                or time.monotonic() >= deadline
            ):
                raise
            time.sleep(0.02)


def worker_identity(directory: Path, receipt: dict) -> dict | None:
    if not (directory / "worker.json").is_file():
        return None
    identity = read_process(directory, "worker", receipt["scope_id"])
    if Path(identity.get("executable", "")) != Path(
        receipt["artifacts"]["worker"]["path"]
    ):
        raise ValueError("Worker 进程收据与构建产物不匹配")
    return (
        identity
        if assert_identity(process_identity(identity.get("pid")), identity)
        else None
    )


def _worker_tree(directory: Path, receipt: dict, *, required: bool) -> dict | None:
    path = directory / "worker-tree.json"
    if not path.is_file():
        if required:
            raise ValueError("已登记 Worker 缺少进程树收据")
        return None
    tree = read_process_tree(directory, "worker", receipt["scope_id"])
    if not (directory / "worker.json").is_file():
        if required:
            raise ValueError("Worker 进程树缺少对应的产品进程收据")
        return tree
    if tree["process"] != read_process(directory, "worker", receipt["scope_id"]):
        raise ValueError("Worker 进程与进程树收据不一致")
    return tree


def _verify_worker_tree_running(directory: Path, receipt: dict, identity: dict) -> dict:
    tree = _worker_tree(directory, receipt, required=True)
    if tree["process"] != identity or not assert_identity(
        process_identity(tree["supervisor"]["pid"]), tree["supervisor"]
    ):
        raise ValueError("Worker 进程树监督器未保持运行")
    return tree


def ready(url: str) -> bool:
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *_args, **_kwargs):
            return None

    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        with opener.open(url, timeout=1) as response:
            return response.status == 200
    except (OSError, urllib.error.URLError):
        return False


def verify_running(identity: dict, url: str) -> None:
    if not ready(url):
        raise ValueError("已登记 Worker 未通过就绪探针，不能声明正在运行")
    verify_listener(identity["pid"], url)
    if not assert_identity(process_identity(identity["pid"]), identity):
        raise ValueError("Worker 在运行态核验后退出，不能声明正在运行")


def ensure_port_free(url: str) -> None:
    parsed = urllib.parse.urlsplit(url)
    with socket.socket() as probe:
        probe.settimeout(1)
        if probe.connect_ex((parsed.hostname, parsed.port)) == 0:
            raise ValueError("全栈就绪端口已占用，拒绝把其他进程当作新服务")


def wait_for_port_free(url: str, timeout: float = 5) -> None:
    deadline = time.monotonic() + timeout
    while True:
        try:
            ensure_port_free(url)
            return
        except ValueError:
            if time.monotonic() >= deadline:
                raise TimeoutError("全栈进程退出后就绪端口仍被占用") from None
            time.sleep(0.05)


def _progress(
    operation_id: str,
    backend: Path,
    directory: Path,
    phase: str,
    identity: dict | None,
) -> None:
    def update() -> None:
        receipt = verify_runtime(backend, directory)
        source = _source_sha256(directory)
        operation = _operation(directory, source)
        _validate_request(operation, backend, receipt)
        supervisor = _validate_child(
            operation, "supervisor.json", "full-stack-worker-control-supervisor"
        )
        if operation["owner"]["operation_id"] != operation_id:
            raise ValueError("Worker 启动进度不属于当前操作")
        value = {
            **_common(
                "full-stack-worker-control-progress", operation_id, directory, source
            ),
            "phase": phase,
            "identity": identity,
            "owner_sha256": _digest(operation["owner"]),
            "request_sha256": _digest(operation["request"]),
            "supervisor_sha256": _digest(supervisor),
            "updated_at_ns": time.time_ns(),
        }
        _write_operation_receipt(operation, "progress.json", value, replace=True)

    _guarded_update(directory, update)


def _publish_result(
    operation_id: str, backend: Path, directory: Path, result: dict
) -> None:
    def update() -> None:
        receipt = verify_runtime(backend, directory)
        source = _source_sha256(directory)
        operation = _operation(directory, source)
        _validate_request(operation, backend, receipt)
        if operation["owner"]["operation_id"] != operation_id:
            raise ValueError("Worker 控制结果不属于当前操作")
        if (operation["path"] / "result.json").exists():
            raise ValueError("Worker 控制结果已经存在")
        _write_operation_receipt(operation, "result.json", result)

    _guarded_update(directory, update)


def _current_progress(
    operation_id: str, backend: Path, directory: Path, source: str
) -> dict | None:
    current = None

    def read() -> None:
        nonlocal current
        receipt = verify_runtime(backend, directory)
        operation = _operation(directory, source)
        _validate_request(operation, backend, receipt)
        if operation["owner"]["operation_id"] != operation_id:
            raise ValueError("Worker start 操作在等待期间发生变化")
        if (operation["path"] / "progress.json").is_file():
            current = _validate_progress(operation)

    _guarded_update(directory, read)
    return current


def _run_worker(
    backend: Path, directory: Path, operation: dict, supervisor: dict
) -> tuple[dict, subprocess.Popen | None, dict | None]:
    request = operation["request"]
    executable, url = request["worker_artifact"]["path"], request["worker_ready_url"]
    log_path = directory / request["log"]
    _progress(request["operation_id"], backend, directory, "launching", None)
    process, spawned_identity, identity, tree = None, None, None, None
    try:
        with log_path.open("xb") as log:
            process = subprocess.Popen(
                [executable],
                cwd=backend,
                env={**os.environ, "SNOWFLAKE_WORKER_ID": "2"},
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
        spawned_identity = process_identity(process.pid)
        if spawned_identity is None:
            raise RuntimeError("Worker 在登记进程创建身份前退出")
        spawned_identity = _valid_identity(spawned_identity, "Worker")
        if Path(spawned_identity["executable"]) != Path(executable).resolve(strict=True):
            raise RuntimeError("Worker 启动产物与实际进程可执行文件不一致")
        tree = record_process_tree(
            directory,
            "worker",
            request["scope_id"],
            supervisor,
            spawned_identity,
            request["operation_id"],
        )
        identity = record_process(
            directory, "worker", process.pid, executable, request["scope_id"]
        )["identity"]
        if identity != spawned_identity:
            raise RuntimeError("Worker 在登记进程收据期间身份发生变化")
        _progress(request["operation_id"], backend, directory, "started", identity)
        deadline = time.monotonic() + request["timeout_seconds"]
        while time.monotonic() < deadline:
            exit_code = process.poll()
            if exit_code is not None:
                raise RuntimeError(
                    f"Worker 就绪前退出，退出码 {exit_code}；日志：{log_path.name}"
                )
            if ready(url):
                verify_listener(process.pid, url)
                if not assert_identity(process_identity(process.pid), identity):
                    raise RuntimeError(
                        f"Worker 在就绪核验后退出；日志：{log_path.name}"
                    )
                _progress(
                    request["operation_id"], backend, directory, "ready", identity
                )
                return _result(operation, "succeeded", "running", identity), process, tree
            time.sleep(0.1)
        raise TimeoutError(f"Worker 就绪超时；日志：{log_path.name}")
    except BaseException as error:
        cleanup_identity = identity or spawned_identity
        try:
            if cleanup_identity is not None:
                terminate_owned_process(cleanup_identity, crash=True)
            if process is not None:
                process.wait(timeout=5)
            wait_for_port_free(url)
        except Exception as cleanup_error:
            return _result(
                operation,
                "failed",
                "unknown",
                cleanup_identity,
                error_type=type(cleanup_error).__name__,
                error=f"{error}；Worker 进程回收失败：{cleanup_error}",
            ), None, tree
        return _result(
            operation,
            "failed",
            "stopped",
            None,
            error_type=type(error).__name__,
            error=str(error) or type(error).__name__,
        ), None, tree


def _supervise_start(backend: Path, directory: Path, operation_id: str) -> int:
    receipt = verify_runtime(backend, directory)
    source = _source_sha256(directory)
    operation = _operation(directory, source)
    _validate_request(operation, backend, receipt)
    if (
        operation["owner"]["operation_id"] != operation_id
        or operation["request"]["operation"] != "start"
    ):
        raise ValueError("Worker 监督进程不属于当前 start 操作")
    observed = process_identity(os.getpid())
    if observed is None:
        raise ValueError("无法取得 Worker 启动监督进程身份")
    identity = _valid_identity(observed, "Worker 启动监督进程")
    candidate = _child_receipt(
        operation,
        "full-stack-worker-control-candidate",
        identity,
        created_at_ns=time.time_ns(),
    )
    _write_operation_receipt(operation, "candidate.json", candidate)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        authorization = operation["path"] / "supervisor.json"
        if authorization.is_file():
            supervisor = _validate_child(
                operation, "supervisor.json", "full-stack-worker-control-supervisor"
            )
            if supervisor["identity"] != identity or supervisor.get(
                "candidate_sha256"
            ) != _digest(candidate):
                raise ValueError("Worker 启动监督授权与实际进程不匹配")
            break
        if not operation["path"].is_dir():
            return 2
        time.sleep(0.02)
    else:
        return 2
    membership = enter_supervision()
    try:
        try:
            result, process, tree = _run_worker(backend, directory, operation, identity)
        except BaseException as error:
            result, process, tree = _result(
                operation,
                "failed",
                "unknown",
                None,
                error_type=type(error).__name__,
                error=str(error) or type(error).__name__,
            ), None, None
        _publish_result(operation_id, backend, directory, result)
        if result["outcome"] != "succeeded":
            return 1
        if process is None or tree is None:
            raise RuntimeError("Worker 成功收据缺少受监督的产品进程")
        code = supervise_product(process, tree)
        return code if 0 <= code <= 255 else 1
    finally:
        finish_supervision(membership)


def _authorize_supervisor(
    backend: Path, directory: Path, operation: dict
) -> subprocess.Popen:
    operation_id = operation["owner"]["operation_id"]
    environment = os.environ.copy()
    environment.pop("SNOWFLAKE_WORKER_ID", None)
    log = directory / f"worker-supervisor-{operation_id}.log"
    with log.open("xb") as output:
        process = subprocess.Popen(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "__supervise",
                "--backend-root",
                str(backend),
                "--runtime-dir",
                str(directory),
                "--operation-id",
                operation_id,
            ],
            cwd=backend,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
    candidate_path = operation["path"] / "candidate.json"
    deadline = time.monotonic() + 5
    spawned_identity = None
    try:
        spawned_identity = process_identity(process.pid)
        if spawned_identity is None:
            raise RuntimeError("Worker 启动监督进程在登记创建身份前退出")
        spawned_identity = _valid_identity(spawned_identity, "Worker 启动监督进程")
        while time.monotonic() < deadline:
            if candidate_path.is_file():
                candidate = _validate_child(
                    operation, "candidate.json", "full-stack-worker-control-candidate"
                )
                if candidate["identity"] != spawned_identity or not assert_identity(
                    process_identity(process.pid), spawned_identity
                ):
                    raise ValueError("Worker 启动监督候选与实际子进程不匹配")
                supervisor = _child_receipt(
                    operation,
                    "full-stack-worker-control-supervisor",
                    candidate["identity"],
                    candidate_sha256=_digest(candidate),
                    authorized_at_ns=time.time_ns(),
                )
                _write_operation_receipt(operation, "supervisor.json", supervisor)
                return process
            if process.poll() is not None:
                raise RuntimeError("Worker 启动监督进程在登记身份前退出")
            time.sleep(0.02)
        raise TimeoutError("Worker 启动监督进程未及时登记创建身份")
    except BaseException:
        if spawned_identity is not None:
            terminate_owned_process(spawned_identity, crash=True)
        process.wait(timeout=5)
        raise


def _simple_result(operation: dict, receipt: dict, operation_name: str) -> dict:
    directory = operation["path"].parent
    identity = worker_identity(directory, receipt)
    if operation_name in {"stop", "crash"}:
        tree = _worker_tree(directory, receipt, required=identity is not None)
        if tree is not None:
            terminate_owned_process_tree(tree, crash=operation_name == "crash")
        identity = None
        wait_for_port_free(receipt["worker_ready_url"])
    elif identity is None:
        tree = _worker_tree(directory, receipt, required=False)
        if tree is not None and assert_identity(
            process_identity(tree["supervisor"]["pid"]), tree["supervisor"]
        ):
            raise ValueError("Worker 已退出但进程树监督器仍在运行")
        ensure_port_free(receipt["worker_ready_url"])
    else:
        verify_running(identity, receipt["worker_ready_url"])
        _verify_worker_tree_running(directory, receipt, identity)
    return _result(
        operation,
        "succeeded",
        "running" if identity else "stopped",
        identity,
    )


def _reconcile_legacy(directory: Path, receipt: dict, source: str) -> str:
    lock = directory / LOCK
    if not lock.is_dir() or lock.is_symlink() or any(lock.iterdir()):
        raise ValueError("旧 Worker 控制锁不是可迁移的空目录")
    inode = _file_identity(lock)
    identity = worker_identity(directory, receipt)
    if identity is None:
        ensure_port_free(receipt["worker_ready_url"])
        state = "stopped"
    else:
        verify_running(identity, receipt["worker_ready_url"])
        _verify_worker_tree_running(directory, receipt, identity)
        state = "running"
    observed = process_identity(os.getpid())
    if observed is None:
        raise ValueError("无法取得旧锁核对控制器身份")
    reconciler = _valid_identity(observed, "旧锁核对控制器")
    operation_id = uuid.uuid4().hex
    migration = {
        **_common(
            "full-stack-worker-legacy-lock-reconciliation",
            operation_id,
            directory,
            source,
        ),
        "reconciler_identity": reconciler,
        "state": state,
        "worker_identity": identity,
        "completed_at_ns": time.time_ns(),
    }
    target = _archive_directory(directory) / f"legacy-{operation_id}.json"
    if target.exists():
        raise ValueError("旧 Worker 控制锁核对 ID 已存在")
    if _file_identity(lock) != inode or any(lock.iterdir()):
        raise ValueError("旧 Worker 控制锁在迁移期间发生变化")
    lock.rmdir()
    write_receipt(target, migration)
    return state


def _reconcile_active(
    backend: Path, directory: Path, receipt: dict, source: str
) -> dict:
    operation = _operation(directory, source)
    _validate_request(operation, backend, receipt)
    owner_identity = operation["owner"]["identity"]
    if process_identity(owner_identity["pid"]) == owner_identity:
        raise ValueError("Worker 控制器仍在运行，拒绝并发操作")
    result_path = operation["path"] / "result.json"
    if result_path.is_file():
        return _archive(operation)
    supervisor_path = operation["path"] / "supervisor.json"
    if supervisor_path.is_file():
        supervisor = _validate_child(
            operation, "supervisor.json", "full-stack-worker-control-supervisor"
        )
        if process_identity(supervisor["identity"]["pid"]) == supervisor["identity"]:
            raise _PendingStart("Worker 后台启动仍在执行")
        if result_path.is_file():
            return _archive(_operation(directory, source))
    else:
        candidate_path = operation["path"] / "candidate.json"
        if candidate_path.is_file():
            candidate = _validate_child(
                operation, "candidate.json", "full-stack-worker-control-candidate"
            )
            if process_identity(candidate["identity"]["pid"]) == candidate["identity"]:
                raise _PendingStart("未授权的 Worker 启动监督进程正在自行退出")
    identity = worker_identity(directory, receipt)
    requested = operation["request"]["operation"]
    if requested in {"stop", "crash"}:
        tree = _worker_tree(directory, receipt, required=identity is not None)
        if tree is not None:
            terminate_owned_process_tree(tree, crash=requested == "crash")
        wait_for_port_free(receipt["worker_ready_url"])
        identity, state, outcome, error = None, "stopped", "succeeded", None
    elif (
        requested == "start"
        and identity is not None
        and ready(receipt["worker_ready_url"])
    ):
        verify_listener(identity["pid"], receipt["worker_ready_url"])
        if not assert_identity(process_identity(identity["pid"]), identity):
            raise ValueError("Worker 在运行态核验后退出，不能声明正在运行")
        _verify_worker_tree_running(directory, receipt, identity)
        state, outcome, error = "running", "succeeded", None
    elif requested in {"status", "reconcile"}:
        if identity is None:
            ensure_port_free(receipt["worker_ready_url"])
        else:
            verify_running(identity, receipt["worker_ready_url"])
            _verify_worker_tree_running(directory, receipt, identity)
        state, outcome, error = (
            "running" if identity else "stopped",
            "succeeded",
            None,
        )
    else:
        if identity is not None:
            tree = _worker_tree(directory, receipt, required=True)
            terminate_owned_process_tree(tree, crash=True)
            wait_for_port_free(receipt["worker_ready_url"])
        else:
            ensure_port_free(receipt["worker_ready_url"])
        identity = None
        state, outcome, error = (
            "stopped",
            "failed",
            "中断的 Worker 控制未产生成功收据",
        )
    result = _result(
        operation,
        outcome,
        state,
        identity,
        reconciled=True,
        error_type=None if error is None else "InterruptedControl",
        error=error,
    )
    _write_operation_receipt(operation, "result.json", result)
    return _archive(operation)


def _finish_start(
    backend: Path,
    directory: Path,
    receipt: dict,
    source: str,
    operation_id: str,
    supervisor: subprocess.Popen,
    timeout: float,
) -> dict:
    deadline = time.monotonic() + timeout + 10
    checkpointed = False
    while time.monotonic() < deadline:
        lock = directory / LOCK
        if (lock / "result.json").is_file():
            with process_guard(directory, "worker-control.guard"):
                operation = _operation(directory, source)
                if operation["owner"]["operation_id"] != operation_id:
                    raise ValueError("Worker start 操作在等待期间发生变化")
                result = _archive(operation)
            if result["outcome"] == "succeeded" and result["state"] == "running":
                tree = _verify_worker_tree_running(
                    directory, receipt, result["identity"]
                )
                actual_supervisor = process_identity(supervisor.pid)
                if tree["supervisor"] != actual_supervisor:
                    raise ValueError("Worker 启动监督进程与进程树收据不一致")
                threading.Thread(
                    target=supervisor.wait,
                    name=f"worker-supervisor-{supervisor.pid}",
                    daemon=True,
                ).start()
            else:
                supervisor.wait(timeout=5)
            return result
        progress = lock / "progress.json"
        if not checkpointed and progress.is_file():
            observed = _current_progress(operation_id, backend, directory, source)
            if observed is not None and observed["phase"] in {"started", "ready"}:
                checkpointed = True
                _control_checkpoint("waiting-ready")
        if supervisor.poll() is not None:
            with process_guard(directory, "worker-control.guard"):
                result = _reconcile_active(backend, directory, receipt, source)
            return result
        time.sleep(0.05)
    raise TimeoutError("Worker 后台启动结果未知；请使用 status 或 reconcile 核对")


def _run_control(
    operation: str, backend: Path, directory: Path, timeout: float
) -> dict:
    with process_guard(directory, "worker-control.guard"):
        receipt = verify_runtime(backend, directory)
        source = _source_sha256(directory)
        lock = directory / LOCK
        if lock.exists():
            if operation == "start":
                raise ValueError(
                    "已有 Worker 控制操作，必须先使用 status 或 reconcile 核对"
                )
            if lock.is_dir() and not lock.is_symlink() and not any(lock.iterdir()):
                if operation != "reconcile":
                    raise ValueError("发现旧空锁；必须显式使用 reconcile 核对")
                _reconcile_legacy(directory, receipt, source)
            else:
                _reconcile_active(backend, directory, receipt, source)
        if operation == "start":
            if worker_identity(directory, receipt) is not None:
                raise ValueError("已登记 Worker 仍在运行，拒绝重复启动")
            previous_tree = _worker_tree(directory, receipt, required=False)
            if previous_tree is not None and assert_identity(
                process_identity(previous_tree["supervisor"]["pid"]),
                previous_tree["supervisor"],
            ):
                raise ValueError("上一代 Worker 进程树监督器仍在运行")
            ensure_port_free(receipt["worker_ready_url"])
        operation_receipt = _claim(
            operation, backend, directory, receipt, source, timeout
        )
        if operation != "start":
            _control_checkpoint(f"{operation}-prepared")
        if operation == "start":
            try:
                _control_checkpoint("prepared")
                supervisor = _authorize_supervisor(
                    backend, directory, operation_receipt
                )
                _control_checkpoint("authorized")
            except BaseException as error:
                result = _result(
                    operation_receipt,
                    "failed",
                    "unknown",
                    None,
                    error_type=type(error).__name__,
                    error=str(error) or type(error).__name__,
                )
                _write_operation_receipt(operation_receipt, "result.json", result)
                _archive(operation_receipt)
                raise
        else:
            try:
                result = _simple_result(operation_receipt, receipt, operation)
            except BaseException as error:
                result = _result(
                    operation_receipt,
                    "failed",
                    "unknown",
                    None,
                    error_type=type(error).__name__,
                    error=str(error) or type(error).__name__,
                )
                _write_operation_receipt(operation_receipt, "result.json", result)
                _archive(operation_receipt)
                raise
            _write_operation_receipt(operation_receipt, "result.json", result)
            return _archive(operation_receipt)
    result = _finish_start(
        backend,
        directory,
        receipt,
        source,
        operation_receipt["owner"]["operation_id"],
        supervisor,
        timeout,
    )
    if result["outcome"] != "succeeded":
        raise RuntimeError(result["error"] or "Worker 启动失败")
    return result


def control(
    operation: str, backend: Path, directory: Path, timeout: float = 60
) -> dict:
    if not isinstance(operation, str) or operation not in OPERATIONS:
        raise ValueError("未知 Worker 控制操作")
    if (
        isinstance(timeout, bool)
        or not isinstance(timeout, (int, float))
        or not 0 < timeout <= 180
    ):
        raise ValueError("Worker 就绪超时必须在 0 到 180 秒之间")
    backend, directory = backend.resolve(strict=True), directory.resolve(strict=True)
    deadline = time.monotonic() + timeout + 10
    while True:
        try:
            return _run_control(operation, backend, directory, timeout)
        except _PendingStart:
            if time.monotonic() >= deadline:
                raise TimeoutError("Worker 后台启动仍在执行；请稍后重试") from None
            time.sleep(0.05)


def _main_supervisor(arguments: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--backend-root", required=True, type=Path)
    parser.add_argument("--runtime-dir", required=True, type=Path)
    parser.add_argument("--operation-id", required=True)
    args = parser.parse_args(arguments)
    if OPERATION_ID.fullmatch(args.operation_id) is None:
        raise ValueError("Worker 监督操作 ID 无效")
    return _supervise_start(
        args.backend_root.resolve(strict=True),
        args.runtime_dir.resolve(strict=True),
        args.operation_id,
    )


def main() -> None:
    if sys.argv[1:2] == ["__supervise"]:
        raise SystemExit(_main_supervisor(sys.argv[2:]))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=tuple(sorted(OPERATIONS)))
    parser.add_argument("--backend-root", required=True, type=Path)
    parser.add_argument("--runtime-dir", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    result = control(args.operation, args.backend_root, args.runtime_dir, args.timeout)
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
