"""为新的隔离参考夹具生成首代环境计划；plan 只读，不启动服务或创建资源。"""
from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import re
import secrets
import shutil
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
from restore_source_binding import mysql_client
from source_inventory import snapshot


SECRET_FILES = ("mysql-client.cnf", "rustfs-access-key.txt", "rustfs-secret-key.txt", "redis-password.txt",
                "reset-admin-password.txt", "reset-user-password.txt", "jwt-secret.txt", "metrics-token.txt")
RESET_SECRET_FILES = {"reset-admin-password.txt", "reset-user-password.txt"}


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
    wsl_value = shutil.which("wsl")
    if wsl_value is None:
        raise ValueError("本机缺少 WSL 启动器，无法预检 Redis 控制器")
    wsl = Path(wsl_value).resolve(strict=True)
    if linked(wsl) or not wsl.is_file():
        raise ValueError("本机 WSL 启动器不是受控普通文件")
    prefix = [str(wsl), "--distribution", distribution, "--exec"]
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
    python = "/usr/bin/python3"
    python_resolved = _command(run, [*prefix, "/usr/bin/readlink", "-f", python])
    python_digest = _command(run, [*prefix, "/usr/bin/sha256sum", python_resolved]).split(maxsplit=1)[0]
    if (not re.fullmatch(r"/usr/bin/python3\.[0-9]+", python_resolved)
            or not re.fullmatch(r"[a-f0-9]{64}", python_digest)):
        raise ValueError("本机 WSL Python 未通过只读核验")
    observed["wsl"] = {"path": str(wsl), "sha256": file_digest(wsl)["sha256"]}
    observed["redis_python"] = {"distribution": distribution, "path": python,
                                 "resolved_path": python_resolved, "sha256": python_digest}
    return observed


def _preflight_binding(review: dict) -> None:
    value = review.get("preflight")
    required = ("mysql", "aws", "rustfs", "redis_server", "wsl", "redis_python")
    if not all(name in review.get("tools", {}) for name in required):
        raise ValueError("夹具审阅计划缺少当前工具预检收据")
    expected = {name: review["tools"][name] for name in required}
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


def _secret_directory(backend: Path, execution: Path, selected: Path | None = None) -> Path:
    root = execution / ".local-tests/reference-fixture"
    directory = root / "secrets" if selected is None else local_path(backend, str(selected))
    if (not directory.is_relative_to(root) or linked(directory) or not directory.is_dir()
            or any(linked(directory / name) or not (directory / name).is_file() for name in SECRET_FILES)):
        raise ValueError("夹具 secret set 必须是执行工作树内完整的普通文件目录")
    if directory != root / "secrets":
        manifest = directory / "secret-set.json"
        value = read_json(manifest)
        exact = {"format_version", "kind", "status", "source", "files"}
        if (set(value) != exact or value["format_version"] != 1 or value["kind"] != "reference-fixture-secret-set"
                or value["status"] != "generated" or set(value["files"]) != set(SECRET_FILES)
                or any(value["files"][name] != bound(directory / name) for name in SECRET_FILES)):
            raise ValueError("夹具生成 secret set 收据无效")
    return directory


def _fixture_execution(backend: Path, fixture_path: Path) -> tuple[Path, dict, dict]:
    """读取并核验一个 Device 收据及其生成后的后端工作树。"""
    receipt_path, fixture = _read(backend, fixture_path)
    _fixture(fixture)
    execution = Path(fixture["paths"]["backend"])
    generated = fixture.get("generated", {}).get("backend")
    if (not isinstance(generated, dict) or not execution.is_dir()
            or snapshot(execution)[0] != generated):
        raise ValueError("Device 工作树与生成快照不一致")
    return receipt_path, fixture, generated


def bootstrap_secrets(backend: Path, source_fixture_path: Path, source_directory: Path,
                      target_fixture_path: Path) -> dict:
    """将已登记的秘密集复制到新 Device；只写新工作树内的私有文件。"""
    backend = backend.resolve(strict=True)
    source_receipt, source_fixture, _ = _fixture_execution(backend, source_fixture_path)
    target_receipt, target_fixture, _ = _fixture_execution(backend, target_fixture_path)
    source_execution = Path(source_fixture["paths"]["backend"])
    target_execution = Path(target_fixture["paths"]["backend"])
    if source_execution == target_execution:
        raise ValueError("秘密导入必须在两个不同的 Device 工作树之间进行")
    source = _secret_directory(backend, source_execution, source_directory)
    destination = target_execution / ".local-tests/reference-fixture/secrets"
    if destination.exists() or linked(destination) or not destination.parent.is_dir():
        raise ValueError("新 Device 默认秘密目录已存在或父目录无效")
    destination.mkdir()
    try:
        for name in SECRET_FILES:
            target = destination / name
            with target.open("xb") as stream:
                stream.write((source / name).read_bytes())
                stream.flush()
                os.fsync(stream.fileno())
        receipt = {
            "format_version": 1,
            "kind": "reference-fixture-secret-bootstrap",
            "status": "imported",
            "source_fixture": bound(source_receipt),
            "source_files": {name: bound(source / name) for name in SECRET_FILES},
            "target_fixture": bound(target_receipt),
            "target_files": {name: bound(destination / name) for name in SECRET_FILES},
            "remote_writes": 0,
            "services_started": False,
        }
        write_json(destination / "bootstrap.json", receipt)
        return receipt
    except BaseException:
        if not (destination / "bootstrap.json").exists():
            write_json(destination / "failed.json", {"status": "failed", "remote_writes": 0,
                                                       "services_started": False})
        raise


