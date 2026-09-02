#!/usr/bin/env python3
"""准备、启动并收集 CI 真实全栈门禁的隔离资源。"""

from __future__ import annotations

import argparse
import os
import re
import signal
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Callable, Protocol


BUCKETS = ("uploads", "avatar", "exports", "imports", "config-packages")
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
    return Path(_required_environment("RUNNER_TEMP")) / "ryframe-full-stack"


def _run(
    arguments: list[str],
    *,
    cwd: Path,
    capture_output: bool = False,
    stdout: object | None = None,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        arguments,
        cwd=cwd,
        check=False,
        text=True,
        capture_output=capture_output,
        stdout=stdout,
        stderr=subprocess.STDOUT if stdout is not None else None,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() if completed.stderr else str(completed.returncode)
        raise FullStackError(f"命令失败（{' '.join(arguments)}）：{detail}")
    return completed


def prepare(backend_root: Path) -> None:
    """构建产品二进制、重建隔离资源，并显式应用全部迁移。"""

    output_dir = _output_dir()
    output_dir.mkdir(parents=True, exist_ok=True)
    storage = Path(_required_environment("APP_OBJECT_STORAGE_LOCAL_BASE_DIR"))
    for bucket in BUCKETS:
        (storage / bucket).mkdir(parents=True, exist_ok=True)

    _run(
        [
            "docker",
            "exec",
            _required_environment("RYFRAME_CI_REDIS_CONTAINER_ID"),
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
    _run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            "bin-reset",
            "--bin",
            "ryframe-reset",
        ],
        cwd=backend_root,
    )
    _run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            "bin-migrate",
            "--bin",
            "ryframe-migrate",
        ],
        cwd=backend_root,
    )
    _run(
        [
            "cargo",
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
            "bin-api,runtime-swagger-ui",
            "--bin",
            "ryframe",
        ],
        cwd=backend_root,
    )

    reset = Path(_required_environment("CARGO_TARGET_DIR")) / "debug/ryframe-reset"
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
    migrate = Path(_required_environment("CARGO_TARGET_DIR")) / "debug/ryframe-migrate"
    _run([str(migrate), "control", "up"], cwd=backend_root)
    _run([str(migrate), "tenant-data", "up", "--all"], cwd=backend_root)


def _ready() -> bool:
    try:
        with urllib.request.urlopen("http://127.0.0.1:8080/readyz", timeout=2) as response:
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
        if ready():
            return
        exit_code = process.poll()
        if exit_code is not None:
            raise FullStackError(f"API 在就绪前退出，退出码 {exit_code}")
        sleep(2)
    raise FullStackError("API 就绪等待超时")


def _tail(path: Path, lines: int = 200) -> str:
    if not path.is_file():
        return ""
    return "\n".join(path.read_text(encoding="utf-8", errors="replace").splitlines()[-lines:])


def start(backend_root: Path) -> None:
    """后台启动 API，并在超时或早退时附带最近日志。"""

    output_dir = _output_dir()
    output_dir.mkdir(parents=True, exist_ok=True)
    api_log = output_dir / "api.log"
    executable = Path(_required_environment("CARGO_TARGET_DIR")) / "debug/ryframe"
    with api_log.open("ab") as log:
        process = subprocess.Popen(
            [str(executable)],
            cwd=backend_root,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    (output_dir / "api.pid").write_text(f"{process.pid}\n", encoding="ascii")
    try:
        wait_for_api(process)
    except FullStackError as error:
        recent = _tail(api_log)
        raise FullStackError(f"{error}\n{recent}" if recent else str(error)) from error


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


def _terminate_process_group(pid: int) -> None:
    if hasattr(os, "killpg"):
        os.killpg(pid, signal.SIGTERM)
    else:
        os.kill(pid, signal.SIGTERM)


def collect() -> None:
    """尽力停止本 Job 的 API，并收集基础设施与编译缓存日志。"""

    output_dir = _output_dir()
    output_dir.mkdir(parents=True, exist_ok=True)
    pid_file = output_dir / "api.pid"
    if pid_file.is_file():
        raw_pid = pid_file.read_text(encoding="ascii").strip()
        if raw_pid.isdecimal() and int(raw_pid) > 1:
            try:
                _terminate_process_group(int(raw_pid))
            except OSError:
                pass

    for variable, filename in (
        ("RYFRAME_CI_MYSQL_CONTAINER_ID", "mysql.log"),
        ("RYFRAME_CI_REDIS_CONTAINER_ID", "redis.log"),
    ):
        container = os.environ.get(variable, "").strip()
        if container:
            _best_effort(["docker", "logs", container], output=output_dir / filename)
    sccache_log = output_dir / "sccache.log"
    _best_effort(["sccache", "--show-stats"], output=sccache_log)
    _best_effort(["sccache", "--stop-server"], output=sccache_log)


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
