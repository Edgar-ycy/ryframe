#!/usr/bin/env python3
"""准备、启动并收集 CI 真实全栈门禁的隔离资源。"""

from __future__ import annotations

import os
import json
import argparse
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Callable, Protocol


from ci_full_stack_databases import DatabasePlan, load_control, plan_databases, prepare_databases
from ci_full_stack_resources import BUCKETS, bucket_request, build_binaries, create_bucket, read_binaries
from full_stack_process import read_process
from full_stack_process_tree import (
    SupervisedProcess,
    launch_supervised_process,
    read_process_tree,
    terminate_owned_process_tree,
)
from rate_limit_client import load_app_table, rate_limit_authority
from full_stack_runtime import environment_scope, process_environment, register_runtime
from full_stack_worker import control as control_worker
from full_stack_worker import ensure_port_free, wait_for_port_free
from process_sockets import verify_listener

PLAN_HASH_PATTERN = re.compile(r"^plan_hash=([0-9a-f]{64})$", re.MULTILINE)


class FullStackError(RuntimeError):
    """全栈门禁无法安全准备或启动。"""


class ApiProcess(Protocol):
    pid: int

    def poll(self) -> int | None: ...


def _required_environment(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise FullStackError(f"缺少环境变量 {name}")
    return value


def _output_dir(backend: Path) -> Path:
    return Path(load_control(backend)["runtime_dir"])


def _container_id(backend: Path, role: str) -> str:
    value = load_control(backend)[f"{role}_container_id"]
    if not re.fullmatch(r"[a-f0-9]{12,64}", value):
        raise FullStackError(f"{role}_container_id 必须是当前 Job 的明确容器 ID")
    return value


def _api_ready_url(backend: Path, config_dir: Path) -> str:
    app = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=config_dir).get("app", {})
    host, port = app.get("host"), app.get("port")
    if host != "127.0.0.1" or type(port) is not int or not 1 <= port <= 65535:
        raise FullStackError("测试 API 必须显式绑定本机 IPv4 和有效端口")
    return f"http://{host}:{port}/readyz"


def _preflight_prepare(backend_root: Path, output_dir: Path) -> DatabasePlan:
    plan = plan_databases(_required_environment, backend_root, output_dir,
                          config_dir=backend_root / ".local-tests/full-stack-config",
                          container_id=_container_id(backend_root, "mysql"))
    target = Path(_required_environment("CARGO_TARGET_DIR"))
    if not target.is_absolute():
        raise FullStackError("CARGO_TARGET_DIR 必须是明确的绝对路径")
    _container_id(backend_root, "redis")
    settings = plan.settings
    if settings.get("jobs", {}).get("mode") != "external":
        raise FullStackError("所选配置文件的 jobs.mode 必须为 external")
    database = settings.get("redis", {}).get("database")
    if type(database) is not int or not 0 <= database <= 15:
        raise FullStackError("隔离 Redis 数据库编号必须在 0 到 15 之间")
    if not settings.get("reset", {}).get("redis_outside_sentinel_key"):
        raise FullStackError("所选配置文件缺少 reset.redis_outside_sentinel_key")
    storage = settings.get("object_storage", {}).get("backend", "local")
    if storage not in {"local", "s3", "rustfs", "minio"}:
        raise FullStackError("对象存储后端无效")
    if storage != "local":
        _container_id(backend_root, "s3")
    return plan


def _prepare_storage(settings: dict) -> None:
    storage = settings["object_storage"]
    if storage["backend"] == "local":
        for bucket in BUCKETS:
            (Path(storage["local_base_dir"]) / bucket).mkdir(parents=True, exist_ok=True)
        return
    endpoint = storage["endpoint"]
    parsed = urllib.parse.urlsplit(endpoint)
    if (parsed.scheme not in ("http", "https") or parsed.hostname not in ("127.0.0.1", "localhost")
            or parsed.path not in ("", "/") or parsed.query or parsed.fragment or parsed.username):
        raise FullStackError("CI S3 端点必须指向本 Job 的 loopback 实例")
    deadline = time.monotonic() + 90
    while True:
        try:
            with socket.create_connection((parsed.hostname, parsed.port or (443 if parsed.scheme == "https" else 80)), timeout=2):
                break
        except OSError:
            if time.monotonic() >= deadline:
                raise FullStackError("隔离 RustFS 就绪等待超时") from None
            time.sleep(1)
    for bucket in BUCKETS:
        create_bucket(bucket_request(endpoint, bucket, storage["access_key"], storage["secret_key"]), bucket, deadline)


