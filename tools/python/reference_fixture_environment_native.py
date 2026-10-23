"""夹具环境原生守卫的正式绑定适配器；不猜测工具、不编译回退。"""
from __future__ import annotations

import json
import os
import subprocess

from native_s3_client import _child_environment
from rate_limit_tool import tool_binding, verified_tool

LIMIT = 4 * 1024 * 1024


def invoke(operation: str, arguments: dict) -> dict:
    binding = tool_binding()
    if binding is None:
        raise ValueError("夹具环境缺少正式原生工具绑定")
    payload = json.dumps({"format_version": 1, "operation": operation,
                          "arguments": arguments, "write": False},
                         ensure_ascii=False, separators=(",", ":")).encode()
    if len(payload) > LIMIT:
        raise ValueError("夹具环境协议超过 4 MiB")
    try:
        return _run(binding, payload)
    except (OSError, subprocess.SubprocessError):
        raise ValueError("夹具环境原生调用未确认；保留现场，不自动重放") from None


def _run(binding: dict, payload: bytes) -> dict:
    with verified_tool(binding) as executable:
        completed = subprocess.run(
            [executable, "__fixture-environment"], input=payload,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120, check=False,
            env=_child_environment(),
            **({"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}),
        )
    if len(completed.stdout) > LIMIT:
        raise ValueError("夹具环境响应过长；保留现场，不自动重放")
    try:
        response = json.loads(completed.stdout.decode("utf-8"), object_pairs_hook=_unique_object)
    except (ValueError, UnicodeError):
        raise ValueError("夹具环境原生响应无效；保留现场，不自动重放") from None
    if (completed.returncode == 0 and isinstance(response, dict)
            and set(response) == {"ok", "result"} and response["ok"] is True
            and isinstance(response["result"], dict)):
        return response["result"]
    if (completed.returncode != 0 and isinstance(response, dict)
            and set(response) == {"ok", "error"} and response["ok"] is False
            and isinstance(response["error"], dict)
            and response["error"].get("code") == "invalid_fixture_environment"
            and response["error"].get("remote_write_may_have_occurred") is False
            and response["error"].get("automatic_retry_permitted") is False
            and isinstance(response["error"].get("message"), str)):
        raise ValueError(response["error"]["message"])
    raise ValueError("夹具环境原生执行结果未知；保留现场，不自动重放")


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("夹具环境响应包含重复字段")
        value[key] = item
    return value
