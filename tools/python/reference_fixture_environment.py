"""为新的隔离参考夹具生成首代环境计划；plan 只读，不启动服务或创建资源。"""
from __future__ import annotations

import argparse
import copy
import json
import os
from pathlib import Path
import shutil
import subprocess

from devex_clone_capture import read_json, write_json
from process_environment import Environments, configured
from devex_clone_model import linked, local_path
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest
from reference_fixture_service_context import load_prepared_environment
from reference_fixture_control_protocol import run_private
from restore_build import file_digest
from restore_reference_plan import plan_hash
from native_s3_client import _invoke as native_invoke, _read_response as native_response
from reference_fixture_environment_native import invoke as environment_native


SECRET_FILES = ("mysql-client.cnf", "rustfs-access-key.txt", "rustfs-secret-key.txt", "redis-password.txt",
                "reset-admin-password.txt", "jwt-secret.txt", "metrics-token.txt")
# 完整浏览器验收单 IP 的请求量高于产品默认限流（100 次/60s）；夹具只提高请求容量，
# 窗口与 login 等 api_limits 保持基础配置，通过 app.test.toml 深度合并生效。
FIXTURE_REQUEST_RATE_LIMIT_CAPACITY = 1000


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def prepared_environment(backend: Path, value: Path) -> tuple[Path, Path, dict, dict]:
    """读取并复核唯一环境 bootstrap、私有环境摘要及全部秘密文件。"""
    return load_prepared_environment(backend, value, SECRET_FILES)


TEST_CONFIG_DIRECTORY = "config"


def _test_configuration(execution: Path, output: Path, targets: list[dict], settings: dict,
                        database_credentials: dict) -> Path:
    """生成测试运行配置目录：基础 app.toml 复制，租户目标写入 app.test.toml。

    调用方显式绑定生成目录读取已登记目标；冻结的夹具执行树
    不能被写入，因此配置目录必须随环境一起生成。
    """
    source = execution / "config" / "app.toml"
    if linked(source) or not source.is_file():
        raise ValueError("夹具执行树缺少基础 app.toml")
    if not targets:
        raise ValueError("夹具测试配置必须声明至少一个租户数据目标")
    directory = output / TEST_CONFIG_DIRECTORY
    if directory.exists() or linked(directory):
        raise ValueError("夹具测试配置目录已存在，拒绝覆盖")
    output.mkdir(parents=True, exist_ok=True)
    from full_stack_ci_support import configuration_directory
    configuration_directory(directory)
    shutil.copyfile(source, directory / "app.toml")
    (directory / "app.toml").chmod(0o600)
    text = environment_native("configuration-text", {"settings": settings, "targets": targets})["text"]
    native_response(*native_invoke(
        "__source-evidence", {"operation": "database-configuration", "write": True,
            "arguments": {**database_credentials, "output": str(directory / "app.test.toml"),
                          "text": text}},
        subprocess.run, may_write=True,
    ))
    (directory / "app.test.toml").chmod(0o600)
    return directory


def _maintenance(value: dict) -> None:
    environment_native("maintenance", value)


def _preflight_binding(review: dict) -> dict:
    return environment_native("preflight-binding", review)


def _preflight(review: dict) -> dict:
    from devex_clone_cache_native import invoke
    return invoke("preflight-tools", review=review)


def validate_current_review_tools(review: dict) -> dict:
    """重新观察 ready 审阅绑定的工具，并拒绝本机工具代次漂移。"""
    expected = _preflight_binding(review)
    observed = _preflight(review)
    if observed != expected:
        raise ValueError("夹具审阅计划中的当前工具已经变化")
    return observed


def validated_recorded_review_tools(review: dict) -> dict:
    """只验证已发布预检收据的冻结工具，不声称当前本机工具仍可用。"""
    return copy.deepcopy(_preflight_binding(review))


def validate_preflight_successor(previous: dict, review: dict) -> None:
    environment_native("preflight-successor", {"previous": previous, "review": review})


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
    requested = selected if selected is None or selected.is_absolute() else backend / selected
    directory = root / "secrets" if requested is None else local_path(backend, str(requested))
    if (not directory.is_relative_to(root) or linked(directory) or not directory.is_dir()
            or any(linked(directory / name) or not (directory / name).is_file() for name in SECRET_FILES)):
        raise ValueError("夹具 secret set 必须是执行工作树内完整的普通文件目录")
    if (directory / "failed.json").exists() or linked(directory / "failed.json"):
        raise ValueError("夹具秘密目录含失败标记，拒绝消费半成品")
    if directory != root / "secrets":
        manifest = directory / "secret-set.json"
        value = read_json(manifest)
        exact = {"format_version", "kind", "status", "source", "files"}
        if (set(value) != exact or value["format_version"] != 1 or value["kind"] != "reference-fixture-secret-set"
                or value["status"] != "generated" or set(value["files"]) != set(SECRET_FILES)
                or any(value["files"][name] != bound(directory / name) for name in SECRET_FILES)):
            raise ValueError("夹具生成 secret set 收据无效")
    return directory


