"""为新的隔离参考夹具生成首代环境计划；plan 只读，不启动服务或创建资源。"""
from __future__ import annotations

import argparse
import configparser
import copy
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit

from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import Environments, configured
from devex_clone_model import linked, local_path
from devex_clone_target_binding import KEYS, validate_review
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest
from restore_build import file_digest
from restore_reference_plan import plan_hash
from source_inventory import snapshot


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("夹具输入必须是受控目录中的普通文件")
    return path, read_json(path)


def _fixture(value: dict) -> None:
    if (value.get("format_version") != 1 or value.get("fixture") != "device"
            or value.get("status") != "ready" or not isinstance(value.get("sources"), dict)
            or set(value.get("paths", {})) != {"backend", "frontend"}
            or not all(isinstance(item, dict) and isinstance(item.get("head"), str)
                       for item in value["sources"].values())):
        raise ValueError("Device 隔离工作树收据不完整或尚未就绪")


def _maintenance(value: dict) -> None:
    artifacts = value.get("artifacts")
    if (value.get("format_version") != 1 or value.get("kind") != "devex-clone-tool-build"
            or value.get("resources_modified") is not False
            or not isinstance(artifacts, dict)
            or not {"reset", "migrate", "tenant-data"}.issubset(artifacts)):
        raise ValueError("维护构建收据不完整")
    for name in ("reset", "migrate", "tenant-data"):
        item = artifacts[name]
        if not isinstance(item, dict) or not isinstance(item.get("executable"), str):
            raise ValueError("维护构建产物描述无效")