def _reset_password() -> str:
    return "Aa1!" + secrets.token_urlsafe(32)


def rotate_secrets(backend: Path, fixture_path: Path, output: Path) -> dict:
    """生成新的夹具 secret set；只写隔离文件，不修改冻结来源或任何远程资源。"""
    backend = backend.resolve(strict=True)
    _, fixture = _read(backend, fixture_path)
    _fixture(fixture)
    execution = Path(fixture["paths"]["backend"])
    generated = fixture.get("generated", {}).get("backend")
    if execution != Path(fixture["paths"]["backend"]) or snapshot(execution)[0] != generated:
        raise ValueError("Device 执行工作树与生成快照不一致")
    source = _secret_directory(backend, execution)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    root = execution / ".local-tests/reference-fixture"
    if not output.parent.is_dir() or not output.is_relative_to(root):
        raise ValueError("新的夹具 secret set 必须写入执行工作树的参考夹具目录")
    output.mkdir()
    try:
        for name in SECRET_FILES:
            target = output / name
            if name in RESET_SECRET_FILES:
                value = _reset_password().encode("utf-8") + b"\n"
            else:
                value = (source / name).read_bytes()
            with target.open("xb") as stream:
                stream.write(value)
                stream.flush()
                os.fsync(stream.fileno())
        if (output / "reset-admin-password.txt").read_bytes() == (output / "reset-user-password.txt").read_bytes():
            raise ValueError("夹具 reset 密码不得相同")
        values = {key: _secret(output, name)[0] for key, name in (
            ("RYFRAME_RESET_ADMIN_PASSWORD", "reset-admin-password.txt"),
            ("RYFRAME_RESET_USER_PASSWORD", "reset-user-password.txt"),
        )}
        _validate_reset_passwords(values)
        result = {"format_version": 1, "kind": "reference-fixture-secret-set", "status": "generated",
                  "source": {name: bound(source / name) for name in SECRET_FILES},
                  "files": {name: bound(output / name) for name in SECRET_FILES}}
        write_json(output / "secret-set.json", result)
        return result
    except BaseException:
        if not (output / "secret-set.json").exists():
            write_json(output / "failed.json", {"status": "failed"})
        raise


