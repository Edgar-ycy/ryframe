"""只在当前 GitHub Job 的 MySQL 容器准备四个明确的新数据库。"""

from __future__ import annotations

import json
import os
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from full_stack_process import write_receipt
from full_stack_migration_mysql import client_invocation, configured_databases, validate_binding
from rate_limit_client import load_app_table


@dataclass(frozen=True)
class DatabasePlan:
    """建库前一次性固定全部公开参数和秘密凭据。"""

    source_config: Path
    config_dir: Path
    worker_config_dir: Path
    source_environment: Path
    scope_id: str
    container: str
    host: str
    port: int
    database: str
    username: str
    password: str = field(repr=False)
    environment_text: str = field(repr=False)
    common_text: str = field(repr=False)
    api_worker_id: int
    worker_worker_id: int
    settings: dict = field(repr=False)
    tls: tuple[tuple[str, str], ...] = ()
    tls_mode: str = "disabled"
    targets: tuple[tuple[str, str], ...] = ()
    schemas: tuple[str, ...] = ()


def plan_databases(required, backend_root: Path, output_dir: Path, *, config_dir: Path, container_id: str) -> DatabasePlan:
    """只读校验当前 Job 的四库计划，任何资源创建都在本函数之后。"""

    values = {name: required(name) for name in (
        "GITHUB_ACTIONS", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT",
        "APP_ENV",
    )}
    if values["GITHUB_ACTIONS"] != "true" or values["APP_ENV"] != "test":
        raise ValueError("数据库准备只接受 GitHub CI 的 test 环境")
    run_id, attempt = values["GITHUB_RUN_ID"], values["GITHUB_RUN_ATTEMPT"]
    if not re.fullmatch(r"[1-9][0-9]{0,19}", run_id) or not re.fullmatch(r"[1-9][0-9]{0,5}", attempt):
        raise ValueError("CI run ID 或 attempt 无效")
    prefix = f"ryframe_e2e_{run_id}_{attempt}"
    directory = config_dir
    if not directory.is_absolute():
        directory = backend_root / directory
    settings = load_app_table(backend_root, {"APP_ENV": values["APP_ENV"]}, config_dir=directory)
    api_worker_id = settings.get("snowflake_worker_id")
    if type(api_worker_id) is not int or not 0 <= api_worker_id <= 1023 or api_worker_id == 2:
        raise ValueError("API snowflake_worker_id 必须为 0–1023 的整数且不能与 Worker 固定编号 2 冲突")
    database, _ = configured_databases(settings)
    validate_binding(database)
    if (database["database"] != prefix or settings.get("scope_id") != f"ci-{run_id}-{attempt}"):
        raise ValueError("数据库与 scope 必须属于当前 CI 运行")
    container = container_id
    if not isinstance(container, str) or not re.fullmatch(r"[a-f0-9]{12,64}", container):
        raise ValueError("必须提供当前 Job 的明确 MySQL 容器 ID")
    if database["tls_mode"] == "preferred":
        raise ValueError("MySQL TLS 模式无效")
    source_config = directory / "app.toml"
    source_environment = directory / "app.test.toml"
    if not backend_root.is_absolute() or not output_dir.is_absolute():
        raise ValueError("后端和运行目录必须使用绝对路径")
    if "\n" in str(output_dir) or "\r" in str(output_dir):
        raise ValueError("运行目录不能包含换行")
    if not source_config.is_file() or not source_environment.is_file():
        raise ValueError("后端基础配置或 app.test.toml 文件不存在")
    if "targets" in settings.get("tenant_data", {}):
        raise ValueError("输入配置不能定义 tenant_data.targets；租户目标由数据库准备阶段登记")
    config_dir = output_dir / "config"
    worker_config_dir = output_dir / "worker-config"
    if any(path.exists() or path.is_symlink() for path in (
            config_dir, worker_config_dir, output_dir / "database-targets.json")):
        raise ValueError("运行目录已有数据库准备记录，拒绝自动重放")
    targets = (("shared", "shared"), ("dedicated-a", "dedicated"),
               ("dedicated-b", "dedicated"))
    schemas = (prefix, *(f"{prefix}_{key.replace('-', '_')}" for key, _ in targets))
    return DatabasePlan(
        source_config=source_config,
        config_dir=config_dir,
        worker_config_dir=worker_config_dir,
        source_environment=source_environment,
        scope_id=settings["scope_id"],
        container=container,
        host=database["host"],
        port=database["port"],
        database=database["database"],
        username=database["username"],
        password=database["password"],
        environment_text=source_environment.read_text(encoding="utf-8"),
        common_text=source_config.read_text(encoding="utf-8"),
        api_worker_id=api_worker_id,
        worker_worker_id=2,
        settings=settings,
        tls=tuple((key, database[key]) for key in ("tls_ca", "tls_client_cert", "tls_client_key") if database.get(key)),
        tls_mode=database["tls_mode"],
        targets=targets,
        schemas=schemas,
    )