def _database_credentials(backend: Path, source: Path, destination: Path) -> dict:
    source = local_path(backend, str(source))
    destination = local_path(backend, str(destination))
    if linked(source) or not source.is_file():
        raise ValueError("冻结 业务 crate MySQL 凭据缺失或经过链接")
    if destination == source:
        return bound(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if linked(destination) or destination.exists() and not destination.is_file():
        raise ValueError("目标 MySQL 凭据路径无效")
    if destination.exists():
        if destination.read_bytes() != source.read_bytes():
            raise ValueError("目标 MySQL 凭据已存在且不属于冻结 业务 crate 来源")
    else:
        with destination.open("xb") as stream:
            stream.write(source.read_bytes())
            stream.flush()
            os.fsync(stream.fileno())
    return bound(destination)


def _validate_reset_password(values: dict) -> None:
    environment_native("reset-password", {"admin_password": values.get("admin_password")})


def _environment(backend: Path, review: dict, publication: dict, output: Path, side: str = "seed",
                 secret_directory: Path | None = None) -> tuple[dict, dict]:
    if side not in review["scopes"]:
        raise ValueError("夹具环境必须选择已审阅侧")
    selected = review["scopes"][side]
    # 完整发布来源与所选侧已由原生 plan/planned-inputs 前后核验。
    execution = Path(publication["published"]["backend"]["path"])
    source_backend = Path(publication["original"]["paths"]["backend"])
    source_secrets = _secret_directory(backend, source_backend)
    secrets = _secret_directory(backend, source_backend, secret_directory)
    source_mysql = source_secrets / "mysql-client.cnf"
    if source_mysql != source_secrets / "mysql-client.cnf" or linked(source_mysql) or not source_mysql.is_file():
        raise ValueError("业务 crate MySQL 凭据路径不属于冻结工作树")
    database = {item["key"]: item for item in selected["databases"]}
    mysql_path = Path(database["shared-control"]["connection_file"])
    if any(item["connection_file"] != str(mysql_path) for item in database.values()):
        raise ValueError("同侧四个数据库必须使用同一冻结 MySQL 凭据")
    mysql_descriptor = _database_credentials(backend, source_mysql, mysql_path)
    try:
        database_credentials = {"backend": str(backend), "database": {
            "defaults_file": str(mysql_path), "defaults_sha256": mysql_descriptor["sha256"],
            "database": database["shared-control"]["database"]}}
        client = native_response(*native_invoke(
            "__source-evidence", {"operation": "connection-summary", "write": False,
                                  "arguments": database_credentials},
            subprocess.run, may_write=False,
        ))[0]["value"]
    except ValueError as error:
        raise ValueError("MySQL 凭据格式无效") from error
    values, files = {}, {"mysql-client.cnf": mysql_descriptor}
    for key, filename in (("access_key", "rustfs-access-key.txt"),
                          ("secret_key", "rustfs-secret-key.txt"),
                          ("redis_password", "redis-password.txt"),
                          ("admin_password", "reset-admin-password.txt"),
                          ("jwt_secret", "jwt-secret.txt"),
                          ("metrics_token", "metrics-token.txt")):
        value, descriptor = _secret(secrets, filename)
        values[key], files[filename] = value, descriptor
    _validate_reset_password(values)
    configuration = environment_native("configuration-settings", {
        "selected": selected, "client": client, "values": values, "side": side})
    targets, settings = configuration["targets"], configuration["settings"]
    environment = {"APP_ENV": "test", "TEMP": str(output / "tmp"), "TMP": str(output / "tmp")}
    _test_configuration(execution, output, targets, settings, database_credentials)
    credentials = output / "administrator.toml"
    text = environment_native("administrator-credentials", {"admin_password": values["admin_password"]})["text"]
    with credentials.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(text)
        stream.flush()
        os.fsync(stream.fileno())
    credentials.chmod(0o600)
    return environment, files


def plan(backend: Path, review_path: Path, execution_source_path: Path, maintenance_path: Path, side: str = "seed") -> dict:
    return environment_native("plan", {"backend": str(backend.resolve(strict=True)),
        "review": str(review_path), "execution_source": str(execution_source_path),
        "maintenance_build": str(maintenance_path), "side": side})


def _planned_inputs(backend: Path, result: dict) -> tuple[Path, dict, Path, dict, Path, dict]:
    values = environment_native("planned-inputs", {"backend": str(backend), "plan": result})["inputs"]
    return tuple(Path(value) if index % 2 == 0 else value for index, value in enumerate(values))


def prepare(backend: Path, review_path: Path, execution_source_path: Path, maintenance_path: Path, output: Path,
            side: str = "seed", secret_directory: Path | None = None) -> dict:
    """显式准备冻结环境；只写本地私有环境和收据，绝不创建服务或业务资源。"""
    backend = backend.resolve(strict=True)
    result = plan(backend, review_path, execution_source_path, maintenance_path, side)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    if not output.parent.is_dir():
        raise ValueError("夹具环境输出父目录不存在")
    inputs = _planned_inputs(backend, result)
    _, review, _, publication, maintenance_file, _ = inputs
    execution = Path(publication["published"]["backend"]["path"])
    output = Path(environment_native("prepare-output", {"backend": str(backend), "plan": result, "output": str(output)})["output"])
    environment, secrets = _environment(backend, review, publication, output, side, secret_directory)
    # 测试配置目录随环境一起生成，因此输出目录可能已经存在。
    output.mkdir(exist_ok=True)
    (output / "tmp").mkdir()
    try:
        # 隔离业务配置，同时保留 Git、Cargo 等本机工具必需的系统环境。
        runtime_environment = configured(environment)
        with Environments(runtime_environment, runtime_environment).use("target"):
            configuration = configuration_digest(execution, output / TEST_CONFIG_DIRECTORY)
            maintenance = verify_tools(execution, maintenance_file)
        if _planned_inputs(backend, result) != inputs:
            raise ValueError("夹具环境计划输入在准备期间发生变化")
        # fresh-target 只接受这一层私有环境；版本和生命周期信息属于相邻 bootstrap 收据。
        write_json(output / "environment.json", {"environment": environment})
        receipt = {"format_version": 1, "kind": "reference-fixture-environment", "status": "prepared",
                   "plan": result, "execution_backend": str(execution), "configuration_sha256": configuration,
                   "config_dir": str(output / TEST_CONFIG_DIRECTORY),
                   "administrator_credentials": bound(output / "administrator.toml"),
                   "maintenance": bound(maintenance_file), "maintenance_source": maintenance["source"],
                   "secret_files": secrets, "environment_sha256": plan_hash(environment),
                   "services_started": False, "remote_writes": 0, "historical_data_used": False}
        write_json(output / "bootstrap.json", receipt)
        return receipt
    except BaseException:
        # 保留目录供定位，不把半成品误当作可执行环境。
        if not (output / "bootstrap.json").exists():
            write_json(output / "failed.json", {"status": "failed", "services_started": False, "remote_writes": 0})
        raise


PROTOCOL_SCHEMAS = {
    "prepare": (("review", "execution_source", "maintenance_build", "side", "output", "native_tool"), ("secrets_dir",), True),
}


def main(argv: list[str]) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("prepare",))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--review", type=Path)
    parser.add_argument("--execution-source", type=Path)
    parser.add_argument("--maintenance-build", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--secrets-dir", type=Path)
    parser.add_argument("--side", choices=("seed", "base", "candidate"), default="seed")
    parser.add_argument("--native-tool", required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args(argv)
    from rate_limit_tool import install_verified_binding
    install_verified_binding(args.native_tool)
    if args.review is None or args.execution_source is None or args.maintenance_build is None:
        parser.error("prepare 需要 --review、--execution-source 与 --maintenance-build")
    if args.output is None or not args.write:
        parser.error("prepare 需要 --output 与 --write")
    result = prepare(args.backend_dir, args.review, args.execution_source, args.maintenance_build, args.output, args.side,
                     args.secrets_dir)
    print(json.dumps({"status": result.get("status", "planned"), "services_started": result.get("services_started", False),
                      "remote_writes": result.get("remote_writes", 0)}, ensure_ascii=False))


if __name__ == "__main__":
    raise SystemExit(run_private("environment", PROTOCOL_SCHEMAS, main, positional_operation=True))