def _database_credentials(backend: Path, source: Path, destination: Path) -> dict:
    source = local_path(backend, str(source))
    destination = local_path(backend, str(destination))
    if linked(source) or not source.is_file():
        raise ValueError("冻结 Device MySQL 凭据缺失或经过链接")
    if destination == source:
        return bound(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if linked(destination) or destination.exists() and not destination.is_file():
        raise ValueError("目标 MySQL 凭据路径无效")
    if destination.exists():
        if destination.read_bytes() != source.read_bytes():
            raise ValueError("目标 MySQL 凭据已存在且不属于冻结 Device 来源")
    else:
        with destination.open("xb") as stream:
            stream.write(source.read_bytes())
            stream.flush()
            os.fsync(stream.fileno())
    return bound(destination)


def _validate_reset_passwords(values: dict) -> None:
    """在创建任何 fresh 资源前复现后端 reset 的密码复杂度门槛。"""
    for key in ("RYFRAME_RESET_ADMIN_PASSWORD", "RYFRAME_RESET_USER_PASSWORD"):
        value = values.get(key)
        if (not isinstance(value, str) or not re.fullmatch(r"[!-~]{8,72}", value)
                or not any(char.isascii() and char.isupper() for char in value)
                or not any(char.isascii() and char.islower() for char in value)
                or not any(char.isascii() and char.isdigit() for char in value)
                or not any(char.isascii() and not char.isalnum() for char in value)):
            raise ValueError(f"{key} 不满足 reset 种子密码复杂度策略")


def _environment(backend: Path, review: dict, fixture: dict, output: Path, side: str = "seed",
                 secret_directory: Path | None = None) -> tuple[dict, dict]:
    if side not in review["scopes"]:
        raise ValueError("夹具环境必须选择已审阅侧")
    selected = review["scopes"][side]
    execution = Path(fixture["paths"]["backend"])
    # Device 收据记录的是生成工作树的内容快照，不包含其后产生的忽略运行目录状态。
    if execution != Path(selected["backend_dir"]) or snapshot(execution)[0] != fixture["generated"]["backend"]:
        raise ValueError("所选侧 Device 工作树与审阅计划或生成快照不一致")
    source_secrets = _secret_directory(backend, execution)
    secrets = _secret_directory(backend, execution, secret_directory)
    source_mysql = source_secrets / "mysql-client.cnf"
    if source_mysql != source_secrets / "mysql-client.cnf" or linked(source_mysql) or not source_mysql.is_file():
        raise ValueError("Device MySQL 凭据路径不属于冻结工作树")
    database = {item["key"]: item for item in selected["databases"]}
    mysql_path = Path(database["shared-control"]["connection_file"])
    if any(item["connection_file"] != str(mysql_path) for item in database.values()):
        raise ValueError("同侧四个数据库必须使用同一冻结 MySQL 凭据")
    mysql_descriptor = _database_credentials(backend, source_mysql, mysql_path)
    try:
        client = mysql_client(mysql_path, mysql_descriptor["sha256"])
    except ValueError as error:
        raise ValueError("MySQL 凭据格式无效") from error
    tls_mode = client["ssl-mode"].lower()
    values, files = {}, {"mysql-client.cnf": mysql_descriptor}
    for key, filename in (("APP_OBJECT_STORAGE_ACCESS_KEY", "rustfs-access-key.txt"),
                          ("APP_OBJECT_STORAGE_SECRET_KEY", "rustfs-secret-key.txt"),
                          ("APP_REDIS_PASSWORD", "redis-password.txt"),
                          ("RYFRAME_RESET_ADMIN_PASSWORD", "reset-admin-password.txt"),
                          ("RYFRAME_RESET_USER_PASSWORD", "reset-user-password.txt"),
                          ("APP_AUTH_JWT_SECRET", "jwt-secret.txt"),
                          ("APP_MONITOR_METRICS_BEARER_TOKEN", "metrics-token.txt")):
        value, descriptor = _secret(secrets, filename)
        values[key], files[filename] = value, descriptor
    _validate_reset_passwords(values)
    targets = []
    for key in ("shared", "dedicated-a", "dedicated-b"):
        item = database[key]
        targets.append({"key": key, "kind": "mysql", "mode": item["mode"], "host": item["host"],
                        "port": item["port"], "database": item["database"], "username": client["user"],
                        "password_env": "APP_DB_PASSWORD", "tls_mode": tls_mode})
    scope = selected["scope_id"]
    api = urlsplit(selected["api_url"])
    worker = urlsplit(selected["worker_ready_url"])
    if (api.hostname not in ("127.0.0.1", "::1") or api.port is None
            or worker.hostname not in ("127.0.0.1", "::1") or worker.port is None):
        raise ValueError("夹具 API 或 Worker 地址必须是明确本机端口")
    environment = {
        "APP_ENV": "test", "APP_SCOPE_ID": scope, "APP_CONFIG_DIR": str(execution / "config"),
        "APP_APP_HOST": api.hostname, "APP_APP_PORT": str(api.port),
        # 控制库使用应用配置的正式覆盖名；租户目标仍以独立的秘密环境变量引用同一凭据。
        "APP_DATABASE_HOST": client["host"], "APP_DATABASE_PORT": client["port"],
        "APP_DATABASE_NAME": database["shared-control"]["database"],
        "APP_DATABASE_USERNAME": client["user"],
        "APP_DATABASE_PASSWORD": client["password"], "APP_DATABASE_TLS_MODE": tls_mode,
        "APP_DB_PASSWORD": client["password"],
        "APP_TENANT_DATA_TARGETS": json.dumps(targets, separators=(",", ":")),
        "APP_OBJECT_STORAGE_BACKEND": "rustfs", "APP_OBJECT_STORAGE_ENDPOINT": selected["objects"]["endpoint"],
        "APP_OBJECT_STORAGE_REGION": selected["objects"]["region"], "APP_OBJECT_STORAGE_USE_SSL": "false",
        "APP_REDIS_HOST": "127.0.0.1", "APP_REDIS_PORT": str(urlsplit(selected["redis"]["url"]).port or 16390),
        "APP_REDIS_DATABASE": "0", "APP_REDIS_TLS": "false", "APP_JOBS_MODE": "external",
        "APP_JOBS_HEALTH_HOST": worker.hostname, "APP_JOBS_HEALTH_PORT": str(worker.port),
        "APP_RESET_CREDENTIAL_VERSION": "fixture-v1",
        "APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY": f"ryframe:devex-fresh:{scope}:sentinel",
        "APP_RESET_LEGACY_MYSQL_EXCLUSIVE": "true", "APP_RESET_LEGACY_REDIS_EXCLUSIVE": "true",
        "APP_RESET_LEGACY_OBJECT_STORAGE_EXCLUSIVE": "true", "TEMP": str(output / "tmp"), "TMP": str(output / "tmp"),
        **values,
    }
    return environment, files


def plan(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path, side: str = "seed") -> dict:
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
    if side not in review["scopes"]:
        raise ValueError("夹具环境必须选择已审阅侧")
    selected = review["scopes"][side]
    databases = {item["key"]: item for item in selected["databases"]}
    if set(databases) != set(KEYS):
        raise ValueError("夹具侧必须精确声明四个数据库")
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
        "side": side,
        "scope_id": selected["scope_id"],
        "databases": [{"key": key, "database": databases[key]["database"],
                       "mode": KEYS[key][1]} for key in sorted(databases)],
        "object_endpoint": selected["objects"]["endpoint"],
        "redis_url": selected["redis"]["url"],
        "historical_data_used": False,
        "remote_writes": 0,
    }
    return {**result, "sha256": plan_hash(result)}


