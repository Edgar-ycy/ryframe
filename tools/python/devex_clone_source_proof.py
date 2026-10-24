"""开发复制源的显式配置、构建、进程代次与停止状态证明。"""
from __future__ import annotations

from native_s3_client import _invoke as native_invoke, _read_response as native_response

from contextlib import nullcontext
import copy
import errno
import os
from pathlib import Path
import re
import socket
import subprocess
from urllib.parse import urlsplit

from devex_clone import read_json
from devex_clone_model import digest, exact, linked, local_path, name
from devex_clone_tools import verify as verify_tools
from devex_provenance import verify_source
from full_stack_process import read_process
from full_stack_runtime import verify_runtime
from rate_limit_client import load_app_table
from process_sockets import endpoint, verify_windows_port_idle
from restore_build import file_digest, verify_build_artifacts
from restore_reference_plan import identifier, plan_hash

REQUEST_FIELDS = {"format_version", "kind", "id", "source", "tools", "backend_build", "maintenance_build",
                  "worktree_fingerprint", "runtime", "processes", "producers_registry", "max_object_bytes"}


def public_request(request: dict) -> dict:
    """公开请求投影只保存对象连接身份，凭据保留在已绑定的私有原文件。"""
    proof = copy.deepcopy(request)
    s3 = request["source"]["s3"]
    proof["source"]["s3"] = {field: s3[field] for field in ("endpoint", "region")}
    return proof


def bound_file(backend: Path, value: dict) -> Path:
    exact(value, {"path", "bytes", "sha256"})
    digest(value["sha256"])
    path = local_path(backend, value["path"])
    if type(value["bytes"]) is not int or value["bytes"] <= 0 or file_digest(path) != {key: value[key] for key in ("bytes", "sha256")}:
        raise ValueError("源导出输入文件与登记摘要不一致")
    return path


def validate_source(backend: Path, source: dict) -> None:
    exact(source, {"scope_id", "runtime_dir", "api_url", "s3", "databases"})
    name(source["scope_id"])
    local_path(backend, source["runtime_dir"])
    endpoint(source["api_url"])
    exact(source["s3"], {"endpoint", "region", "access_key", "secret_key"})
    endpoint(source["s3"]["endpoint"])
    if not re.fullmatch(r"[a-z0-9-]+", source["s3"]["region"]):
        raise ValueError("源对象 region 无效")
    for field in ("access_key", "secret_key"):
        if not isinstance(source["s3"][field], str) or not source["s3"][field]:
            raise ValueError("源对象认证必须使用明确的配置文件字段")
    keys, physical = set(), set()
    if not isinstance(source["databases"], list) or not source["databases"]:
        raise ValueError("源数据库目标不能为空")
    for database in source["databases"]:
        exact(database, {"key", "kind", "mode", "database", "server_uuid", "defaults_file", "defaults_sha256"})
        key = name(database["key"])
        identity = (database["server_uuid"], identifier(database["database"]).lower())
        if (not re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", database["server_uuid"])
                or key in keys or identity in physical or database["kind"] not in {"combined", "tenant"}
                or database["mode"] not in {"shared", "dedicated"}
                or (key == "shared-control") != (database["kind"] == "combined")
                or database["kind"] == "combined" and database["mode"] != "shared"):
            raise ValueError("源数据库逻辑目标重复、物理身份重叠或类型不符")
        path = local_path(backend, database["defaults_file"])
        if file_digest(path)["sha256"] != digest(database["defaults_sha256"]):
            raise ValueError("源数据库凭据文件已变化")
        keys.add(key)
        physical.add(identity)
    if "shared-control" not in keys:
        raise ValueError("源必须明确包含 combined 控制目标")


