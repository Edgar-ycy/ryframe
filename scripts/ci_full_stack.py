#!/usr/bin/env python3
"""准备、启动并收集 CI 真实全栈门禁的隔离资源。"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Callable, Protocol


from ci_full_stack_resources import BUCKETS, build_binaries, prepare_storage, read_binaries
from ci_full_stack_databases import DatabasePlan, plan_databases, prepare_databases
from full_stack_process import read_process, record_process, terminate_owned_process
from full_stack_runtime import register_runtime, worker_ready_url
from full_stack_worker import control as control_worker
from full_stack_worker import ensure_port_free, wait_for_port_free
from process_sockets import verify_listener
PLAN_HASH_PATTERN = re.compile(r"^plan_hash=([0-9a-f]{64})$", re.MULTILINE)


class FullStackError(RuntimeError):
    """全栈门禁无法安全准备或启动。"""


class ApiProcess(Protocol):
    def poll(self) -> int | None: ...


def _required_environment(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise FullStackError(f"缺少环境变量 {name}")
    return value


def _output_dir() -> Path:
    root = Path(_required_environment("RUNNER_TEMP"))
    if not root.is_absolute() or "\n" in str(root) or "\r" in str(root):
        raise FullStackError("RUNNER_TEMP 必须是无换行的绝对路径")
    return root / "ryframe-full-stack"


def _container_id(name: str) -> str:
    value = _required_environment(name)
    if not re.fullmatch(r"[a-f0-9]{12,64}", value):
        raise FullStackError(f"{name} 必须是当前 Job 的明确容器 ID")
    return value


def _api_ready_url() -> str:
    host = _required_environment("APP_APP_HOST")
    port = _required_environment("APP_APP_PORT")
    if host != "127.0.0.1" or not port.isdecimal() or not 1 <= int(port) <= 65535:
        raise FullStackError("测试 API 必须显式绑定本机 IPv4 和有效端口")
    return f"http://{host}:{port}/readyz"


def _preflight_prepare(backend_root: Path, output_dir: Path) -> DatabasePlan:
    plan = plan_databases(_required_environment, backend_root, output_dir)
    target = Path(_required_environment("CARGO_TARGET_DIR"))
    if not target.is_absolute():
        raise FullStackError("CARGO_TARGET_DIR 必须是明确的绝对路径")
    _container_id("RYFRAME_CI_REDIS_CONTAINER_ID")
    database = _required_environment("APP_REDIS_DATABASE")
    if not database.isdecimal() or not 0 <= int(database) <= 15:
        raise FullStackError("隔离 Redis 数据库编号必须在 0 到 15 之间")
    _required_environment("APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY")
    storage = os.environ.get("APP_OBJECT_STORAGE_BACKEND", "local")
    if storage not in {"local", "s3", "rustfs", "minio"}:
        raise FullStackError("对象存储后端无效")
    if storage != "local":
        _container_id("RYFRAME_CI_S3_CONTAINER_ID")
    return plan


def _run(
    arguments: list[str],
    *,
    cwd: Path,
    capture_output: bool = False,
    stdout: object | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        arguments,
        cwd=cwd,
        check=False,
        text=True,
        capture_output=capture_output,
        stdout=stdout,
        stderr=subprocess.STDOUT if stdout is not None else None,
        env=env,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() if completed.stderr else str(completed.returncode)
        raise FullStackError(f"命令失败（{' '.join(arguments)}）：{detail}")
    return completed


def prepare(backend_root: Path) -> None:
    """构建产品二进制、重建隔离资源，并显式应用全部迁移。"""

    output_dir = _output_dir()
    database_plan = _preflight_prepare(backend_root, output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    binaries = build_binaries(_run, backend_root, output_dir)
    prepare_storage(_required_environment)
    prepare_databases(
        _run, _required_environment, backend_root, output_dir, plan=database_plan
    )

    _run(
        [
            "docker",
            "exec",
            _container_id("RYFRAME_CI_REDIS_CONTAINER_ID"),
            "redis-cli",
            "-n",
            _required_environment("APP_REDIS_DATABASE"),
            "SET",
            _required_environment("APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY"),
            "ci-sentinel",
            "NX",
        ],
        cwd=backend_root,
        stdout=subprocess.DEVNULL,
    )
    reset = binaries["ryframe-reset"]
    plan = _run([str(reset), "plan"], cwd=backend_root, capture_output=True).stdout
    (output_dir / "reset-plan.log").write_text(plan, encoding="utf-8", newline="\n")
    hashes = PLAN_HASH_PATTERN.findall(plan)
    if not hashes:
        raise FullStackError("未能从 reset plan 读取有效 hash")
    confirmation = (
        f"RESET-RYFRAME-{_required_environment('APP_ENV')}-"
        f"{_required_environment('APP_SCOPE_ID')}"
    )
    with (output_dir / "reset-execute.log").open("w", encoding="utf-8") as log:
        _run(
            [
                str(reset),
                "execute",
                "--plan-hash",
                hashes[-1],
                "--confirm-reset",
                confirmation,
            ],
            cwd=backend_root,
            stdout=log,
        )
    migrate = binaries["ryframe-migrate"]
    _run([str(migrate), "control", "up"], cwd=backend_root)
    _run([str(migrate), "tenant-data", "up", "--all"], cwd=backend_root)
    _run([str(migrate), "control", "verify"], cwd=backend_root)
    _run([str(migrate), "tenant-data", "verify", "--all"], cwd=backend_root)


def _ready(url: str = "http://127.0.0.1:8080/readyz") -> bool:
    try:
        with urllib.request.urlopen(url, timeout=2) as response:
            return 200 <= response.status < 300
    except (OSError, urllib.error.URLError):
        return False


def wait_for_api(
    process: ApiProcess,
    *,
    ready: Callable[[], bool] = _ready,
    sleep: Callable[[float], None] = time.sleep,
    attempts: int = 90,
) -> None:
    for _ in range(attempts):
        exit_code = process.poll()
        if exit_code is not None:
            raise FullStackError(f"API 在就绪前退出，退出码 {exit_code}")
        if ready():
            return
        sleep(2)
    raise FullStackError("API 就绪等待超时")


def _tail(path: Path, lines: int = 200) -> str:
    if not path.is_file():
        return ""
    return "\n".join(path.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:])


def _stop_started_process(
    process, identity: dict | None, ready_url: str, *, crash: bool = True
) -> None:
    if identity is not None:
        terminate_owned_process(identity, crash=crash)
    elif process.poll() is None:
        process.kill()
    process.wait(timeout=5)
    wait_for_port_free(ready_url)


def start(backend_root: Path) -> None:
    """后台启动 API 与 Worker，并在超时或早退时附带最近日志。"""

    output_dir = _output_dir()
    if _required_environment("APP_JOBS_MODE") != "external":
        raise FullStackError("真实全栈验收必须启用 external Worker")
    api_ready_url = _api_ready_url()
    output_dir.mkdir(parents=True, exist_ok=True)
    receipt = register_runtime(backend_root, output_dir)
    if receipt["worker_ready_url"] == api_ready_url:
        raise FullStackError("API 和 Worker 必须使用不同的明确监听端口")
    binaries = read_binaries(output_dir)
    for name, binary, ready_url in (
        ("api", "ryframe", api_ready_url),
    ):
        if (output_dir / f"{name}.json").exists():
            raise FullStackError("运行目录已有 API 进程收据；请使用新的运行目录")
        ensure_port_free(ready_url)
        service_log = output_dir / f"{name}.log"
        service_environment = os.environ.copy()
        service_environment["SNOWFLAKE_WORKER_ID"] = "1" if name == "api" else "2"
        with service_log.open("ab") as log:
            process = subprocess.Popen(
                [binaries[binary]], cwd=backend_root, stdin=subprocess.DEVNULL,
                stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
                env=service_environment,
            )
        identity = None
        try:
            recorded = record_process(
                output_dir, name, process.pid, binaries[binary], receipt["scope_id"]
            )
            identity = recorded["identity"]
            wait_for_api(process, ready=lambda url=ready_url: _ready(url))
            verify_listener(process.pid, ready_url)
        except BaseException as error:
            try:
                _stop_started_process(process, identity, ready_url)
            except Exception as cleanup_error:
                raise FullStackError(
                    f"{name}: {error}；并且启动失败后的进程回收失败：{cleanup_error}"
                ) from error
            recent = _tail(service_log)
            if not isinstance(error, Exception):
                raise
            raise FullStackError(f"{name}: {error}\n{recent}") from error
    try:
        control_worker("start", backend_root, output_dir, timeout=180)
    except BaseException as error:
        try:
            identity = read_process(output_dir, "api", receipt["scope_id"])
            _stop_started_process(process, identity, api_ready_url, crash=False)
        except Exception as cleanup_error:
            raise FullStackError(
                f"Worker 启动失败：{error}；并且 API 回收失败：{cleanup_error}"
            ) from error
        if not isinstance(error, Exception):
            raise
        raise FullStackError(f"Worker 启动失败：{error}；API 已安全回收") from error


def _best_effort(arguments: list[str], *, output: Path) -> None:
    with output.open("a", encoding="utf-8") as log:
        try:
            subprocess.run(
                arguments,
                check=False,
                text=True,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
        except OSError as error:
            log.write(f"无法执行 {' '.join(arguments)}：{error}\n")


def collect() -> None:
    """尽力停止本 Job 的 API、Worker，并收集基础设施与编译缓存日志。"""

    output_dir = _output_dir()
    output_dir.mkdir(parents=True, exist_ok=True)
    failures: list[str] = []
    endpoints = {"worker": worker_ready_url, "api": _api_ready_url}
    for name in ("worker", "api"):
        if (output_dir / f"{name}.json").is_file():
            try:
                identity = read_process(output_dir, name, _required_environment("APP_SCOPE_ID"))
                terminate_owned_process(identity)
            except Exception as error:
                failures.append(f"{name}: {error}")
        try:
            wait_for_port_free(endpoints[name]())
        except Exception as error:
            failures.append(f"{name} 端口: {error}")

    for variable, filename in (
        ("RYFRAME_CI_MYSQL_CONTAINER_ID", "mysql.log"),
        ("RYFRAME_CI_REDIS_CONTAINER_ID", "redis.log"),
        ("RYFRAME_CI_S3_CONTAINER_ID", "rustfs.log"),
    ):
        container = os.environ.get(variable, "").strip()
        if container:
            try:
                _container_id(variable)
            except FullStackError as error:
                failures.append(str(error))
            else:
                _best_effort(["docker", "logs", container], output=output_dir / filename)
    sccache_log = output_dir / "sccache.log"
    _best_effort(["sccache", "--show-stats"], output=sccache_log)
    _best_effort(["sccache", "--stop-server"], output=sccache_log)
    if failures:
        details = "\n".join(failures)
        (output_dir / "cleanup-errors.log").write_text(details, encoding="utf-8")
        raise FullStackError(f"全栈进程未安全回收：\n{details}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("prepare", "start", "collect"))
    parser.add_argument("--backend-root", type=Path, default=Path.cwd())
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.operation == "prepare":
        prepare(args.backend_root.resolve())
    elif args.operation == "start":
        start(args.backend_root.resolve())
    else:
        collect()


if __name__ == "__main__":
    main()
