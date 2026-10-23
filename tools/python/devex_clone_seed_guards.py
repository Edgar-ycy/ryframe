"""close/rebind 只读模型的受管文件传输；不编译工具、不重试、不授权写入。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
from subprocess import run as run_native

from devex_clone_capture import unique_object, write_json
from devex_clone_run_state import binding
from rate_limit_client import ROOT, _child_environment
from rate_limit_tool import tool_binding, verified_tool
from workspace_directory import WorkspaceDirectory


def guard(operation: str, arguments: dict, *, backend: Path = ROOT, label: str = "seed close/rebind 证据"):
    with WorkspaceDirectory(dir=backend / ".local-tests/t", prefix="seed-guard-") as temporary:
        directory = Path(temporary)
        path = directory / "parameters.json"
        write_json(path, {"operation": operation, "arguments": arguments})
        payload = json.dumps({"format_version": 1, "kind": "ryframe-seed-evidence",
                              "run_dir": str(directory), "operation": "close-rebind-guard",
                              "arguments": {"backend_root": str(backend), "parameters": binding(path)}},
                             ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if len(payload) > 65536:
            raise ValueError("seed 守卫请求超过 64 KiB")
        options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
        try:
            with verified_tool(tool_binding()) as executable:
                if executable is None:
                    raise ValueError("seed 守卫必须由正式入口绑定工具；禁止临时编译或猜测工具")
                process = run_native([executable, "__seed-evidence"], cwd=ROOT, env=_child_environment(),
                                     input=payload, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     timeout=30, check=False, **options)
            if len(process.stdout) > 16 * 1024 * 1024:
                raise ValueError("seed 守卫响应超过 16 MiB")
            response = json.loads(process.stdout.decode("utf-8"), object_pairs_hook=unique_object)
        except (OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError):
            raise ValueError(f"{label}核验未完成；保留原阶段及未知结果，禁止自动重放") from None
        if (process.returncode != 0 or not isinstance(response, dict) or set(response) != {"ok", "result"}
                or response["ok"] is not True or not isinstance(response["result"], dict)):
            raise ValueError(f"{label}无效；保留原阶段及未知结果，禁止自动重放")
        result = response["result"]
        if (set(result) != {"value", "remote_writes", "automatic_retry_permitted"}
                or type(result["remote_writes"]) is not int or result["remote_writes"] != 0
                or result["automatic_retry_permitted"] is not False):
            raise ValueError(f"{label}响应无效；保留原阶段，禁止自动重放")
        return result["value"]
