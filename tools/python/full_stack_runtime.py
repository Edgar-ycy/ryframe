"""绑定全栈测试的源码、运行目录、配置摘要和构建产物。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
from pathlib import Path

from artifact_digests import file_digest
from process_environment import configured
from ci_full_stack_resources import read_binaries
from full_stack_provenance import (
    full_stack_source_evidence,
    register_runtime_evidence,
    verify_build_evidence,
    verify_runtime_evidence,
)
from full_stack_process import write_receipt
from rate_limit_client import load_app_table


def process_environment() -> dict[str, str]:
    """产品子进程只继承 OS/第三方工具环境与唯一应用环境选择。"""
    return configured({"APP_ENV": "test"})


def environment_scope(backend: Path, config_dir: Path) -> str:
    table = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=config_dir)
    scope = table.get("scope_id", "")
    if (os.environ.get("APP_ENV") != "test" or not isinstance(scope, str)
            or not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", scope)):
        raise ValueError("全栈进程控制要求 APP_ENV=test，所选配置文件的 scope_id 必须是明确的隔离标识")
    if not isinstance(table.get("jobs"), dict) or table["jobs"].get("mode") != "external":
        raise ValueError("所选配置文件的 jobs.mode 必须为 external")
    return scope


def configuration_digest(backend: Path, config_dir: Path) -> str:
    values = {"APP_ENV": os.environ.get("APP_ENV")}
    load_app_table(backend, values, config_dir=config_dir)
    directory = config_dir.resolve(strict=True)
    paths = [directory / f"app.{values['APP_ENV']}.toml"]
    if (directory / "app.toml").exists():
        paths.insert(0, directory / "app.toml")
    files = {path.name: file_digest(path)["sha256"] for path in paths}
    payload = {"environment": values, "config_directory": str(directory), "files": files}
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def worker_ready_url(backend: Path, config_dir: Path) -> str:
    jobs = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=config_dir).get("jobs", {})
    if not isinstance(jobs, dict):
        raise ValueError("所选配置文件的 jobs 必须是配置表")
    host, port = jobs.get("health_host"), jobs.get("health_port")
    if host != "127.0.0.1":
        raise ValueError("所选配置文件的 jobs.health_host 必须显式绑定 127.0.0.1")
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("所选配置文件的 jobs.health_port 必须是 1–65535 的整数")
    return f"http://{host}:{port}/readyz"


def runtime_contract(backend: Path, directory: Path, *, api_config_dir: Path | None = None,
                     worker_config_dir: Path | None = None) -> dict:
    if os.environ.get("APP_ENV") != "test":
        raise ValueError("全栈进程控制要求 APP_ENV=test")
    binaries = read_binaries(directory)
    api_dir = api_config_dir if api_config_dir is not None else directory / "config"
    worker_dir = worker_config_dir if worker_config_dir is not None else directory / "worker-config"
    if (not api_dir.is_absolute() or not worker_dir.is_absolute() or api_dir.name != "config"
            or worker_dir.name != "worker-config" or api_dir.parent != worker_dir.parent):
        raise ValueError("运行收据必须绑定同一受管配置根内的 config 与 worker-config")
    for config_dir in (api_dir, worker_dir):
        if not config_dir.is_dir() or config_dir.is_symlink() or getattr(config_dir.lstat(), "st_file_attributes", 0) & 0x400:
            raise ValueError("全栈受管配置目录缺失或发生目录漂移")
    api = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=api_dir)
    worker = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=worker_dir)
    api_id, worker_id = api.get("snowflake_worker_id"), worker.get("snowflake_worker_id")
    if (type(api_id) is not int or type(worker_id) is not int
            or not 0 <= api_id <= 1023 or not 0 <= worker_id <= 1023 or api_id == worker_id):
        raise ValueError("API 与 Worker 的 snowflake_worker_id 无效或重复")
    if {k: v for k, v in api.items() if k != "snowflake_worker_id"} != {
            k: v for k, v in worker.items() if k != "snowflake_worker_id"}:
        raise ValueError("API 与 Worker 配置的 scope、数据库、Redis 或其他字段不匹配")
    api_digest, worker_digest = configuration_digest(backend, api_dir), configuration_digest(backend, worker_dir)
    combined = hashlib.sha256(json.dumps([api_digest, worker_digest]).encode()).hexdigest()
    return {
        "format_version": 1,
        "backend_root": str(backend.resolve()),
        "scope_id": environment_scope(backend, api_dir),
        "api_config_dir": str(api_dir.resolve(strict=True)),
        "worker_config_dir": str(worker_dir.resolve(strict=True)),
        "api_configuration_sha256": api_digest,
        "worker_configuration_sha256": worker_digest,
        "configuration_sha256": combined,
        "worker_ready_url": worker_ready_url(backend, worker_dir),
        "artifacts": {role: {"path": binaries[name], "sha256": file_digest(Path(binaries[name]))["sha256"]}
                      for role, name in (("api", "ryframe"), ("worker", "ryframe-worker"))},
    }


def prepare_runtime_configuration(backend: Path, directory: Path, source_config_dir: Path) -> None:
    """显式登记阶段复制已选配置；只读验证和预览不调用本入口。"""
    from ci_full_stack_databases import _instance_text, _private_directory

    if not source_config_dir.is_absolute() or source_config_dir.is_symlink():
        raise ValueError("运行配置来源必须是明确的无链接绝对目录")
    table = load_app_table(backend, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=source_config_dir)
    api_id = table.get("snowflake_worker_id")
    if type(api_id) is not int or not 0 <= api_id <= 1023 or api_id == 2:
        raise ValueError("API 实例编号无效或与 Worker 编号 2 冲突")
    files = [source_config_dir / "app.toml", source_config_dir / "app.test.toml"]
    before = {path.name: file_digest(path) for path in files}
    for name, worker_id in (("config", api_id), ("worker-config", 2)):
        destination = directory / name
        if destination.exists() or destination.is_symlink():
            raise ValueError("运行配置快照已存在，拒绝覆盖或自动重放")
        _private_directory(destination)
        for source in files:
            if source.is_symlink() or not source.is_file():
                raise ValueError("配置来源必须是无链接的普通文件")
            target = destination / source.name
            shutil.copyfile(source, target)
            target.chmod(0o600)
        environment = destination / "app.test.toml"
        environment.write_text(_instance_text(environment.read_text(encoding="utf-8"), worker_id), encoding="utf-8")
    if {path.name: file_digest(path) for path in files} != before:
        raise ValueError("来源配置在快照期间漂移；保留快照并停止登记")


def register_runtime(backend: Path, directory: Path, *, source_config_dir: Path | None = None,
                     api_config_dir: Path | None = None, worker_config_dir: Path | None = None) -> dict:
    source = full_stack_source_evidence(backend, directory)
    if source is not None and verify_build_evidence(backend, directory)["source"] != source:
        raise ValueError("运行登记使用的源码与构建来源不匹配")
    if source_config_dir is not None:
        if api_config_dir is not None or worker_config_dir is not None:
            raise ValueError("来源配置复制与已绑定双配置目录不能同时指定")
        from ci_full_stack_databases import _private_directory
        configuration = directory.parent / f"{directory.name}-configuration"
        _private_directory(configuration)
        prepare_runtime_configuration(backend, configuration, source_config_dir)
        api_config_dir, worker_config_dir = configuration / "config", configuration / "worker-config"
    contract = runtime_contract(backend, directory, api_config_dir=api_config_dir, worker_config_dir=worker_config_dir)
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
    if not isinstance(receipt, dict) or any(not isinstance(receipt.get(key), str)
                                          for key in ("api_config_dir", "worker_config_dir")):
        raise ValueError("全栈运行收据缺少明确的双配置目录")
    if receipt != runtime_contract(backend, directory, api_config_dir=Path(receipt["api_config_dir"]),
                                   worker_config_dir=Path(receipt["worker_config_dir"])):
        raise ValueError("全栈运行收据与当前 scope、配置或二进制不匹配")
    verify_runtime_evidence(backend, directory, path)
    return receipt
