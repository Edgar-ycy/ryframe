"""为新的隔离参考夹具生成首代环境计划；plan 只读，不启动服务或创建资源。"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess

from devex_clone_capture import read_json, write_json
from process_environment import Environments, configured
from devex_clone_model import linked, local_path
from devex_clone_target_binding import REVIEW_FILE_TOOLS, validate_review
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest
from reference_fixture_service_context import load_prepared_environment, service_run
from reference_fixture_control_protocol import run_private
from restore_build import file_digest
from restore_reference_plan import plan_hash
from native_s3_client import _invoke as native_invoke, _read_response as native_response
from source_inventory import snapshot
from reference_fixture_environment_native import invoke as environment_native


SECRET_FILES = ("mysql-client.cnf", "rustfs-access-key.txt", "rustfs-secret-key.txt", "redis-password.txt",
                "reset-admin-password.txt", "jwt-secret.txt", "metrics-token.txt")
# 完整浏览器验收单 IP 的请求量高于产品默认限流（100 次/60s）；夹具只提高请求容量，
# 窗口与 login 等 api_limits 保持基础配置，通过 app.test.toml 深度合并生效。
FIXTURE_REQUEST_RATE_LIMIT_CAPACITY = 1000


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("夹具输入必须是受控目录中的普通文件")
    return path, read_json(path)


def prepared_environment(backend: Path, value: Path) -> tuple[Path, Path, dict, dict]:
    """读取并复核唯一环境 bootstrap、私有环境摘要及全部秘密文件。"""
    return load_prepared_environment(backend, value, SECRET_FILES)


def _fixture(value: dict) -> None:
    environment_native("fixture", value)


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


def _command(run, arguments: list[str]) -> str:
    completed = run(arguments, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return completed.stdout.decode("utf-8").strip()


def _preflight_python(review: dict, run=subprocess.run) -> dict:
    """重新读取计划绑定的工具，不启动服务、不连接业务资源。"""
    if type(review.get("ready_for_execution")) is not bool:
        raise ValueError("夹具审阅计划就绪状态无效")
    structural = copy.deepcopy(review)
    structural["ready_for_execution"] = True
    validate_review(structural)
    tools = review["tools"]
    observed = {}
    for name in REVIEW_FILE_TOOLS:
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


def _preflight_binding(review: dict) -> dict:
    return environment_native("preflight-binding", review)


def _preflight(review: dict, run=subprocess.run) -> dict:
    from devex_clone_cache_native import invoke
    return invoke("preflight-tools", review=review)


def validate_current_review_tools(review: dict, run=subprocess.run) -> dict:
    """重新观察 ready 审阅绑定的工具，并拒绝本机工具代次漂移。"""
    expected = _preflight_binding(review)
    observed = _preflight(review, run)
    if observed != expected:
        raise ValueError("夹具审阅计划中的当前工具已经变化")
    return observed


def validated_recorded_review_tools(review: dict) -> dict:
    """只验证已发布预检收据的冻结工具，不声称当前本机工具仍可用。"""
    return copy.deepcopy(_preflight_binding(review))


def validate_preflight_successor(previous: dict, review: dict) -> None:
    environment_native("preflight-successor", {"previous": previous, "review": review})


def revalidate(backend: Path, review_path: Path, output: Path, run=subprocess.run) -> dict:
    """为旧的只读计划创建一份新的、经本机工具复核的不可变审阅收据。"""
    backend = backend.resolve(strict=True)
    review_file, review = _read(backend, review_path)
    predecessor = bound(review_file)
    observed = _preflight(review, run)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    if not output.parent.is_dir():
        raise ValueError("新审阅收据的父目录不存在")
    current_file, current_review = _read(backend, review_file)
    if current_file != review_file or current_review != review or bound(current_file) != predecessor:
        raise ValueError("审阅 predecessor 在工具预检期间发生变化")
    revised = copy.deepcopy(review)
    revised["ready_for_execution"] = True
    revised["tools"] = {**review["tools"], **observed}
    revised["tools"].pop("redis_python", None)
    seed = revised["scopes"]["seed"]
    service_directory = service_run(revised)
    revised["services"]["rustfs"].update(
        scope_id="services-" + seed["scope_id"], process_receipt=str(service_directory / "rustfs/process.json"))
    revised["preflight"] = {"format_version": 1, "kind": "reference-fixture-tool-preflight", "status": "verified",
                            "supersedes": predecessor, "tools": copy.deepcopy(observed)}
    terminal = _preflight(current_review, run)
    if terminal != observed or bound(review_file) != predecessor or read_json(review_file) != review:
        raise ValueError("审阅 predecessor 或当前工具在 ready 签发前发生变化")
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
    requested = selected if selected is None or selected.is_absolute() else backend / selected
    directory = root / "secrets" if requested is None else local_path(backend, str(requested))
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
    """读取并核验一个 业务 crate 收据及其生成后的后端工作树。"""
    receipt_path, fixture = _read(backend, fixture_path)
    _fixture(fixture)
    execution = Path(fixture["paths"]["backend"])
    generated = fixture.get("generated", {}).get("backend")
    if (not isinstance(generated, dict) or not execution.is_dir()
            or snapshot(execution)[0] != generated):
        raise ValueError("业务 crate 工作树与生成快照不一致")
    return receipt_path, fixture, generated


def bootstrap_secrets(backend: Path, source_fixture_path: Path, source_directory: Path,
                      target_fixture_path: Path) -> dict:
    """将已登记的秘密集复制到新 业务 crate；只写新工作树内的私有文件。"""
    backend = backend.resolve(strict=True)
    source_receipt, source_fixture, _ = _fixture_execution(backend, source_fixture_path)
    target_receipt, target_fixture, _ = _fixture_execution(backend, target_fixture_path)
    source_execution = Path(source_fixture["paths"]["backend"])
    target_execution = Path(target_fixture["paths"]["backend"])
    if source_execution == target_execution:
        raise ValueError("秘密导入必须在两个不同的 业务 crate 工作树之间进行")
    source = _secret_directory(backend, source_execution, source_directory)
    source_fixture_binding = bound(source_receipt)
    target_fixture_binding = bound(target_receipt)
    source_files, contents = {}, {}
    for name in SECRET_FILES:
        path = source / name
        descriptor = bound(path)
        content = path.read_bytes()
        if (bound(path) != descriptor or len(content) != descriptor["bytes"]
                or hashlib.sha256(content).hexdigest() != descriptor["sha256"]):
            raise ValueError("秘密导入源在读取期间发生变化")
        source_files[name], contents[name] = descriptor, content
    destination = target_execution / ".local-tests/reference-fixture/secrets"
    if destination.exists() or linked(destination) or not destination.parent.is_dir():
        raise ValueError("新 业务 crate 默认秘密目录已存在或父目录无效")
    destination.mkdir()
    try:
        for name in SECRET_FILES:
            target = destination / name
            with target.open("xb") as stream:
                stream.write(contents[name])
                stream.flush()
                os.fsync(stream.fileno())
        target_files = {name: bound(destination / name) for name in SECRET_FILES}
        if (bound(source_receipt) != source_fixture_binding or bound(target_receipt) != target_fixture_binding
                or any(bound(source / name) != source_files[name] for name in SECRET_FILES)
                or any((destination / name).read_bytes() != contents[name] for name in SECRET_FILES)
                or any({key: target_files[name][key] for key in ("bytes", "sha256")}
                       != {key: source_files[name][key] for key in ("bytes", "sha256")}
                       for name in SECRET_FILES)):
            raise ValueError("秘密导入源、业务 crate 收据或目标字节在复制期间发生变化")
        receipt = {
            "format_version": 1,
            "kind": "reference-fixture-secret-bootstrap",
            "status": "imported",
            "source_fixture": source_fixture_binding,
            "source_files": source_files,
            "target_fixture": target_fixture_binding,
            "target_files": target_files,
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
        raise ValueError("业务 crate 执行工作树与生成快照不一致")
    source = _secret_directory(backend, execution)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    root = execution / ".local-tests/reference-fixture"
    if not output.parent.is_dir() or not output.is_relative_to(root):
        raise ValueError("新的夹具 secret set 必须写入执行工作树的参考夹具目录")
    output.mkdir()
    try:
        for name in SECRET_FILES:
            target = output / name
            if name == "reset-admin-password.txt":
                value = _reset_password().encode("utf-8") + b"\n"
            else:
                value = (source / name).read_bytes()
            with target.open("xb") as stream:
                stream.write(value)
                stream.flush()
                os.fsync(stream.fileno())
        values = {"admin_password": _secret(output, "reset-admin-password.txt")[0]}
        _validate_reset_password(values)
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


def _environment(backend: Path, review: dict, fixture: dict, output: Path, side: str = "seed",
                 secret_directory: Path | None = None) -> tuple[dict, dict]:
    if side not in review["scopes"]:
        raise ValueError("夹具环境必须选择已审阅侧")
    selected = review["scopes"][side]
    execution = Path(fixture["paths"]["backend"])
    # 业务 crate 收据记录的是生成工作树的内容快照，不包含其后产生的忽略运行目录状态。
    if execution != Path(selected["backend_dir"]) or snapshot(execution)[0] != fixture["generated"]["backend"]:
        raise ValueError("所选侧 业务 crate 工作树与审阅计划或生成快照不一致")
    source_secrets = _secret_directory(backend, execution)
    secrets = _secret_directory(backend, execution, secret_directory)
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


def plan(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path, side: str = "seed") -> dict:
    return environment_native("plan", {"backend": str(backend.resolve(strict=True)),
        "review": str(review_path), "fixture": str(fixture_path),
        "maintenance_build": str(maintenance_path), "side": side})


def _planned_inputs(backend: Path, result: dict) -> tuple[Path, dict, Path, dict, Path, dict]:
    values = environment_native("planned-inputs", {"backend": str(backend), "plan": result})["inputs"]
    return tuple(Path(value) if index % 2 == 0 else value for index, value in enumerate(values))


def prepare(backend: Path, review_path: Path, fixture_path: Path, maintenance_path: Path, output: Path,
            side: str = "seed", secret_directory: Path | None = None) -> dict:
    """显式准备冻结环境；只写本地私有环境和收据，绝不创建服务或业务资源。"""
    backend = backend.resolve(strict=True)
    result = plan(backend, review_path, fixture_path, maintenance_path, side)
    output = local_path(backend, str(output if output.is_absolute() else backend / output), new=True)
    if not output.parent.is_dir():
        raise ValueError("夹具环境输出父目录不存在")
    inputs = _planned_inputs(backend, result)
    _, review, _, fixture, maintenance_file, _ = inputs
    execution = Path(fixture["paths"]["backend"])
    output = Path(environment_native("prepare-output", {"backend": str(backend), "plan": result, "output": str(output)})["output"])
    environment, secrets = _environment(backend, review, fixture, output, side, secret_directory)
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
    "plan": (("review", "fixture", "maintenance_build", "side", "native_tool"), (), False),
    "prepare": (("review", "fixture", "maintenance_build", "side", "output", "native_tool"), ("secrets_dir",), True),
    "review": (("review", "output", "native_tool"), (), True),
    "rotate-secrets": (("fixture", "output", "native_tool"), (), True),
    "bootstrap-secrets": (("source_fixture", "source_secrets", "fixture", "native_tool"), (), True),
}


def main(argv: list[str]) -> None:
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
    parser.add_argument("--native-tool", required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args(argv)
    from rate_limit_tool import install_verified_binding
    install_verified_binding(args.native_tool)
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
    raise SystemExit(run_private("environment", PROTOCOL_SCHEMAS, main, positional_operation=True))
