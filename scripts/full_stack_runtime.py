"""绑定全栈测试的明确运行目录、配置摘要和构建产物。"""

from __future__ import annotations

import hashlib
import json
import os
import re
from pathlib import Path

from artifact_digests import file_digest
from ci_full_stack_resources import read_binaries
from full_stack_process import write_receipt


def environment_scope() -> str:
    scope = os.environ.get("APP_SCOPE_ID", "")
    if os.environ.get("APP_ENV") != "test" or not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", scope):
        raise ValueError("全栈进程控制要求 APP_ENV=test 和明确的隔离 APP_SCOPE_ID")
    if os.environ.get("APP_JOBS_MODE") != "external":
        raise ValueError("全栈进程控制要求 external Worker")
    return scope


def configuration_digest(backend: Path) -> str:
    values = {key: value for key, value in os.environ.items() if key.startswith("APP_")}
    directory = Path(values.get("APP_CONFIG_DIR", str(backend / "config")))
    directory = (directory if directory.is_absolute() else backend / directory).resolve()
    files = {path.name: file_digest(path)["sha256"] for path in sorted(directory.glob("*.toml"))}
    if not files:
        raise ValueError("全栈进程配置目录缺少 TOML 文件")
    payload = {"environment": values, "config_directory": str(directory), "files": files}
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def worker_ready_url() -> str:
    host = os.environ.get("APP_JOBS_HEALTH_HOST", "")
    port = os.environ.get("APP_JOBS_HEALTH_PORT", "")
    if host != "127.0.0.1" or not port.isdecimal() or not 1 <= int(port) <= 65535:
        raise ValueError("测试 Worker 就绪地址必须显式绑定本机 IPv4 和有效端口")
    return f"http://{host}:{port}/readyz"


def runtime_contract(backend: Path, directory: Path) -> dict:
    binaries = read_binaries(directory)
    return {
        "format_version": 1,
        "backend_root": str(backend.resolve()),
        "scope_id": environment_scope(),
        "configuration_sha256": configuration_digest(backend),
        "worker_ready_url": worker_ready_url(),
        "artifacts": {role: {"path": binaries[name], "sha256": file_digest(Path(binaries[name]))["sha256"]}
                      for role, name in (("api", "ryframe"), ("worker", "ryframe-worker"))},
    }


def register_runtime(backend: Path, directory: Path) -> dict:
    contract = runtime_contract(backend, directory)
    path = directory / "runtime.json"
    if path.exists():
        if json.loads(path.read_text(encoding="utf-8")) != contract:
            raise ValueError("已登记全栈运行目录的配置或产物发生变化")
    else:
        write_receipt(path, contract)
    return contract


def verify_runtime(backend: Path, directory: Path) -> dict:
    receipt = json.loads((directory / "runtime.json").read_text(encoding="utf-8"))
    if receipt != runtime_contract(backend, directory):
        raise ValueError("全栈运行收据与当前 scope、配置或二进制不匹配")
    return receipt
