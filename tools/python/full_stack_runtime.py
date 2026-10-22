"""绑定全栈测试的源码、运行目录、配置摘要和构建产物。"""

from __future__ import annotations

import hashlib
import json
import os
import re
from pathlib import Path

from artifact_digests import file_digest
from ci_full_stack_resources import read_binaries
from full_stack_provenance import (
    full_stack_source_evidence,
    register_runtime_evidence,
    verify_build_evidence,
    verify_runtime_evidence,
)
from full_stack_process import write_receipt
from rate_limit_client import load_app_table


def environment_scope(backend: Path) -> str:
    table = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")})
    scope = table.get("scope_id", "")
    if (os.environ.get("APP_ENV") != "test" or not isinstance(scope, str)
            or not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", scope)):
        raise ValueError("全栈进程控制要求 APP_ENV=test，所选配置文件的 scope_id 必须是明确的隔离标识")
    if not isinstance(table.get("jobs"), dict) or table["jobs"].get("mode") != "external":
        raise ValueError("所选配置文件的 jobs.mode 必须为 external")
    return scope


def configuration_digest(backend: Path) -> str:
    values = {"APP_ENV": os.environ.get("APP_ENV")}
    load_app_table(backend, values)
    directory = (backend / "config").resolve()
    paths = [directory / f"app.{values['APP_ENV']}.toml"]
    if (directory / "app.toml").exists():
        paths.insert(0, directory / "app.toml")
    files = {path.name: file_digest(path)["sha256"] for path in paths}
    payload = {"environment": values, "config_directory": str(directory), "files": files}
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def worker_ready_url(backend: Path) -> str:
    jobs = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}).get("jobs", {})
    if not isinstance(jobs, dict):
        raise ValueError("所选配置文件的 jobs 必须是配置表")
    host, port = jobs.get("health_host"), jobs.get("health_port")
    if host != "127.0.0.1":
        raise ValueError("所选配置文件的 jobs.health_host 必须显式绑定 127.0.0.1")
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("所选配置文件的 jobs.health_port 必须是 1–65535 的整数")
    return f"http://{host}:{port}/readyz"


def runtime_contract(backend: Path, directory: Path) -> dict:
    binaries = read_binaries(directory)
    return {
        "format_version": 1,
        "backend_root": str(backend.resolve()),
        "scope_id": environment_scope(backend),
        "configuration_sha256": configuration_digest(backend),
        "worker_ready_url": worker_ready_url(backend),
        "artifacts": {role: {"path": binaries[name], "sha256": file_digest(Path(binaries[name]))["sha256"]}
                      for role, name in (("api", "ryframe"), ("worker", "ryframe-worker"))},
    }


def register_runtime(backend: Path, directory: Path) -> dict:
    source = full_stack_source_evidence(backend, directory)
    if source is not None and verify_build_evidence(backend, directory)["source"] != source:
        raise ValueError("运行登记使用的源码与构建来源不匹配")
    contract = runtime_contract(backend, directory)
    path = directory / "runtime.json"
    if path.exists():
        if json.loads(path.read_text(encoding="utf-8")) != contract:
            raise ValueError("已登记全栈运行目录的配置或产物发生变化")
    else:
        write_receipt(path, contract)
    register_runtime_evidence(backend, directory, path)
    return contract


def verify_runtime(backend: Path, directory: Path) -> dict:
    path = directory / "runtime.json"
    receipt = json.loads(path.read_text(encoding="utf-8"))
    if receipt != runtime_contract(backend, directory):
        raise ValueError("全栈运行收据与当前 scope、配置或二进制不匹配")
    verify_runtime_evidence(backend, directory, path)
    return receipt