def validate_request(backend: Path, request: dict) -> None:
    exact(request, REQUEST_FIELDS)
    if request["format_version"] != 1 or request["kind"] != "devex-clone-source-export":
        raise ValueError("源导出请求类型无效")
    name(request["id"])
    validate_source(backend, request["source"])
    if (not isinstance(request["worktree_fingerprint"], str)
            or not re.fullmatch(r"sha256:[a-f0-9]{64}", request["worktree_fingerprint"])
            or type(request["max_object_bytes"]) is not int or request["max_object_bytes"] < 0):
        raise ValueError("源导出须绑定完整工作区指纹和对象大小上限")
    exact(request["tools"], {"mysql", "mysqldump", "node"})
    for tool in request["tools"].values():
        exact(tool, {"path", "sha256"})
        path = Path(tool["path"])
        if (not path.is_absolute() or any(linked(item) for item in (path, *path.parents))
                or file_digest(path)["sha256"] != digest(tool["sha256"])):
            raise ValueError("源导出外部工具实际文件与绑定不符")
    for field in ("backend_build", "runtime", "producers_registry"):
        bound_file(backend, request[field])
    bound_file(execution_backend(backend, request), request["maintenance_build"])
    exact(request["processes"], {"api", "worker"})
    directory = Path(request["source"]["runtime_dir"])
    if bound_file(backend, request["runtime"]) != directory / "runtime.json":
        raise ValueError("运行收据不属于源明确运行目录")
    for role, binding in request["processes"].items():
        if bound_file(backend, binding) != directory / f"{role}.json":
            raise ValueError("进程收据不属于源明确运行目录")


def require_closed_port(url: str) -> None:
    if os.name == "nt":
        verify_windows_port_idle(url)
        return
    family, host, port = endpoint(url)
    with socket.socket(family, socket.SOCK_STREAM) as probe:
        # 非 Windows 的连接观察只接受明确拒绝；超时仍不表示端口已关闭。
        probe.settimeout(5)
        result = probe.connect_ex((host, port))
    if result not in {errno.ECONNREFUSED, 10061}:
        raise ValueError("源精确端口仍在监听或无法明确证明连接被拒绝")


def verify_api_address_environment(backend: Path, url: str, environment, *, config_dir: Path | None = None) -> None:
    if set(environment) != {"APP_ENV"}:
        raise ValueError("源 API 配置只接受 APP_ENV 选择的明文配置文件")
    app = load_app_table(backend, environment, config_dir=config_dir)["app"]
    host, port = app.get("host"), app.get("port")
    parsed = urlsplit(url)
    allowed = {"127.0.0.1": {"127.0.0.1", "0.0.0.0"}, "::1": {"::1", "::"}}
    if (parsed.hostname not in allowed or host not in allowed[parsed.hostname] or type(port) is not int
            or parsed.port != port or parsed.path not in ("", "/")):
        raise ValueError("源 API 精确端口未绑定实际 APP 配置")


def verify_api_address(backend: Path, url: str, *, config_dir: Path | None = None) -> None:
    verify_api_address_environment(backend, url, {"APP_ENV": os.environ.get("APP_ENV")}, config_dir=config_dir)


def require_recorded_producer_stopped(expected: dict) -> None:
    """只读核验登记代次停止；身份与原生 CIM 失败均由原生权威拒绝。"""
    result = native_response(*native_invoke(
        "__source-evidence", {"operation": "recorded-process-stopped", "write": False,
            "arguments": {"identity": expected}}, subprocess.run, may_write=False,
    ))[0]["value"]
    exact(result, {"stopped", "signals_sent"})
    if result["stopped"] is not True or type(result["signals_sent"]) is not int or result["signals_sent"] != 0:
        raise ValueError("原生历史 producer 停止证明结果无效")


