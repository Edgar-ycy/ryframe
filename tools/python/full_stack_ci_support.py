"""为历史运行配置复用原生目录权限与实例配置操作。"""
from __future__ import annotations

import json
from pathlib import Path


def configuration_directory(path: Path) -> None:
    _configuration("private-directory", {"path": str(path)})


def instance_text(text: str, worker_id: int) -> str:
    return _configuration("instance-text", {"text": text, "worker_id": worker_id})["text"]


def _configuration(operation: str, request: dict) -> dict:
    """仅传输已有 Rust 文件操作，不复制目录权限或配置改写规则。"""
    import os
    import subprocess
    from process_environment import configured
    from rate_limit_tool import tool_binding, verified_tool
    payload = json.dumps(request, ensure_ascii=False).encode("utf-8")
    if len(payload) > 4 * 1024 * 1024:
        raise ValueError("文件操作请求超过 4 MiB")
    root = Path(__file__).resolve().parents[2]
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    with verified_tool(tool_binding()) as executable:
        if executable is None:
            raise ValueError("文件操作缺少受管原生工具绑定；请通过正式 xtask 或 run_checks.py 绑定当前工具后执行，禁止临时编译回退")
        command = [executable]
        result = subprocess.run([*command, "__ci-databases", operation], cwd=root,
                                env=configured({}), input=payload, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=180, check=False, **options)
    if len(result.stdout) > 4 * 1024 * 1024:
        raise RuntimeError("文件操作响应超过 4 MiB；保留现场")
    try:
        value = json.loads(result.stdout, object_pairs_hook=unique)
    except (ValueError, UnicodeError):
        raise RuntimeError("文件操作未确认完成；保留现场，不自动重放") from None
    if (not isinstance(value, dict) or set(value) != {"ok", "result"}
            or value["ok"] is not True or result.returncode != 0):
        raise RuntimeError("文件操作未确认完成；保留现场，不自动重放")
    return value["result"]


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result: raise ValueError('重复字段')
        result[key] = value
    return result
