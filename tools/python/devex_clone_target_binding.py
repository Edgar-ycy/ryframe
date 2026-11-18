"""将新目标严格绑定到已审阅单侧资源、实际配置和三份维护工具。"""
from __future__ import annotations

from native_s3_client import (NativeS3InvocationError, _invoke as native_invoke,
                              _read_response as native_response, configuration as object_configuration)
import subprocess

import hashlib
import os
from pathlib import Path
import re

from artifact_digests import filesystem_path
from devex_clone_capture import read_bound_json
from devex_clone_model import digest, exact, linked, local_path
from devex_clone_inventory import configuration as inventory_configuration
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, require_closed_port, verify_api_address
from devex_clone_tools import verify as verify_tools
from full_stack_runtime import configuration_digest, worker_ready_url
from rate_limit_client import load_app_table
from restore_build import file_digest
from restore_reference_plan import configuration_binding, plan_hash

KEYS = {"shared-control": ("combined", "shared"), "shared": ("tenant", "shared"),
        "dedicated-a": ("tenant", "dedicated"), "dedicated-b": ("tenant", "dedicated")}
EXCLUSIVE = {"mysql_exclusive", "redis_exclusive", "object_storage_exclusive"}
REVIEW_FILE_TOOLS = ("mysql", "mysqldump", "rustfs")
_REGISTRATION_FIELDS = {"format_version", "kind", "request", "environment", "storage_run",
                        "target_directory"}


def target_files(target: Path, *, ignored: set[str] | None = None,
                 locked_guard: bool = False, initialize_lock_released: bool = False) -> list[dict]:
    """返回普通目录树的稳定摘要；任何链接、重解析点或特殊文件均拒绝。"""
    if initialize_lock_released and not locked_guard:
        raise ValueError("只有持有目标 guard 时才能核对已释放的初始化锁")
    if locked_guard:
        from devex_clone_target_state import generation_checkpoint, generation_guard_checkpoint

        checkpoint = generation_guard_checkpoint if initialize_lock_released else generation_checkpoint
        checkpoint(target)
    ignored = set() if ignored is None else ignored
    if linked(target) or not target.is_dir():
        raise ValueError("fresh 目标根目录缺失或经过链接")
    entries = []
    pending = [target]
    while pending:
        directory = pending.pop()
        for path in directory.iterdir():
            relative = path.relative_to(target).as_posix()
            if linked(path):
                raise ValueError("fresh 目标阶段文件经过链接")
            native = filesystem_path(path)
            if relative in ignored:
                # 控制互斥以文件或含 owner 记录的目录形式存在，均不属于快照集合。
                continue
            item = {"path": relative}
            if relative == "copy-target.guard" and locked_guard:
                expected = {**item, "type": "file", "bytes": 1,
                            "sha256": hashlib.sha256(b"\0").hexdigest()}
                # generation_checkpoint 已经通过持锁句柄核对同一文件的精确 NUL 字节；
                # Windows 不允许再次打开被 msvcrt 锁定的字节区间。
                if not os.path.isfile(native) or os.stat(native).st_size != 1:
                    raise ValueError("fresh 目标已持有的控制互斥文件无效")
                entries.append(expected)
                continue
            if os.path.isdir(native):
                entries.append({**item, "type": "directory"})
                pending.append(path)
            elif os.path.isfile(native):
                try:
                    digest_value = file_digest(path)
                except OSError as error:
                    raise ValueError("fresh 目标阶段文件无法安全读取：" + relative) from error
                entries.append({**item, "type": "file", **digest_value})
            else:
                raise ValueError("fresh 目标阶段包含未知文件类型")
    result = sorted(entries, key=lambda item: item["path"])
    if locked_guard:
        checkpoint(target)
    return result


