"""初始化状态机到原生只读证据核验器的有界标准输入传输。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess

from rate_limit_client import ROOT, _child_environment
from rate_limit_tool import tool_binding, verified_tool
from devex_clone_capture import unique_object


def evidence(operation: str, backend: Path, target: Path, **fields) -> dict:
    # 显式保持原控制器的后端边界；不从嵌套目录或环境变量推断来源。
    if not backend.is_absolute() or not target.is_absolute():
        raise ValueError("目标证据必须绑定绝对后端及目标路径")
    try:
        relative = target.relative_to(backend / ".local-tests")
    except ValueError:
        raise ValueError("目标证据必须位于明确后端的 .local-tests 内") from None
    if not relative.parts:
        raise ValueError("目标证据必须位于明确后端的 .local-tests 内")
    payload = json.dumps({"format_version": 1, "operation": operation,
                          "backend": str(backend), "target": str(target), **fields},
                         ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(payload) > 1024 * 1024:
        raise ValueError("目标证据请求超过 1 MiB")
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    try:
        with verified_tool(tool_binding()) as executable:
            command = [executable, "__target-evidence"] if executable else [
                os.environ.get("CARGO") or shutil.which("cargo") or "cargo",
                "run", "--quiet", "--locked", "-p", "xtask", "--", "__target-evidence"]
            result = subprocess.run(command, cwd=ROOT, env=_child_environment(), input=payload,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    timeout=30, check=False, **options)
        if len(result.stdout) > 4 * 1024 * 1024:
            raise ValueError("目标证据响应超过 4 MiB")
        response = json.loads(result.stdout.decode("utf-8"), object_pairs_hook=unique_object)
    except (OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError):
        raise ValueError("原生目标证据进程未完成；保留现场，不重放任何写入") from None
    if (result.returncode == 1 and isinstance(response, dict)
            and set(response) == {"ok", "error"} and response["ok"] is False
            and isinstance(response["error"], dict)
            and set(response["error"]) == {"category", "message", "remote_writes", "automatic_retry"}
            and response["error"]["category"] == "TargetEvidenceRejected"
            and response["error"]["remote_writes"] == 0
            and response["error"]["automatic_retry"] is False
            and isinstance(response["error"]["message"], str)):
        raise ValueError(response["error"]["message"])
    if (result.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True
            or not isinstance(response["result"], dict)):
        raise ValueError("原生目标证据核验失败；核对计划、账本及直接前像，不自动重放或清理")
    return response["result"]
