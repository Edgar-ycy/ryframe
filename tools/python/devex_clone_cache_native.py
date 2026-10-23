"""缓存进程证据和控制调用的有界 Rust 传输；不重试，不读取环境凭据。"""
from __future__ import annotations

import json
import os
import subprocess
from subprocess import run as run_native

from devex_clone_capture import unique_object
from rate_limit_client import ROOT, _child_environment
from rate_limit_tool import tool_binding, verified_tool
from process_environment import configured


def invoke(operation, **arguments):
    try:
        payload = json.dumps({"format_version": 1, "kind": "ryframe-cache-process",
                              "operation": operation, "arguments": arguments},
                             ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    except (TypeError, ValueError, UnicodeError):
        raise ValueError("缓存原生请求必须是有效 UTF-8 JSON") from None
    if len(payload) > 1024 * 1024:
        raise ValueError("缓存原生请求超过 1 MiB")
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    try:
        with verified_tool(tool_binding()) as executable:
            if executable is None:
                raise ValueError("缓存控制必须由正式入口绑定原生工具；不能临时编译或猜测工具")
            result = run_native([executable, "__cache-process"], cwd=ROOT,
                                    env=configured({}, inherited=_child_environment()), input=payload,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    timeout=30, check=False, **options)
        if len(result.stdout) > 1024 * 1024:
            raise ValueError("缓存原生响应超过 1 MiB")
        response = json.loads(result.stdout.decode("utf-8"), object_pairs_hook=unique_object)
    except (OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError):
        raise RuntimeError("缓存原生调用未完成；保留意图与锁，不重放任何写入") from None
    if (result.returncode == 1 and isinstance(response, dict)
            and set(response) == {"ok", "error", "message", "exit_code"} and response["ok"] is False
            and response["error"] in {"CacheProcessRejected", "CacheControlFailed", "CacheConnectionRefused", "CacheObservationTimeout"}
            and isinstance(response["message"], str)):
        if response["error"] == "CacheConnectionRefused":
            raise ConnectionRefusedError(response["message"])
        if response["error"] == "CacheObservationTimeout":
            raise TimeoutError(response["message"])
        code = response["exit_code"]
        if code is not None and type(code) is not int:
            raise RuntimeError("缓存原生退出状态无效；保留现场，禁止重放")
        message = response["message"] + (" 原始退出码：" + str(code) if code is not None else "")
        if response["error"] == "CacheControlFailed":
            raise RuntimeError(message)
        raise ValueError(message)
    if (result.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True):
        raise RuntimeError("缓存原生响应无效；保留现场，不自动重放或清理")
    return response["result"]
