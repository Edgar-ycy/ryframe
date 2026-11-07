"""初始化状态机到原生只读证据核验器的有界标准输入传输。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

from rate_limit_client import ROOT, _child_environment
from rate_limit_tool import tool_binding, verified_tool
from devex_clone_capture import unique_object
from devex_clone_model import digest


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
    registered = tool_binding()
    if registered is None:
        raise ValueError("目标证据缺少受管 Rust 工具；请通过正式 xtask 或 run_checks.py 绑定工具后执行")
    try:
        with verified_tool(registered) as executable:
            if not executable:
                raise ValueError("目标证据缺少受管 Rust 工具；请核对绑定，禁止临时编译回退")
            command = [executable, "__target-evidence"]
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
    value = response["result"]
    if operation in {"process-argv", "storage-layout", "storage-generation", "fixture-storage-layout"}:
        if type(value.get("remote_writes")) is not int or value["remote_writes"] != 0:
            raise ValueError("原生存储观察响应无效；保留现场，不重放")
        if operation == "process-argv":
            if set(value) != {"actual_arguments_sha256", "remote_writes"}:
                raise ValueError("原生参数观察只允许安全摘要；保留现场，不重放")
            digest(value["actual_arguments_sha256"])
        elif value != {"status": operation.replace("-", "_") + "_verified", "remote_writes": 0}:
            raise ValueError("原生存储观察响应无效；保留现场，不重放")
    return value
