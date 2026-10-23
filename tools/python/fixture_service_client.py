"""Python 调用原生夹具操作与生命周期；只传协议，不决定阶段、恢复或重试。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess

from rate_limit_tool import tool_binding, verified_tool
from process_environment import configured

ROOT = Path(__file__).resolve().parents[2]
MAX_BYTES = 4 * 1024 * 1024


def _invoke(operation: str, arguments: dict, *, backend: Path = ROOT):
    payload = json.dumps({"format_version": 1, "operation": operation,
                          "backend": str(backend), "arguments": arguments},
                         ensure_ascii=False, separators=(",", ":"), default=_path_json).encode("utf-8")
    return _request("__fixture-service-history", payload, 30, "invalid_fixture_history")


def lifecycle(backend: Path, operation: str, review: Path, environment: Path,
              owner: Path | None, *, write: bool) -> dict:
    payload = json.dumps({"format_version": 1, "backend": str(backend), "operation": operation,
                          "review": str(review), "environment": str(environment),
                          "owner": None if owner is None else str(owner), "write": write},
                         ensure_ascii=False, separators=(",", ":"), default=_path_json).encode("utf-8")
    return _request("__fixture-service-lifecycle", payload, 1800, "invalid_fixture_lifecycle")


def _path_json(value):
    if isinstance(value, Path):
        return str(value)
    raise TypeError("原生夹具协议只允许 JSON 与 Path")


def _request(marker: str, payload: bytes, timeout: int, error_code: str):
    if len(payload) > MAX_BYTES:
        raise ValueError("夹具原生请求超过 4 MiB")
    environment = configured({})
    environment.update({key: value for key, value in os.environ.items()
                        if key.upper().startswith(("CARGO_", "RUSTUP_"))})
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    try:
        with verified_tool(tool_binding()) as executable:
            command = ([executable] if executable is not None else
                       [os.environ.get("CARGO") or shutil.which("cargo") or "cargo",
                        "run", "--quiet", "--locked", "-p", "xtask", "--"])
            result = subprocess.run([*command, marker], cwd=ROOT,
                                    env=environment, input=payload, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, timeout=timeout, check=False, **options)
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError("无法完成原生夹具操作校验；保留现场，不自动重放") from None
    if len(result.stdout) > MAX_BYTES:
        raise RuntimeError("原生夹具操作响应超过 4 MiB")
    try:
        response = json.loads(result.stdout.decode("utf-8"), object_pairs_hook=_unique_object)
    except (ValueError, UnicodeError):
        raise RuntimeError("原生夹具操作响应不是有效 JSON") from None
    if (isinstance(response, dict) and set(response) == {"ok", "result"}
            and response["ok"] is True and result.returncode == 0):
        return response["result"]
    if (isinstance(response, dict) and set(response) == {"ok", "error"}
            and response["ok"] is False and result.returncode == 2
            and isinstance(response["error"], dict)
            and set(response["error"]) == {"code", "message"}
            and response["error"]["code"] == error_code
            and isinstance(response["error"]["message"], str)):
        raise ValueError(response["error"]["message"])
    raise RuntimeError("原生夹具操作响应与退出状态不一致")


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("原生夹具操作响应不能重复字段")
        value[key] = item
    return value


def validate_manifest(value: dict) -> None:
    _invoke("manifest", {"value": value})


def validate_initial(state: dict) -> None:
    _invoke("initial", {"state": state})


def validate_history(run: Path, state: dict, *, ready: bool = True, settled: bool = True) -> dict:
    return _invoke("history", {"run": str(run), "state": state, "ready": ready, "settled": settled})