def producer_identities(request: dict, registry: dict, processes: dict, *, run) -> list[dict]:
    exact(registry, {"format_version", "scope_id", "runtime_sha256", "processes"})
    if (registry["format_version"] != 1 or registry["scope_id"] != request["source"]["scope_id"]
            or registry["runtime_sha256"] != request["runtime"]["sha256"]
            or not isinstance(registry["processes"], list) or len(registry["processes"]) < 3):
        raise ValueError("操作人声明的 producer 清单须包含 API、Worker 和额外数据准备进程")
    names, identities = set(), set()
    for producer in registry["processes"]:
        exact(producer, {"name", "identity"})
        role, identity = name(producer["name"]), producer["identity"]
        exact(identity, {"pid", "started", "executable"})
        identity_key = (identity["pid"], identity["started"])
        if (role in names or type(identity["pid"]) is not int or identity["pid"] <= 1
                or identity_key in identities or not isinstance(identity["started"], str)
                or not identity["started"].isdigit()
                or not Path(identity["executable"]).is_absolute()):
            raise ValueError("producer 内核创建身份无效或重复")
        if role in processes and identity != processes[role]:
            raise ValueError("producer 清单中的 API/Worker 代次与真实运行收据不同")
        require_recorded_producer_stopped(identity)
        names.add(role)
        identities.add(identity_key)
    if not {"api", "worker"}.issubset(names):
        raise ValueError("producer 清单缺少 API 或 Worker")
    return sorted(registry["processes"], key=lambda item: item["name"])


def verify_generation(backend: Path, request: dict, run, *, control_environment=None) -> dict:
    validate_request(backend, request)
    execution = execution_backend(backend, request)
    build = read_json(bound_file(backend, request["backend_build"]))
    with control_environment() if control_environment is not None else nullcontext():
        snapshot = verify_source(execution, build, request["worktree_fingerprint"])
        verify_build_artifacts(build)
        maintenance = verify_tools(execution, bound_file(execution, request["maintenance_build"]), run)
    if (maintenance["source"]["snapshot"] != snapshot
            or maintenance["source"]["worktree_fingerprint"] != request["worktree_fingerprint"]):
        raise ValueError("维护工具与源实际后端构建不属于同一完整源码")
    source, directory = request["source"], Path(request["source"]["runtime_dir"])
    runtime = verify_runtime(execution, directory)
    if runtime != read_json(bound_file(backend, request["runtime"])) or runtime["scope_id"] != source["scope_id"]:
        raise ValueError("源运行配置或 runtime 代次已变化")
    app_env = os.environ.get("APP_ENV")
    file_environment = {"APP_ENV": app_env} if isinstance(app_env, str) else {}
    physical = native_response(*native_invoke(
        "__source-evidence", {"operation": "physical-binding", "write": False,
            "arguments": {
                'backend': str(execution),
                'evidence_root': str(backend),
                'source': source,
                'app_env': (file_environment).get("APP_ENV"),
                'config_dir': None,
            },
        }, subprocess.run, may_write=False,
    ))[0]["value"]
    if load_app_table(execution, file_environment)["jobs"]["mode"] != "external":
        raise ValueError("源必须明确使用 external Worker，scheduler/consumer 由 Worker 承载")
    processes = {}
    for role in ("api", "worker"):
        identity = read_process(directory, role, source["scope_id"])
        artifact = build["artifacts"][role]
        if (Path(identity["executable"]).resolve() != Path(artifact["executable"]).resolve()
                or runtime["artifacts"][role] != {"path": artifact["executable"], "sha256": artifact["sha256"]}):
            raise ValueError("源登记进程实际二进制与本侧构建不一致")
        processes[role] = identity
    producers = producer_identities(request, read_json(bound_file(backend, request["producers_registry"])), processes, run=run)
    verify_api_address_environment(execution, source["api_url"], file_environment)
    require_closed_port(source["api_url"])
    require_closed_port(runtime["worker_ready_url"])
    return {"source": snapshot, "worktree_fingerprint": request["worktree_fingerprint"], "runtime": runtime,
            "processes": processes, "operator_declared_producers": producers, "physical_binding": physical,
            "maintenance": maintenance, "request_sha256": plan_hash(request),
            "external_writers_discovered": False, "clone_verified": False, "restore_qualified": False}


def execution_backend(backend: Path, request: dict) -> Path:
    """运行目录已绑定实际源码根；协调工具目录不冒充源产品执行目录。"""
    runtime = read_json(bound_file(backend, request["runtime"]))
    root = Path(runtime["backend_root"])
    if (not root.is_absolute() or not root.is_dir() or root != root.resolve(strict=True)
            or any(linked(path) for path in (root, *root.parents))):
        raise ValueError("源 runtime 的执行后端必须是无链接的规范绝对目录")
    return root