def _private_directory(directory: Path) -> None:
    directory.mkdir(mode=0o700, exist_ok=False)
    directory.chmod(0o700)
    if os.name == "nt":
        # Windows chmod 不收紧 ACL；只给当前调用者授予可继承权限。
        identity = subprocess.run(["whoami", "/user", "/fo", "csv", "/nh"],
                                  capture_output=True, text=True, check=True,
                                  creationflags=subprocess.CREATE_NO_WINDOW)
        match = re.search(r"S-1-[0-9-]+", identity.stdout)
        if match is None:
            raise ValueError("无法取得配置目录调用者 SID；保留现场并停止准备")
        result = subprocess.run(["icacls", str(directory), "/inheritance:r", "/grant:r",
                                 f"*{match[0]}:(OI)(CI)F"], capture_output=True,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        if result.returncode != 0:
            raise ValueError("无法收紧受管配置目录权限；保留现场并停止准备")


def _instance_text(text: str, worker_id: int) -> str:
    # 只替换顶层实例编号，禁止误改后续表里的同名字段。
    boundary = re.search(r"(?m)^\s*\[", text)
    head, tail = (text[:boundary.start()], text[boundary.start():]) if boundary else (text, "")
    head, count = re.subn(r'''(?m)^[ \t]*(?:snowflake_worker_id|'snowflake_worker_id'|"snowflake_worker_id")[ \t]*=.*$''',
                          f"snowflake_worker_id = {worker_id}", head)
    if count == 0:
        head = f"snowflake_worker_id = {worker_id}\n" + head
    return head + tail


def prepare_databases(
    run,
    required,
    backend_root: Path,
    output_dir: Path,
    *,
    plan: DatabasePlan | None = None,
    config_dir: Path | None = None,
    container_id: str | None = None,
) -> Path:
    if plan is None and config_dir is None:
        raise ValueError("数据库准备必须显式绑定 config_dir，不能默认读取后端 config")
    plan = plan or plan_databases(required, backend_root, output_dir, config_dir=config_dir, container_id=container_id)
    config = plan.config_dir
    for directory in (config, plan.worker_config_dir):
        _private_directory(directory)
        with (directory / "app.toml").open("x", encoding="utf-8") as output:
            (directory / "app.toml").chmod(0o600)
            output.write(plan.common_text)
    receipt_path = output_dir / "database-targets.json"
    receipt = {
        "format_version": 1,
        "scope_id": plan.scope_id,
        "container_id": plan.container,
        "host": plan.host,
        "port": plan.port,
        "schemas": list(plan.schemas),
        "state": "planned",
    }
    write_receipt(receipt_path, receipt)
    # CREATE 不使用 IF NOT EXISTS：已有同名资源意味着归属不明，必须失败。
    sql = "\n".join(
        f"CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;"
        for name in plan.schemas
    )
    binding = {"host": plan.host, "port": plan.port, "database": plan.database,
               "username": plan.username, "password": plan.password, "tls_mode": plan.tls_mode,
               "_scope_id": plan.scope_id, "_container_id": plan.container, **dict(plan.tls)}
    try:
        with client_invocation(binding, "mysql") as (command, environment, prefix):
            run(command, cwd=backend_root, capture_output=True, input=prefix + sql + "\n", env=environment)
    except BaseException as error:
        write_receipt(receipt_path, {**receipt, "state": "creation-unknown"})
        if not isinstance(error, Exception):
            raise
        raise RuntimeError("隔离 MySQL 建库失败；创建结果未知，保留收据并拒绝自动重放") from None
    write_receipt(receipt_path, {**receipt, "state": "created"})
    sections = [plan.environment_text]
    for (key, mode), schema in zip(plan.targets, plan.schemas[1:], strict=True):
        fields = {"key": key, "display_name": f"CI {key}", "kind": "mysql", "mode": mode,
                  "host": plan.host, "port": plan.port, "database": schema,
                  "username": plan.username, "password": plan.password,
                  "tls_mode": plan.tls_mode, "max_connections": 8, **dict(plan.tls)}
        sections.append("[[tenant_data.targets]]\n" + "\n".join(
            f"{key} = {json.dumps(value, ensure_ascii=False)}" for key, value in fields.items()))
    text = "\n\n".join(sections) + "\n"
    for directory, worker_id in ((config, plan.api_worker_id),
                                 (plan.worker_config_dir, plan.worker_worker_id)):
        with (directory / "app.test.toml").open("x", encoding="utf-8") as output:
            (directory / "app.test.toml").chmod(0o600)
            output.write(_instance_text(text, worker_id))
    return config.resolve()