def _command(run, arguments: list[str]) -> str:
    completed = run(arguments, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return completed.stdout.decode("utf-8").strip()


def _preflight(review: dict, run=subprocess.run) -> dict:
    """重新读取计划绑定的工具，不启动服务、不连接业务资源。"""
    validate_review(review)
    tools = review["tools"]
    observed = {}
    for name in ("mysql", "aws", "rustfs"):
        item = tools[name]
        path = Path(item["path"])
        if not path.is_absolute() or linked(path) or not path.is_file() or file_digest(path)["sha256"] != item["sha256"]:
            raise ValueError(f"审阅计划中的 {name} 工具已变化")
        observed[name] = {"path": str(path), "sha256": item["sha256"]}
    redis = tools["redis_server"]
    distribution = redis["distribution"]
    if not re.fullmatch(r"[A-Za-z0-9._-]+", distribution):
        raise ValueError("审阅计划中的 Redis 发行版无效")
    launcher = "/usr/bin/redis-server"
    prefix = ["wsl", "--distribution", distribution, "--exec"]
    resolved = _command(run, [*prefix, "/usr/bin/readlink", "-f", launcher])
    digest = _command(run, [*prefix, "/usr/bin/sha256sum", launcher]).split(maxsplit=1)[0]
    version = _command(run, [*prefix, launcher, "--version"])
    # Ubuntu 将 redis-server 链接到 multi-call 二进制时，内核实际路径可以是 redis-check-rdb；
    # 启动入口始终固定为 redis-server，运行身份则绑定 readlink 的实际文件。
    if (not re.fullmatch(r"/usr/bin/(?:redis-server|redis-check-rdb)", resolved) or not re.fullmatch(r"[a-f0-9]{64}", digest)
            or not version.startswith("Redis server v=")):
        raise ValueError("本机 WSL Redis 服务二进制未通过只读核验")
    observed["redis_server"] = {"distribution": distribution, "resolved_path": resolved,
                                "sha256": digest, "version": version}
    return observed


def _preflight_binding(review: dict) -> None:
    value = review.get("preflight")
    expected = {name: review["tools"][name] for name in ("mysql", "aws", "rustfs", "redis_server")}
    seed = review["scopes"]["seed"]
    service_run = Path(seed["backend_dir"]) / ".local-tests/reference-fixture/service-run"
    rustfs = review["services"]["rustfs"]
    expected_scope = "services-" + seed["scope_id"]
    if (not isinstance(value, dict) or value.get("format_version") != 1
            or value.get("kind") != "reference-fixture-tool-preflight"
            or value.get("status") != "verified" or value.get("tools") != expected
            or rustfs.get("scope_id") != expected_scope
            or rustfs.get("process_receipt") != str(service_run / "rustfs/process.json")):
        raise ValueError("夹具审阅计划缺少当前工具预检收据")


def revalidate(backend: Path, review_path: Path, output: Path, run=subprocess.run) -> dict:
    """为旧的只读计划创建一份新的、经本机工具复核的不可变审阅收据。"""
    backend = backend.resolve(strict=True)
    review_file, review = _read(backend, review_path)
    observed = _preflight(review, run)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    if not output.parent.is_dir():
        raise ValueError("新审阅收据的父目录不存在")
    revised = copy.deepcopy(review)
    revised["tools"] = {**review["tools"], **observed}
    seed = revised["scopes"]["seed"]
    service_run = Path(seed["backend_dir"]) / ".local-tests/reference-fixture/service-run"
    revised["services"]["rustfs"].update(
        scope_id="services-" + seed["scope_id"], process_receipt=str(service_run / "rustfs/process.json"))
    revised["preflight"] = {"format_version": 1, "kind": "reference-fixture-tool-preflight", "status": "verified",
                            "supersedes": bound(review_file), "tools": copy.deepcopy(observed)}
    write_json(output, revised)
    return revised


def _secret(directory: Path, name: str) -> tuple[str, dict]:
    path = directory / name
    if linked(path) or not path.is_file():
        raise ValueError("隔离夹具秘密文件缺失或经过链接")
    value = path.read_text(encoding="utf-8").strip()
    if not value:
        raise ValueError("隔离夹具秘密文件不能为空")
    return value, bound(path)


def _environment(backend: Path, review: dict, fixture: dict, output: Path) -> tuple[dict, dict]:
    seed = review["scopes"]["seed"]
    execution = Path(fixture["paths"]["backend"])
    # Device 收据记录的是生成工作树的内容快照，不包含其后产生的忽略运行目录状态。
    if execution != Path(seed["backend_dir"]) or snapshot(execution)[0] != fixture["generated"]["backend"]:
        raise ValueError("seed Device 工作树与审阅计划或生成快照不一致")
    secrets = execution / ".local-tests/reference-fixture/secrets"
    database = {item["key"]: item for item in seed["databases"]}
    mysql_path = Path(database["shared-control"]["connection_file"])
    if mysql_path != secrets / "mysql-client.cnf" or linked(mysql_path) or not mysql_path.is_file():
        raise ValueError("seed MySQL 凭据路径不属于冻结工作树")
    parser = configparser.ConfigParser(interpolation=None)
    parser.read_string(mysql_path.read_text(encoding="utf-8"))
    if parser.sections() != ["client"] or set(parser["client"]) != {"host", "port", "user", "password", "ssl-mode"}:
        raise ValueError("seed MySQL 凭据格式无效")
    if any(item["connection_file"] != str(mysql_path) for item in database.values()):
        raise ValueError("seed 四个数据库必须使用同一冻结 MySQL 凭据")
    values, files = {}, {"mysql-client.cnf": bound(mysql_path)}
    for key, filename in (("APP_OBJECT_STORAGE_ACCESS_KEY", "rustfs-access-key.txt"),
                          ("APP_OBJECT_STORAGE_SECRET_KEY", "rustfs-secret-key.txt"),
                          ("APP_REDIS_PASSWORD", "redis-password.txt"),
                          ("RYFRAME_RESET_ADMIN_PASSWORD", "reset-admin-password.txt"),
                          ("RYFRAME_RESET_USER_PASSWORD", "reset-user-password.txt"),
                          ("APP_AUTH_JWT_SECRET", "jwt-secret.txt"),
                          ("APP_MONITOR_METRICS_BEARER_TOKEN", "metrics-token.txt")):
        value, descriptor = _secret(secrets, filename)
        values[key], files[filename] = value, descriptor
    targets = []
    for key in ("shared", "dedicated-a", "dedicated-b"):
        item = database[key]
        targets.append({"key": key, "kind": "mysql", "mode": item["mode"], "host": item["host"],
                        "port": item["port"], "database": item["database"], "username": parser["client"]["user"],
                        "password_env": "APP_DB_PASSWORD", "tls_mode": "disabled"})
    scope = seed["scope_id"]
    environment = {
        "APP_ENV": "test", "APP_SCOPE_ID": scope, "APP_CONFIG_DIR": str(execution / "config"),
        # 控制库使用应用配置的正式覆盖名；租户目标仍以独立的秘密环境变量引用同一凭据。
        "APP_DATABASE_HOST": parser["client"]["host"], "APP_DATABASE_PORT": parser["client"]["port"],
        "APP_DATABASE_NAME": database["shared-control"]["database"],
        "APP_DATABASE_USERNAME": parser["client"]["user"],
        "APP_DATABASE_PASSWORD": parser["client"]["password"], "APP_DATABASE_TLS_MODE": "disabled",
        "APP_DB_PASSWORD": parser["client"]["password"],
        "APP_TENANT_DATA_TARGETS": json.dumps(targets, separators=(",", ":")),
        "APP_OBJECT_STORAGE_BACKEND": "rustfs", "APP_OBJECT_STORAGE_ENDPOINT": seed["objects"]["endpoint"],
        "APP_OBJECT_STORAGE_REGION": seed["objects"]["region"], "APP_OBJECT_STORAGE_USE_SSL": "false",
        "APP_REDIS_HOST": "127.0.0.1", "APP_REDIS_PORT": str(urlsplit(seed["redis"]["url"]).port or 16390),
        "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false", "APP_JOBS_MODE": "external",
        "APP_JOBS_HEALTH_HOST": "127.0.0.1", "APP_JOBS_HEALTH_PORT": "19210",
        "APP_RESET_CREDENTIAL_VERSION": "fixture-v1",
        "APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY": f"ryframe:devex-fresh:{scope}:sentinel",
        "APP_RESET_LEGACY_MYSQL_EXCLUSIVE": "true", "APP_RESET_LEGACY_REDIS_EXCLUSIVE": "true",
        "APP_RESET_LEGACY_OBJECT_STORAGE_EXCLUSIVE": "true", "TEMP": str(output / "tmp"), "TMP": str(output / "tmp"),
        **values,
    }
    return environment, files


def plan(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path) -> dict:
    """验证 C52 无关的输入，并返回后续显式创建阶段应消费的不可变描述。"""
    backend = backend.resolve(strict=True)
    review_file, review = _read(backend, review_path)
    fixture_file, fixture = _read(backend, fixture_path)
    maintenance_file, maintenance = _read(backend, maintenance_path)
    validate_review(review)
    _fixture(fixture)
    _maintenance(maintenance)
    reference = review["reference"]
    if any(reference[side]["databases"] for side in ("source", "protected_target")):
        raise ValueError("新的隔离参考夹具不得读取或复用历史来源数据库")
    seed = review["scopes"]["seed"]
    databases = {item["key"]: item for item in seed["databases"]}
    if set(databases) != set(KEYS):
        raise ValueError("seed 侧必须精确声明四个数据库")
    names = [entry["database"].lower() for side in review["scopes"].values()
             for entry in side["databases"]]
    if len(names) != 12 or len(set(names)) != len(names):
        raise ValueError("三侧夹具数据库名称不得重叠")
    result = {
        "format_version": 1,
        "kind": "reference-fixture-environment-plan",
        "review": {**bound(review_file), "canonical_sha256": plan_hash(review)},
        "fixture": bound(fixture_file),
        "maintenance_build": bound(maintenance_file),
        "side": "seed",
        "scope_id": seed["scope_id"],
        "databases": [{"key": key, "database": databases[key]["database"],
                       "mode": KEYS[key][1]} for key in sorted(databases)],
        "object_endpoint": seed["objects"]["endpoint"],
        "redis_url": seed["redis"]["url"],
        "historical_data_used": False,
        "remote_writes": 0,
    }
    return {**result, "sha256": plan_hash(result)}


def prepare(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path, output: Path) -> dict:
    """显式准备冻结环境；只写本地私有环境和收据，绝不创建服务或业务资源。"""
    backend = backend.resolve(strict=True)
    result = plan(backend, review_path, fixture_path, maintenance_path)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    if not output.parent.is_dir():
        raise ValueError("夹具环境输出父目录不存在")
    review = read_json(Path(result["review"]["path"]))
    _preflight_binding(review)
    fixture = read_json(Path(result["fixture"]["path"]))
    maintenance_path = Path(result["maintenance_build"]["path"])
    execution = Path(fixture["paths"]["backend"])
    environment, secrets = _environment(backend, review, fixture, output)
    output.mkdir()
    (output / "tmp").mkdir()
    try:
        # 隔离业务配置，同时保留 Git、Cargo 等本机工具必需的系统环境。
        runtime_environment = configured(environment)
        with Environments(runtime_environment, runtime_environment).use("target"):
            configuration = configuration_digest(execution)
            maintenance = verify_tools(execution, maintenance_path)
        write_json(output / "environment.json", {"format_version": 1, "environment": environment})
        receipt = {"format_version": 1, "kind": "reference-fixture-environment", "status": "prepared",
                   "plan": result, "execution_backend": str(execution), "configuration_sha256": configuration,
                   "maintenance": bound(maintenance_path), "maintenance_source": maintenance["source"],
                   "secret_files": secrets, "environment_sha256": plan_hash(environment),
                   "services_started": False, "remote_writes": 0, "historical_data_used": False}
        write_json(output / "bootstrap.json", receipt)
        return receipt
    except BaseException:
        # 保留目录供定位，不把半成品误当作可执行环境。
        if not (output / "bootstrap.json").exists():
            write_json(output / "failed.json", {"status": "failed", "services_started": False, "remote_writes": 0})
        raise


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("plan", "prepare", "review"))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--review", type=Path, required=True)
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--maintenance-build", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation in ("plan", "prepare") and (args.fixture is None or args.maintenance_build is None):
        parser.error(f"{args.operation} 需要 --fixture 与 --maintenance-build")
    if args.operation == "plan":
        if args.output is not None or args.write:
            parser.error("plan 不接受 --output 或 --write")
        result = plan(args.backend_dir, args.review, args.fixture, args.maintenance_build)
    elif args.operation == "prepare":
        if args.output is None or not args.write:
            parser.error("prepare 需要 --output 与 --write")
        result = prepare(args.backend_dir, args.review, args.fixture, args.maintenance_build, args.output)
    else:
        if args.output is None or not args.write:
            parser.error("review 需要 --output 与 --write")
        result = revalidate(args.backend_dir, args.review, args.output)
    print(json.dumps({"status": result.get("status", "planned"), "services_started": result.get("services_started", False),
                      "remote_writes": result.get("remote_writes", 0)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