def prepare(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path, output: Path,
            side: str = "seed", secret_directory: Path | None = None) -> dict:
    """显式准备冻结环境；只写本地私有环境和收据，绝不创建服务或业务资源。"""
    backend = backend.resolve(strict=True)
    result = plan(backend, review_path, fixture_path, maintenance_path, side)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    if not output.parent.is_dir():
        raise ValueError("夹具环境输出父目录不存在")
    review = read_json(Path(result["review"]["path"]))
    _preflight_binding(review)
    fixture = read_json(Path(result["fixture"]["path"]))
    maintenance_path = Path(result["maintenance_build"]["path"])
    execution = Path(fixture["paths"]["backend"])
    environment, secrets = _environment(backend, review, fixture, output, side, secret_directory)
    output.mkdir()
    (output / "tmp").mkdir()
    try:
        # 隔离业务配置，同时保留 Git、Cargo 等本机工具必需的系统环境。
        runtime_environment = configured(environment)
        with Environments(runtime_environment, runtime_environment).use("target"):
            configuration = configuration_digest(execution)
            maintenance = verify_tools(execution, maintenance_path)
        # fresh-target 只接受这一层私有环境；版本和生命周期信息属于相邻 bootstrap 收据。
        write_json(output / "environment.json", {"environment": environment})
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
    parser.add_argument("operation", choices=("plan", "prepare", "review", "rotate-secrets", "bootstrap-secrets"))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--source-fixture", type=Path)
    parser.add_argument("--source-secrets", type=Path)
    parser.add_argument("--review", type=Path)
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--maintenance-build", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--secrets-dir", type=Path)
    parser.add_argument("--side", choices=("seed", "base", "candidate"), default="seed")
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation in ("plan", "prepare") and (args.review is None or args.fixture is None or args.maintenance_build is None):
        parser.error(f"{args.operation} 需要 --review、--fixture 与 --maintenance-build")
    if args.operation == "plan":
        if args.output is not None or args.write or args.secrets_dir is not None:
            parser.error("plan 不接受 --output、--write 或 --secrets-dir")
        result = plan(args.backend_dir, args.review, args.fixture, args.maintenance_build, args.side)
    elif args.operation == "prepare":
        if args.output is None or not args.write:
            parser.error("prepare 需要 --output 与 --write")
        result = prepare(args.backend_dir, args.review, args.fixture, args.maintenance_build, args.output, args.side,
                         args.secrets_dir)
    elif args.operation == "rotate-secrets":
        if args.fixture is None or args.output is None or not args.write:
            parser.error("rotate-secrets 需要 --fixture、--output 与 --write")
        if args.review is not None or args.maintenance_build is not None or args.secrets_dir is not None:
            parser.error("rotate-secrets 不接受 --review、--maintenance-build 或 --secrets-dir")
        result = rotate_secrets(args.backend_dir, args.fixture, args.output)
    elif args.operation == "bootstrap-secrets":
        if (args.source_fixture is None or args.source_secrets is None or args.fixture is None or not args.write
                or any(value is not None for value in (args.review, args.maintenance_build, args.output, args.secrets_dir))):
            parser.error("bootstrap-secrets 需要 --source-fixture、--source-secrets、--fixture 与 --write")
        result = bootstrap_secrets(args.backend_dir, args.source_fixture, args.source_secrets, args.fixture)
    else:
        if args.review is None or args.output is None or not args.write or args.secrets_dir is not None:
            parser.error("review 需要 --review、--output 与 --write，且不接受 --secrets-dir")
        result = revalidate(args.backend_dir, args.review, args.output)
    print(json.dumps({"status": result.get("status", "planned"), "services_started": result.get("services_started", False),
                      "remote_writes": result.get("remote_writes", 0)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
