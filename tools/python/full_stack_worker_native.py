"""Python 调用方使用的 Worker 有界 stdin 传输；状态机和进程控制全部由 Rust 持有。"""
from __future__ import annotations

import json
import os
import subprocess
from subprocess import run as run_native

from process_environment import configured
from rate_limit_tool import tool_binding, verified_tool


def _unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Worker 原生响应包含重复字段")
        result[key] = value
    return result


def invoke(operation, **arguments):
    payload = json.dumps({"format_version": 1, "kind": "ryframe-full-stack-worker",
                          "operation": operation, "arguments": arguments},
                         ensure_ascii=True, separators=(",", ":"), allow_nan=False).encode()
    if len(payload) > 1024 * 1024:
        raise ValueError("Worker 原生请求超过 1 MiB")
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    try:
        with verified_tool(tool_binding()) as executable:
            if executable is None:
                raise ValueError("Worker 控制要求正式入口绑定原生工具；禁止临时编译回退")
            result = run_native([executable, "__full-stack-worker"], input=payload,
                                env=configured({"APP_ENV": "test"}), stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=195, check=False, **options)
        if len(result.stdout) > 1024 * 1024:
            raise ValueError("Worker 原生响应超过 1 MiB")
        response = json.loads(result.stdout.decode("utf-8"), object_pairs_hook=_unique)
    except (OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError):
        raise RuntimeError("Worker 原生核验未完成；保留锁与收据，不自动重放") from None
    if (result.returncode == 1 and isinstance(response, dict)
            and set(response) == {"ok", "error", "message"} and response["ok"] is False
            and response["error"] == "WorkerRejected" and isinstance(response["message"], str)):
        if response["message"] == "全栈进程退出后就绪端口仍被占用":
            raise TimeoutError(response["message"])
        raise ValueError(response["message"])
    if (result.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True):
        raise RuntimeError("Worker 原生响应无效；保留现场，不自动清理或重放")
    return response["result"]
