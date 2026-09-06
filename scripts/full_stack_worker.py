"""控制已经登记的隔离 external Worker；不暴露产品测试接口。"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from contextlib import contextmanager
from pathlib import Path

from full_stack_process import assert_identity, process_identity, read_process, record_process, terminate_owned_process
from full_stack_runtime import verify_runtime
from process_guard import process_guard
from process_sockets import verify_listener


@contextmanager
def worker_lock(directory: Path):
    # 不自动删除陈旧锁：控制器异常退出时保留证据，由明确的资源清理处理。
    path = directory / "worker-control.lock"
    try:
        path.mkdir()
    except FileExistsError as error:
        raise ValueError("Worker 控制正在执行，或上次控制异常退出；拒绝并发启动") from error
    try:
        yield
    finally:
        path.rmdir()


def worker_identity(directory: Path, receipt: dict) -> dict | None:
    if not (directory / "worker.json").is_file():
        return None
    identity = read_process(directory, "worker", receipt["scope_id"])
    if Path(identity.get("executable", "")) != Path(receipt["artifacts"]["worker"]["path"]):
        raise ValueError("Worker 进程收据与构建产物不匹配")
    return identity if assert_identity(process_identity(identity.get("pid")), identity) else None


def ready(url: str) -> bool:
    try:
        with urllib.request.urlopen(url, timeout=1) as response:
            return response.status == 200
    except (OSError, urllib.error.URLError):
        return False


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


def start_worker(backend: Path, directory: Path, receipt: dict, timeout: float) -> dict:
    if worker_identity(directory, receipt) is not None:
        raise ValueError("已登记 Worker 仍在运行，拒绝重复启动")
    ensure_port_free(receipt["worker_ready_url"])
    executable = receipt["artifacts"]["worker"]["path"]
    log_path = directory / f"worker-{uuid.uuid4().hex}.log"
    environment = {**os.environ, "SNOWFLAKE_WORKER_ID": "2"}
    with log_path.open("xb") as log:
        process = subprocess.Popen(
            [executable], cwd=backend, env=environment, stdin=subprocess.DEVNULL,
            stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
    identity = None
    try:
        recorded = record_process(
            directory, "worker", process.pid, executable, receipt["scope_id"]
        )
        identity = recorded["identity"]
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"Worker 就绪前退出；日志：{log_path.name}")
            if ready(receipt["worker_ready_url"]):
                verify_listener(process.pid, receipt["worker_ready_url"])
                if not assert_identity(process_identity(process.pid), identity):
                    raise RuntimeError(f"Worker 在就绪核验后退出；日志：{log_path.name}")
                return {"state": "running", "identity": identity, "log": log_path.name}
            time.sleep(0.2)
        raise TimeoutError(f"Worker 就绪超时；日志：{log_path.name}")
    except BaseException as error:
        try:
            if identity is not None:
                terminate_owned_process(identity, crash=True)
            elif process.poll() is None:
                process.kill()
            process.wait(timeout=5)
            wait_for_port_free(receipt["worker_ready_url"])
        except Exception as cleanup_error:
            raise RuntimeError(
                f"Worker 启动失败，并且进程回收失败：{cleanup_error}"
            ) from error
        raise


def control(operation: str, backend: Path, directory: Path, timeout: float = 60) -> dict:
    if operation not in {"start", "stop", "crash", "status"}:
        raise ValueError("未知 Worker 控制操作")
    if not 0 < timeout <= 180:
        raise ValueError("Worker 就绪超时必须在 0 到 180 秒之间")
    with process_guard(directory, "worker-control.guard"), worker_lock(directory):
        receipt = verify_runtime(backend, directory)
        if operation == "start":
            result = start_worker(backend, directory, receipt, timeout)
        elif operation in {"stop", "crash", "status"}:
            identity = worker_identity(directory, receipt)
            if operation != "status" and identity is not None:
                terminate_owned_process(identity, crash=operation == "crash")
                identity = None
                wait_for_port_free(receipt["worker_ready_url"])
            elif identity is None:
                ensure_port_free(receipt["worker_ready_url"])
            result = {"state": "running" if identity else "stopped", "identity": identity}
        return {"format_version": 1, "scope_id": receipt["scope_id"], "operation": operation, **result}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("start", "stop", "crash", "status"))
    parser.add_argument("--backend-root", required=True, type=Path)
    parser.add_argument("--runtime-dir", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    result = control(args.operation, args.backend_root.resolve(), args.runtime_dir.resolve(), args.timeout)
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