def _run(
    arguments: list[str],
    *,
    cwd: Path,
    capture_output: bool = False,
    stdout: object | None = None,
    env: dict[str, str] | None = None,
    input: str | None = None,
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
        input=input,
    )
    if completed.returncode != 0:
        detail = (
            completed.stderr.strip() if completed.stderr else str(completed.returncode)
        )
        raise FullStackError(f"命令失败（{' '.join(arguments)}）：{detail}")
    return completed


def prepare(backend_root: Path) -> None:
    """构建产品二进制、重建隔离资源，并显式应用全部迁移。"""

    output_dir = _output_dir(backend_root)
    database_plan = _preflight_prepare(backend_root, output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    binaries = build_binaries(_run, backend_root, output_dir)
    _prepare_storage(database_plan.settings)
    prepare_databases(
        _required_environment, backend_root, output_dir, plan=database_plan
    )
    config_arguments = ["--config-dir", str(output_dir / "config")]

    _run(
        [
            "docker",
            "exec",
            _container_id(backend_root, "redis"),
            "redis-cli",
            "-n",
            str(database_plan.settings["redis"]["database"]),
            "SET",
            database_plan.settings["reset"]["redis_outside_sentinel_key"],
            "ci-sentinel",
            "NX",
        ],
        cwd=backend_root,
        stdout=subprocess.DEVNULL,
    )
    reset = binaries["ryframe-reset"]
    plan = _run([str(reset), *config_arguments, "plan"], cwd=backend_root, capture_output=True, env=process_environment()).stdout
    (output_dir / "reset-plan.log").write_text(plan, encoding="utf-8", newline="\n")
    hashes = PLAN_HASH_PATTERN.findall(plan)
    if not hashes:
        raise FullStackError("未能从 reset plan 读取有效 hash")
    confirmation = (
        f"RESET-RYFRAME-{_required_environment('APP_ENV')}-"
        f"{database_plan.scope_id}"
    )
    with (output_dir / "reset-execute.log").open("w", encoding="utf-8") as log:
        _run(
            [
                str(reset),
                *config_arguments,
                "execute",
                "--plan-hash",
                hashes[-1],
                "--confirm-reset",
                confirmation,
            ],
            cwd=backend_root,
            stdout=log,
            env=process_environment(),
        )
    migrate = binaries["ryframe-migrate"]
    _run([str(migrate), *config_arguments, "control", "up"], cwd=backend_root, env=process_environment())
    _run([str(migrate), *config_arguments, "tenant-data", "up", "--all"], cwd=backend_root, env=process_environment())
    _run([str(migrate), *config_arguments, "control", "verify"], cwd=backend_root, env=process_environment())
    _run([str(migrate), *config_arguments, "tenant-data", "verify", "--all"], cwd=backend_root, env=process_environment())


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
    return "\n".join(
        path.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:]
    )


def _stop_started_process(
    process: SupervisedProcess, ready_url: str, *, crash: bool = True
) -> None:
    terminate_owned_process_tree(process.tree, crash=crash)
    process.wait(timeout=5)
    wait_for_port_free(ready_url)


def start(backend_root: Path) -> None:
    """后台启动 API 与 Worker，并在超时或早退时附带最近日志。"""

    output_dir = _output_dir(backend_root)
    api_ready_url = _api_ready_url(backend_root, output_dir / "config")
    output_dir.mkdir(parents=True, exist_ok=True)
    receipt = register_runtime(backend_root, output_dir)
    if receipt["worker_ready_url"] == api_ready_url:
        raise FullStackError("API 和 Worker 必须使用不同的明确监听端口")
    binaries = read_binaries(output_dir)
    for name, binary, ready_url in (("api", "ryframe", api_ready_url),):
        if (output_dir / f"{name}.json").exists() or (
            output_dir / f"{name}-tree.json"
        ).exists():
            raise FullStackError("运行目录已有 API 进程或进程树收据；请先核对现场")
        ensure_port_free(ready_url)
        service_log = output_dir / f"{name}.log"
        service_environment = process_environment()
        process = None
        try:
            with service_log.open("ab") as log:
                process = launch_supervised_process(
                    output_dir,
                    name,
                    receipt["scope_id"],
                    [str(Path(binaries[binary]).resolve(strict=True)), "--config-dir", receipt["api_config_dir"]],
                    backend_root,
                    service_environment,
                    log,
                )
            wait_for_api(process, ready=lambda url=ready_url: _ready(url))
            verify_listener(process.pid, ready_url)
        except BaseException as error:
            try:
                if process is not None:
                    _stop_started_process(process, ready_url)
                else:
                    wait_for_port_free(ready_url)
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
            _stop_started_process(process, api_ready_url, crash=False)
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