def _file_map(files: list[dict]) -> dict[str, dict]:
    if not isinstance(files, list):
        raise ValueError("fresh 目标文件快照不是列表")
    result = {}
    for item in files:
        if not isinstance(item, dict) or item.get("type") not in {"file", "directory"}:
            raise ValueError("fresh 目标文件快照条目无效")
        fields = {"path", "type"} | ({"bytes", "sha256"} if item["type"] == "file" else set())
        path = item.get("path")
        if (set(item) != fields or not isinstance(path, str) or not path
                or "\\" in path or Path(path).is_absolute()
                or any(part in {"", ".", ".."} for part in path.split("/"))
                or path in result):
            raise ValueError("fresh 目标文件快照路径重复、越界或字段无效")
        if item["type"] == "file" and (type(item["bytes"]) is not int or item["bytes"] < 0
                or not isinstance(item["sha256"], str)
                or re.fullmatch(r"[a-f0-9]{64}", item["sha256"]) is None):
            raise ValueError("fresh 目标文件快照摘要无效")
        result[path] = item
    return result


def prepared_target_files(backend: Path, target: Path, evidence: dict,
                          request_descriptor: dict) -> list[dict]:
    """重读 CLI 发布的完整 prepare 快照，并绑定 registration 与空 predecessor。"""
    exact(evidence, {"descriptor", "registration", "predecessor"})
    descriptor = evidence["descriptor"]
    path = bound_file(backend, descriptor)
    snapshot = read_bound_json(path, descriptor)
    exact(snapshot, {"format_version", "kind", "registration", "predecessor", "files"})
    registration_path = bound_file(backend, evidence["registration"])
    registration = read_bound_json(registration_path, evidence["registration"])
    exact(registration, _REGISTRATION_FIELDS)
    if (path.name != "prepared-files.json" or registration_path != path.parent / "registration.json"
            or snapshot["format_version"] != 1
            or snapshot["kind"] != "devex-clone-fresh-target-prepared-files"
            or snapshot["registration"] != evidence["registration"]
            or snapshot["predecessor"] is not None or evidence["predecessor"] is not None
            or registration["format_version"] != 1
            or registration["kind"] != "devex-clone-fresh-target-registration"
            or registration["request"] != request_descriptor
            or local_path(backend, registration["target_directory"]) != target
            or path.parent != target.parent):
        raise ValueError("fresh 目标 prepare 文件快照、registration 或 predecessor 绑定无效")
    _file_map(snapshot["files"])
    from devex_clone_target_prepared_files import validate_prepared_tree

    request = read_bound_json(bound_file(backend, request_descriptor), request_descriptor)
    validate_prepared_tree(backend, target, snapshot["files"], request_descriptor, request)
    if binding(path) != descriptor or binding(registration_path) != evidence["registration"]:
        raise ValueError("fresh 目标 prepare 文件快照或 registration 在读取期间变化")
    return snapshot["files"]


def initialized_target_files(backend: Path, target: Path, *, locked_guard: bool = False) -> dict:
    """验证正式消费者所见完整树及 initialized→prepared→registration 谱系。"""
    workspace = target.parent
    initialized_path = workspace / "initialized-files.json"
    prepared_path = workspace / "prepared-files.json"
    registration_path = workspace / "registration.json"
    descriptors = {"initialized": binding(initialized_path),
                   "prepared": binding(prepared_path),
                   "registration": binding(registration_path)}
    initialized = read_bound_json(initialized_path, descriptors["initialized"])
    exact(initialized, {"format_version", "kind", "registration", "predecessor", "files"})
    invalid = []
    if initialized["format_version"] != 1:
        invalid.append("format_version")
    if initialized["kind"] != "devex-clone-fresh-target-initialized-files":
        invalid.append("kind")
    if initialized["registration"] != descriptors["registration"]:
        invalid.append("registration")
    if initialized["predecessor"] != descriptors["prepared"]:
        invalid.append("predecessor")
    if invalid:
        raise ValueError("fresh 目标 initialized 文件快照谱系无效：" + ", ".join(invalid))
    prepared = read_bound_json(prepared_path, descriptors["prepared"])
    evidence = {"descriptor": descriptors["prepared"],
                "registration": prepared.get("registration"),
                "predecessor": prepared.get("predecessor")}
    registration = read_bound_json(registration_path, descriptors["registration"])
    prepared_target_files(backend, target, evidence, registration["request"])
    _file_map(initialized["files"])
    if initialized["files"] != target_files(target, ignored={"initialize.lock"}
                                             if locked_guard else None,
                                             locked_guard=locked_guard) \
            or any(binding(path) != descriptors[key] for key, path in (
                ("initialized", initialized_path), ("prepared", prepared_path),
                ("registration", registration_path))):
        raise ValueError("fresh 目标 initialized 完整文件树或谱系在核验期间变化")
    return descriptors["initialized"]


