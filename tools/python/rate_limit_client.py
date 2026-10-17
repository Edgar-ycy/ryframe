"""通过 Rust xtask 私有入口读取限流配置；本模块不复制配置合并规则。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
from typing import Mapping

ROOT = Path(__file__).resolve().parents[2]
MAX_OUTPUT_BYTES = 4 * 1024 * 1024
OPERATIONS = frozenset({"authority", "load-table", "settings", "login-budget"})
LOGIN_BUDGET_FIELDS = frozenset({
    "RYFRAME_E2E_LOGIN_BUDGET_STATE",
    "RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY",
    "RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS",
})
_ENCODING_NAMES = frozenset({"PYTHONUTF8", "PYTHONIOENCODING"})


def load_app_table(backend: Path, environment: Mapping[str, str]) -> dict:
    return _table_request("load-table", backend, environment)


def rate_limit_settings(backend: Path, environment: Mapping[str, str]) -> dict:
    return _table_request("settings", backend, environment)


def rate_limit_authority(backend: Path, environment: Mapping[str, str]) -> dict:
    return _table_request("authority", backend, environment)


def login_budget_environment(
    backend: Path, environment: Mapping[str, str], state_path: Path
) -> dict[str, str]:
    result = _invoke(
        "login-budget",
        {
            "backend": str(Path(backend).resolve()),
            "environment": _request_environment(environment),
            "state_path": str(state_path),
        },
    )
    if (
        not isinstance(result, dict)
        or set(result) != LOGIN_BUDGET_FIELDS
        or any(not isinstance(value, str) for value in result.values())
    ):
        raise RuntimeError("限流配置入口返回了无效的登录预算环境")
    return result


def _table_request(
    operation: str, backend: Path, environment: Mapping[str, str]
) -> dict:
    result = _invoke(
        operation,
        {
            "backend": str(Path(backend).resolve()),
            "environment": _request_environment(environment),
        },
    )
    if not isinstance(result, dict):
        raise RuntimeError("限流配置入口返回了无效的配置表")
    return result


def _request_environment(environment: Mapping[str, str]) -> dict[str, str]:
    if not isinstance(environment, Mapping) or any(
        not isinstance(key, str) or not isinstance(value, str)
        for key, value in environment.items()
    ):
        raise ValueError("限流配置环境必须是文本映射")
    return dict(environment)


def _invoke(operation: str, request: dict) -> dict:
    if operation not in OPERATIONS:
        raise ValueError("未知限流配置操作")
    payload = json.dumps(
        request, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    argv = [
        os.environ.get("CARGO") or "cargo",
        "run",
        "--quiet",
        "--locked",
        "-p",
        "xtask",
        "--",
        "__rate-limit",
        operation,
    ]
    options = {}
    if os.name == "nt":
        options["creationflags"] = subprocess.CREATE_NO_WINDOW
    try:
        result = subprocess.run(
            argv,
            cwd=ROOT,
            env=_child_environment(),
            input=payload,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=30,
            check=False,
            **options,
        )
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError("无法启动 Rust 限流配置入口") from None
    if len(result.stdout) > MAX_OUTPUT_BYTES:
        raise RuntimeError("Rust 限流配置入口输出超出限额")
    try:
        response = json.loads(result.stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise RuntimeError("Rust 限流配置入口返回了无效 JSON") from None
    if not isinstance(response, dict):
        raise RuntimeError("Rust 限流配置入口返回了无效协议")
    if set(response) == {"ok", "result"} and response["ok"] is True:
        if result.returncode != 0:
            raise RuntimeError("Rust 限流配置入口返回了不一致的退出状态")
        return response["result"]
    if set(response) == {"ok", "error"} and response["ok"] is False:
        _raise_error(response["error"])
    raise RuntimeError("Rust 限流配置入口返回了无效协议")


def _child_environment() -> dict[str, str]:
    return {
        key: value
        for key, value in os.environ.items()
        if key.upper() not in _ENCODING_NAMES and not key.upper().startswith("APP_")
    }


def _raise_error(value: object) -> None:
    if (
        not isinstance(value, dict)
        or set(value) != {"code", "message"}
        or not isinstance(value["code"], str)
        or not isinstance(value["message"], str)
    ):
        raise RuntimeError("Rust 限流配置入口返回了无效错误")
    if value["code"] == "config_file_missing":
        raise FileNotFoundError(value["message"])
    raise ValueError(f"{value['message']}（{value['code']}）")