def collect(backend_root: Path | None = None) -> None:
    """尽力停止本 Job 的 API、Worker，并收集基础设施与编译缓存日志。"""

    backend_root = (backend_root or Path.cwd()).resolve()
    output_dir = _output_dir(backend_root)
    output_dir.mkdir(parents=True, exist_ok=True)
    failures: list[str] = []
    if (output_dir / "runtime.json").is_file():
        try:
            control_worker("stop", backend_root, output_dir, timeout=180)
        except Exception as error:
            failures.append(f"worker: {error}")
    elif (output_dir / "worker.json").exists() or (
        output_dir / "worker-control.lock"
    ).exists():
        failures.append("worker: 存在进程或控制收据，但缺少可信 runtime 收据")
    for name in ("api",):
        process_path = output_dir / f"{name}.json"
        tree_path = output_dir / f"{name}-tree.json"
        if process_path.is_file() or tree_path.is_file():
            try:
                scope = environment_scope(backend_root, output_dir / "config")
                tree = read_process_tree(
                    output_dir, name, scope
                )
                if not process_path.is_file():
                    failures.append("api: 进程树已登记，但缺少产品进程收据")
                elif tree["process"] != read_process(output_dir, name, scope):
                    raise ValueError("API 进程与进程树收据不一致")
                terminate_owned_process_tree(tree)
            except Exception as error:
                failures.append(f"{name}: {error}")
        try:
            wait_for_port_free(_api_ready_url(backend_root, output_dir / "config"))
        except Exception as error:
            failures.append(f"{name} 端口: {error}")

    for variable, filename in (
        ("mysql", "mysql.log"),
        ("redis", "redis.log"),
        ("s3", "rustfs.log"),
    ):
        container = _container_id(backend_root, variable)
        if container:
            try:
                _container_id(backend_root, variable)
            except FullStackError as error:
                failures.append(str(error))
            else:
                _best_effort(
                    ["docker", "logs", container], output=output_dir / filename
                )
    sccache_log = output_dir / "sccache.log"
    _best_effort(["sccache", "--show-stats"], output=sccache_log)
    _best_effort(["sccache", "--stop-server"], output=sccache_log)
    if failures:
        details = "\n".join(failures)
        (output_dir / "cleanup-errors.log").write_text(details, encoding="utf-8")
        raise FullStackError(f"全栈进程未安全回收：\n{details}")


def export_rate_limit(backend_root: Path, tool_config_file: Path) -> None:
    """从绑定配置导出 real_browser 工具使用的结构字段，不生成环境覆盖。"""

    directory = _output_dir(backend_root)
    authority = rate_limit_authority(backend_root, {"APP_ENV": _required_environment("APP_ENV")},
                                     config_dir=directory / "config")
    login = authority.get("login")
    if (not isinstance(login, dict) or type(login.get("capacity")) is not int
            or not 1 <= login["capacity"] <= 10_000
            or type(login.get("window_ms")) is not int
            or not 1_000 <= login["window_ms"] <= 86_400_000
            or login["window_ms"] % 1_000 != 0):
        raise FullStackError("绑定配置的登录限流容量或窗口无效，未写入工具配置")
    values = {"loginBudgetState": str(directory / "login-budget.json"),
              "loginCapacity": login["capacity"], "loginWindowSeconds": login["window_ms"] // 1_000}
    with tool_config_file.open("x", encoding="utf-8", newline="\n") as output:
        tool_config_file.chmod(0o600)
        json.dump(values, output, ensure_ascii=False, indent=2)
        output.write("\n")
    print("已从当前配置写入浏览器登录预算结构字段")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--operation", required=True, choices=("prepare", "rate-limit", "start", "collect"))
    parser.add_argument("--backend-root", required=True, type=Path)
    parser.add_argument("--tool-config-file", type=Path)
    args = parser.parse_args()
    if not args.backend_root.is_absolute():
        raise FullStackError("后端目录必须是明确绝对路径")
    backend_root = args.backend_root.resolve(strict=True)
    if args.operation == "rate-limit":
        if args.tool_config_file is None or not args.tool_config_file.is_absolute():
            raise FullStackError("rate-limit 必须明确提供 --tool-config-file 绝对路径")
        export_rate_limit(backend_root, args.tool_config_file)
    elif args.tool_config_file is not None:
        raise FullStackError("当前阶段不接受 --tool-config-file")
    else:
        {"prepare": prepare, "start": start, "collect": collect}[args.operation](backend_root)


if __name__ == "__main__":
    main()