def _native(operation: str, backend: Path, **arguments) -> dict:
    """向已绑定协调器交付只读校验；不使用 PATH 或 Cargo 猜测工具。"""
    from rate_limit_tool import tool_binding

    if tool_binding() is None:
        raise ValueError("目标绑定缺少受管 Rust 工具；请通过 run_checks.py 或正式 xtask 入口执行")
    def invoke(command, **options):
        # 完整复验沿用各读命令期限，不增加整体短上界。
        options["timeout"] = None if operation == "fresh-verify-target" else 30
        return subprocess.run(command, **options)

    try:
        result, response = native_invoke(
            "__target-binding", {"format_version": 1, "operation": operation,
                                  "backend": str(backend), "arguments": arguments},
            invoke, may_write=operation == "fresh-verify-target",
        )
    except NativeS3InvocationError as error:
        raise ValueError("原生目标绑定进程未完成：" + str(error.error_type)
                         + "；请核对受管工具与输入文件，保留现场，不重放任何写入") from None
    if (result.returncode == 1 and isinstance(response, dict)
            and set(response) == {"ok", "error"} and response["ok"] is False
            and isinstance(response["error"], dict)
            and set(response["error"]) == {"category", "message", "remote_writes", "automatic_retry"}
            and response["error"]["category"] == "TargetEvidenceRejected"
            and response["error"]["remote_writes"] == 0
            and response["error"]["automatic_retry"] is False
            and isinstance(response["error"]["message"], str)):
        raise ValueError(response["error"]["message"])
    if (result.returncode != 0 or not isinstance(response, dict)
            or set(response) != {"ok", "result"} or response["ok"] is not True
            or not isinstance(response["result"], dict)):
        raise ValueError("原生目标绑定未确认；请核对当前工具和绑定文件，保留现场，不重放任何写入")
    return response["result"]


def validate_review(review: dict) -> None:
    _native("review", Path(__file__).resolve().parents[2], review=review)


def external_file(value: dict) -> None:
    _native("external-file", Path(__file__).resolve().parents[2], descriptor=value)


def request_binding(backend: Path, request: dict) -> tuple[dict, dict]:
    result = _native("request", backend, request=request, predecessor=None)
    return result["review"], result["selected"]


def execution_binary_bindings(backend: Path, request: dict) -> list[dict]:
    return _native("binaries", backend, request=request)["bindings"]


def predecessor_request_binding(backend: Path, request: dict, predecessor: dict) -> tuple[dict, dict]:
    result = _native("request", backend, request=request, predecessor=predecessor)
    return result["review"], result["selected"]


def review_successor_binding(backend: Path, descriptor: dict) -> dict:
    return _native("review-successor", backend, descriptor=descriptor)


def fixture_arm_storage(backend: Path, registration: dict, binding: dict | None,
                        *, active: bool) -> dict | None:
    result = _native("fixture-arm-storage", backend, registration=registration,
                     binding=binding, active=active)
    if set(result) != {"binding"} or (result["binding"] is not None and not isinstance(result["binding"], dict)):
        raise ValueError("原生 arm 夹具存储响应无效；保留现场，不重放")
    return result["binding"]


def execution_backend(backend: Path, request: dict) -> tuple[Path, dict]:
    """由同一原生权威核验当前、生成或独立发布执行来源。"""
    result = _native("execution-backend", backend, request=request)
    return Path(result["path"]), result["binding"]

