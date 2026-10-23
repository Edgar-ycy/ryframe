"""CI 协调器绑定 Rust 四库准备协议；不再实现 MySQL 校验或启动客户端。"""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from rate_limit_client import load_app_table
from rate_limit_tool import tool_binding, verified_tool

ROOT = Path(__file__).resolve().parents[2]
LIMIT = 4 * 1024 * 1024


@dataclass(frozen=True)
class DatabasePlan:
    scope_id: str
    settings: dict = field(repr=False)
    request: str = field(repr=False)


def _native(operation: str, request: dict) -> dict:
    payload = json.dumps(request, ensure_ascii=False).encode("utf-8")
    if len(payload) > LIMIT:
        raise ValueError("CI 数据库协议输入超过 4 MiB")
    environment = {key: value for key, value in os.environ.items()
                   if key.upper() in {"PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC",
                                      "HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "TEMP", "TMP",
                                      "DOCKER_HOST", "DOCKER_CONFIG", "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH"}
                   or key.upper().startswith(("CARGO_", "RUSTUP_"))}
    options = ({"creationflags": subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP}
               if os.name == "nt" else {})
    with verified_tool(tool_binding()) as executable:
        command = ([executable] if executable else
                   [os.environ.get("CARGO", "cargo"), "run", "--quiet", "--locked", "-p", "xtask", "--"])
        process = subprocess.Popen([*command, "__ci-databases", operation], cwd=ROOT,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                   env=environment, **options)
        try:
            output, _ = process.communicate(payload, timeout=180)
        except BaseException:
            # 先要求原生驱动取消并回收客户端进程树，不能超时后重放 DDL。
            try:
                process.send_signal(signal.CTRL_BREAK_EVENT if os.name == "nt" else signal.SIGINT)
                process.communicate(timeout=10)
            except (OSError, subprocess.TimeoutExpired):
                process.kill()
                process.communicate()
            raise
    if len(output) > LIMIT:
        raise RuntimeError("CI 数据库协议输出超过限额；保留现场")
    try:
        response = json.loads(output)
    except (ValueError, UnicodeError):
        raise RuntimeError("CI 数据库协议结果无效；保留现场，禁止自动重放") from None
    if isinstance(response, dict) and set(response) == {"ok", "result"} and response["ok"] is True and process.returncode == 0:
        return response["result"]
    if (isinstance(response, dict) and set(response) == {"ok", "error"} and response["ok"] is False
            and isinstance(response["error"], dict) and set(response["error"]) == {"code", "message"}
            and all(isinstance(value, str) for value in response["error"].values())):
        error = response["error"]
        exception = RuntimeError if error["code"] in {"creation_unknown", "config_publish"} else ValueError
        raise exception(f"{error['message']}（{error['code']}）")
    raise RuntimeError("CI 数据库协议响应与退出状态不一致；保留现场")


def load_control(backend: Path) -> dict:
    return _native("control", {"backend": str(backend)})


def plan_databases(required, backend_root: Path, output_dir: Path, *, config_dir: Path, container_id: str) -> DatabasePlan:
    if config_dir is None:
        raise ValueError("数据库准备必须显式绑定 config_dir")
    directory = config_dir if config_dir.is_absolute() else backend_root / config_dir
    settings = load_app_table(backend_root, {"APP_ENV": required("APP_ENV")}, config_dir=directory)
    request = {"format_version": 1, "backend": str(backend_root), "output": str(output_dir),
               "config_dir": str(directory), "github_actions": required("GITHUB_ACTIONS"),
               "run_id": required("GITHUB_RUN_ID"), "attempt": required("GITHUB_RUN_ATTEMPT"),
               "app_env": required("APP_ENV"), "container": container_id, "settings": settings,
               "common_text": (directory / "app.toml").read_text(encoding="utf-8"),
               "environment_text": (directory / "app.test.toml").read_text(encoding="utf-8")}
    result = _native("plan", request)
    return DatabasePlan(result["scope_id"], settings, json.dumps(request, ensure_ascii=False))


def _private_directory(directory: Path) -> None:
    directory.mkdir(mode=0o700, exist_ok=False)
    directory.chmod(0o700)
    if os.name == "nt":
        # Windows chmod 不收紧 ACL；只给当前调用者授予可继承权限。
        identity = subprocess.run(["whoami", "/user", "/fo", "csv", "/nh"],
                                  capture_output=True, text=True, check=True,
                                  creationflags=subprocess.CREATE_NO_WINDOW)
        match = re.search(r"S-1-[0-9-]+", identity.stdout)
        if match is None:
            raise ValueError("无法取得配置目录调用者 SID；保留现场并停止准备")
        result = subprocess.run(["icacls", str(directory), "/inheritance:r", "/grant:r",
                                 f"*{match[0]}:(OI)(CI)F"], capture_output=True,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        if result.returncode != 0:
            raise ValueError("无法收紧受管配置目录权限；保留现场并停止准备")


def _instance_text(text: str, worker_id: int) -> str:
    # 只替换顶层实例编号，禁止误改后续表里的同名字段。
    boundary = re.search(r"(?m)^\s*\[", text)
    head, tail = (text[:boundary.start()], text[boundary.start():]) if boundary else (text, "")
    head, count = re.subn(r'''(?m)^[ \t]*(?:snowflake_worker_id|'snowflake_worker_id'|"snowflake_worker_id")[ \t]*=.*$''',
                          f"snowflake_worker_id = {worker_id}", head)
    if count == 0:
        head = f"snowflake_worker_id = {worker_id}\n" + head
    return head + tail


def prepare_databases(required, backend_root: Path, output_dir: Path, *, plan: DatabasePlan | None = None,
                      config_dir: Path | None = None, container_id: str | None = None) -> Path:
    plan = plan or plan_databases(required, backend_root, output_dir, config_dir=config_dir, container_id=container_id)
    request = json.loads(plan.request)
    if request["backend"] != str(backend_root) or request["output"] != str(output_dir):
        raise ValueError("数据库计划与执行目录不一致，拒绝重放")
    return Path(_native("prepare", request)["config_dir"])
