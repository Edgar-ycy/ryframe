"""只在当前 GitHub Job 的 MySQL 容器准备四个明确的新数据库。"""

from __future__ import annotations

import json
import os
import re
import shutil
from dataclasses import dataclass, field
from pathlib import Path

from full_stack_process import write_receipt


@dataclass(frozen=True)
class DatabasePlan:
    """建库前一次性固定全部公开参数和秘密凭据。"""

    source_config: Path
    config_dir: Path
    environment_file: Path
    scope_id: str
    container: str
    host: str
    port: int
    database: str
    username: str
    password: str = field(repr=False)
    tls_mode: str = "disabled"
    targets: tuple[tuple[str, str], ...] = ()
    schemas: tuple[str, ...] = ()


def plan_databases(required, backend_root: Path, output_dir: Path) -> DatabasePlan:
    """只读校验当前 Job 的四库计划，任何资源创建都在本函数之后。"""

    values = {name: required(name) for name in (
        "GITHUB_ACTIONS", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_ENV",
        "APP_ENV", "APP_SCOPE_ID", "APP_DATABASE_HOST", "APP_DATABASE_PORT",
        "APP_DATABASE_NAME", "APP_DATABASE_USERNAME", "APP_DATABASE_PASSWORD",
        "APP_DATABASE_TLS_MODE", "RYFRAME_CI_MYSQL_CONTAINER_ID",
    )}
    if values["GITHUB_ACTIONS"] != "true" or values["APP_ENV"] != "test":
        raise ValueError("数据库准备只接受 GitHub CI 的 test 环境")
    run_id, attempt = values["GITHUB_RUN_ID"], values["GITHUB_RUN_ATTEMPT"]
    if not re.fullmatch(r"[1-9][0-9]{0,19}", run_id) or not re.fullmatch(r"[1-9][0-9]{0,5}", attempt):
        raise ValueError("CI run ID 或 attempt 无效")
    prefix = f"ryframe_e2e_{run_id}_{attempt}"
    if (values["APP_DATABASE_HOST"] not in ("127.0.0.1", "localhost")
            or values["APP_DATABASE_NAME"] != prefix
            or values["APP_SCOPE_ID"] != f"ci-{run_id}-{attempt}"):
        raise ValueError("数据库与 scope 必须属于当前 CI 运行")
    container = values["RYFRAME_CI_MYSQL_CONTAINER_ID"]
    if not re.fullmatch(r"[a-f0-9]{12,64}", container):
        raise ValueError("必须提供当前 Job 的明确 MySQL 容器 ID")
    if not re.fullmatch(r"[1-9][0-9]{0,4}", values["APP_DATABASE_PORT"]):
        raise ValueError("MySQL 端口无效")
    port = int(values["APP_DATABASE_PORT"])
    if not 1 <= port <= 65535:
        raise ValueError("MySQL 端口无效")
    if not values["APP_DATABASE_USERNAME"] or not values["APP_DATABASE_PASSWORD"]:
        raise ValueError("MySQL 用户名和密码不能为空")
    if values["APP_DATABASE_TLS_MODE"] not in {"disabled", "required", "verify_ca", "verify_identity"}:
        raise ValueError("MySQL TLS 模式无效")
    source_config = backend_root / "config/app.toml"
    if not backend_root.is_absolute() or not output_dir.is_absolute():
        raise ValueError("后端和运行目录必须使用绝对路径")
    if "\n" in str(output_dir) or "\r" in str(output_dir):
        raise ValueError("运行目录不能包含换行")
    if not source_config.is_file():
        raise ValueError("后端基础配置文件不存在")
    environment_file = Path(values["GITHUB_ENV"])
    if (not environment_file.is_absolute() or "\n" in str(environment_file)
            or "\r" in str(environment_file) or not environment_file.parent.is_dir()):
        raise ValueError("GITHUB_ENV 必须是已有目录中的绝对文件路径")
    config_dir = output_dir / "config"
    if config_dir.exists() or (output_dir / "database-targets.json").exists():
        raise ValueError("运行目录已有数据库准备记录，拒绝自动重放")
    targets = (("shared", "shared"), ("dedicated-a", "dedicated"),
               ("dedicated-b", "dedicated"))
    schemas = (prefix, *(f"{prefix}_{key.replace('-', '_')}" for key, _ in targets))
    return DatabasePlan(
        source_config=source_config,
        config_dir=config_dir,
        environment_file=environment_file,
        scope_id=values["APP_SCOPE_ID"],
        container=container,
        host=values["APP_DATABASE_HOST"],
        port=port,
        database=values["APP_DATABASE_NAME"],
        username=values["APP_DATABASE_USERNAME"],
        password=values["APP_DATABASE_PASSWORD"],
        tls_mode=values["APP_DATABASE_TLS_MODE"],
        targets=targets,
        schemas=schemas,
    )


def prepare_databases(
    run,
    required,
    backend_root: Path,
    output_dir: Path,
    *,
    plan: DatabasePlan | None = None,
) -> None:
    plan = plan or plan_databases(required, backend_root, output_dir)
    config = plan.config_dir
    config.mkdir(exist_ok=False)
    shutil.copyfile(plan.source_config, config / "app.toml")
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
    try:
        run(["docker", "exec", "--env", "MYSQL_PWD", plan.container, "mysql",
             "--host=127.0.0.1", "--user=" + plan.username, "--execute=" + sql],
            cwd=backend_root, capture_output=True,
            env={**os.environ, "MYSQL_PWD": plan.password})
    except BaseException:
        write_receipt(receipt_path, {**receipt, "state": "creation-unknown"})
        raise
    write_receipt(receipt_path, {**receipt, "state": "created"})
    sections = []
    for (key, mode), schema in zip(plan.targets, plan.schemas[1:], strict=True):
        fields = {"key": key, "display_name": f"CI {key}", "kind": "mysql", "mode": mode,
                  "host": plan.host, "port": plan.port, "database": schema,
                  "username": plan.username, "password_env": "APP_DATABASE_PASSWORD",
                  "tls_mode": plan.tls_mode, "max_connections": 8}
        sections.append("[[tenant_data.targets]]\n" + "\n".join(
            f"{key} = {json.dumps(value)}" for key, value in fields.items()))
    (config / "app.test.toml").write_text("\n\n".join(sections) + "\n", encoding="utf-8")
    config_path = str(config.resolve())
    with plan.environment_file.open("a", encoding="utf-8") as environment_file:
        environment_file.write(f"APP_CONFIG_DIR={config_path}\n")
    os.environ["APP_CONFIG_DIR"] = config_path