def application_config_directory(backend: Path, request: dict) -> Path:
    return Path(_native("config-directory", backend, request=request)["path"])


def reset_config(backend: Path, request: dict, selected: dict) -> None:
    _native("reset-configuration", backend, request=request, selected=selected,
            app_env=os.environ.get("APP_ENV"))


def redis_configuration(backend: Path, request: dict, selected: dict, *, environment=None) -> None:
    environment = os.environ if environment is None else environment
    _native("redis-configuration", backend, request=request, selected=selected,
            app_env=environment.get("APP_ENV"))


def never_started(backend: Path, selected: dict) -> None:
    _native("never-started", backend, selected=selected)


def generation(backend: Path, request: dict, run) -> dict:
    environment = {"APP_ENV": os.environ.get("APP_ENV")}
    review, selected = request_binding(backend, request)
    execution_root, execution = execution_backend(backend, request)
    config_dir = application_config_directory(backend, request)
    never_started(backend, selected)
    if configuration_digest(execution_root, config_dir) != digest(request["configuration_sha256"]):
        raise ValueError("新目标 APP 配置或秘密发生变化")
    object_binding = configuration_binding(execution_root, config_dir)
    if object_configuration(object_binding, execution_root) != request["target"]["s3"]:
        raise ValueError("目标对象连接身份与绑定配置文件不同；请核对请求和显式配置，不回显凭据")
    side = {**request["target"], "s3": {"configuration": object_binding}}
    maintenance = verify_tools(execution_root, bound_file(backend, request["maintenance_build"]), run)
    physical = native_response(*native_invoke(
        "__source-evidence", {"operation": "physical-binding", "write": False,
            "arguments": {
                'backend': str(execution_root),
                'evidence_root': str(execution_root),
                'source': side,
                'app_env': environment.get("APP_ENV"),
                'config_dir': str(config_dir),
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
    planned = {db["key"]: db for db in selected["databases"]}
    for actual in physical["databases"]:
        if any(actual[key] != planned[actual["key"]][key] for key in ("host", "port")):
            raise ValueError("目标 MySQL host/port 不是审阅计划中的地址")
    configuration = inventory_configuration(execution_root, environment, side, config_dir=config_dir)
    reset_config(execution_root, request, selected)
    verify_api_address(execution_root, selected["api_url"], config_dir=config_dir)
    table = load_app_table(execution_root, environment, config_dir=config_dir)
    if not isinstance(table.get("jobs"), dict) or table["jobs"].get("mode") != "external":
        raise ValueError("所选配置文件的 jobs.mode 必须为 external")
    if worker_ready_url(execution_root, config_dir) != selected["worker_ready_url"]:
        raise ValueError("所选配置文件的 jobs.health_host/health_port 与目标 Worker 地址不同")
    for field in ("api_url", "worker_ready_url", "frontend_url"):
        require_closed_port(selected[field])
    # 只保存文件中种子密码的摘要，防止同代次 reset 凭据变更。
    bootstrap = table.get("bootstrap", {})
    password = bootstrap.get("admin_password") if isinstance(bootstrap, dict) else None
    if not isinstance(password, str) or not password.strip():
        raise ValueError("所选配置文件的 bootstrap.admin_password 必须显式提供初始化管理员密码")
    secrets = {"bootstrap.admin_password": password}
    if (configuration_binding(execution_root, config_dir) != object_binding
            or configuration_digest(execution_root, config_dir) != request["configuration_sha256"]
            or inventory_configuration(execution_root, environment, side, config_dir=config_dir) != configuration):
        raise ValueError("目标代次观察期间绑定配置发生变化；保留现场并重新核对，不自动重放")
    return {"execution": execution, "maintenance": maintenance, "physical": physical, "configuration": configuration,
            "configuration_sha256": request["configuration_sha256"],
            "seed_credentials_sha256": plan_hash(secrets), "request_sha256": plan_hash(request),
            "review_sha256": request["review"]["sha256"], "selected": selected,
            "operator_declared_controlled_generation_only": True, "external_writers_discovered": False}
