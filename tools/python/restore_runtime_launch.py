"""从严格登记与构建收据推导恢复运行启动请求；不启动进程。"""

from __future__ import annotations

from contextlib import contextmanager
import tomllib
from pathlib import Path
from urllib.parse import urlsplit

from process_environment import configured
from devex_clone_inventory import configuration
from process_sockets import endpoint
from rate_limit_tool import tool_binding, verified_tool
from restore_build import repository
from restore_frontend_build import FRONTEND_RECEIPT
from restore_frontend_native import B0_BACKEND_ADAPTER, B0_HEAD, b0_runtime_source
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    exact_fields,
    read_json_document,
    validate_authority,
)
from restore_runtime_registration import registration_binding
from restore_runtime_source import require_bindings, resolve_runtime_sources, runtime_authority
from source_inventory import canonical_digest, capture_inventory

ROLES = ("api", "worker", "frontend")


def _document_descriptor(document) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def _environment_document(coordinator: Path, facts: dict):
    descriptor = _descriptor(facts["fresh_target"]["environment"], "恢复运行目标环境文件")
    document = read_json_document(Path(descriptor["path"]))
    if _document_descriptor(document) != descriptor:
        raise ValueError("恢复运行目标环境与 target plan 描述不一致")
    private = exact_fields(document.value, {
        "metrics_bearer_token", "scope_id", "api_config_dir", "worker_config_dir",
    }, "恢复运行私有配置")
    if any(not isinstance(value, str) or not value or value != value.strip()
           or any(ord(char) < 32 or 0x7F <= ord(char) <= 0x9F for char in value) for value in private.values()):
        raise ValueError("恢复运行私有配置必须使用非空明文字符串")
    if not document.path.is_relative_to((coordinator / ".local-tests").resolve(strict=True)):
        raise ValueError("恢复运行私有配置必须位于协调器忽略目录")
    return document, private


def _environment(authority: dict, execution: Path, document, private: dict) -> dict:
    if private["scope_id"] != authority["scope_id"] or any(
        urlsplit(authority[field]).scheme != "http"
        for field in ("api_endpoint", "worker_endpoint", "frontend_endpoint")
    ):
        raise ValueError("恢复运行私有配置必须匹配 scope 和本机 HTTP 端点")
    snapshots, identifiers = {}, []
    for role in ("api", "worker"):
        directory = Path(private[f"{role}_config_dir"])
        if not directory.is_absolute():
            raise ValueError("恢复运行角色配置目录必须是绝对路径")
        snapshot = configuration(execution, {"APP_ENV": "test"}, {},
                                 evidence_root=document.path.parent, config_dir=directory)
        merged = {}
        for name in ("app.toml", "app.test.toml"):
            if name not in snapshot["files"]:
                continue
            try:
                with (directory / name).open("rb") as stream:
                    value = tomllib.load(stream)
            except (OSError, ValueError):
                raise ValueError("恢复运行角色配置文件无法读取或 TOML 无效") from None
            for key, item in value.items():
                if isinstance(item, dict) and isinstance(merged.get(key), dict):
                    merged[key].update(item)
                else:
                    merged[key] = item
        jobs, app, monitor = (merged.get(key, {}) for key in ("jobs", "app", "monitor"))
        if any(not isinstance(value, dict) for value in (jobs, app, monitor)):
            raise ValueError("恢复运行角色配置表类型无效")
        _family, host, port = endpoint(authority[f"{role}_endpoint"])
        actual = (app.get("host"), app.get("port")) if role == "api" else (
            jobs.get("health_host"), jobs.get("health_port"))
        worker_id = merged.get("snowflake_worker_id")
        if (merged.get("scope_id") != private["scope_id"] or jobs.get("mode") != "external"
                or monitor.get("metrics_bearer_token") != private["metrics_bearer_token"]
                or actual != (host, port) or type(actual[1]) is not int
                or type(worker_id) is not int or not 0 <= worker_id <= 1023):
            raise ValueError("恢复运行角色配置与 scope、指标令牌、端点或实例编号不匹配")
        if configuration(execution, {"APP_ENV": "test"}, {},
                         evidence_root=document.path.parent, config_dir=directory) != snapshot:
            raise ValueError("恢复运行角色配置在读取期间变化")
        snapshots[role] = snapshot
        identifiers.append(worker_id)
    if len(set(identifiers)) != 2:
        raise ValueError("恢复运行 API 与 Worker 的实例编号必须不同")
    document.assert_unchanged()
    return {"document": _document_descriptor(document), "configurations": snapshots}


def _tools(coordinator: Path) -> dict:
    with frontend_tool() as server:
        inventory = capture_inventory(coordinator)
        snapshot = inventory["source"]["snapshot"]
        if not snapshot["clean"]:
            raise ValueError("恢复运行协调工具必须来自干净工作树")
    return {
        "coordinator_root": str(coordinator),
        "coordinator_head": snapshot["head"],
        "inventory_sha256": canonical_digest(inventory),
        "frontend_server": server,
    }


@contextmanager
def frontend_tool(expected=None):
    binding = tool_binding()
    if binding is None:
        raise ValueError("恢复前端服务要求当前正式协调器的原生工具绑定")
    descriptor = {field: binding[field] for field in ("path", "bytes", "sha256")}
    if expected is not None and descriptor != expected:
        raise ValueError("恢复前端服务工具与当前正式协调器绑定不同")
    with verified_tool(binding):
        yield descriptor


def _frontend_launch(authority: dict) -> tuple[int, int, bool]:
    _family, host, port = endpoint(authority["frontend_endpoint"])
    if urlsplit(authority["frontend_endpoint"]).scheme != "http" or host != "127.0.0.1" or not 0 < port <= 65535:
        raise ValueError("恢复前端服务只接受明确的本机 IPv4 HTTP 端口")
    _family, api_host, api_port = endpoint(authority["api_endpoint"])
    api_url = urlsplit(authority["api_endpoint"])
    if (
        api_url.scheme != "http"
        or api_url.path != "/readyz"
        or api_host != "127.0.0.1"
        or not 0 < api_port <= 65535
        or api_port == port
    ):
        raise ValueError("恢复前端代理只接受不同端口的已登记 IPv4 回环 API")
    b0 = b0_runtime_source(authority)
    if not b0 and (authority["frontend_sha"] == B0_HEAD or authority["backend_adapter_contract"] == B0_BACKEND_ADAPTER):
        raise ValueError("恢复 B0 前端服务必须绑定精确历史产品、执行树和适配合同")
    return port, api_port, b0


def _target_matches(authority: dict, facts: dict, resolved: dict, backend_build, frontend_build) -> None:
    product = facts["product_plan"]
    expected = {
        "restore_id": product["id"],
        "backup_id": product["backup_id"],
        "scope_id": product["scope_id"],
        "frontend_sha": product["frontend_sha"],
        "api_endpoint": facts["endpoints"]["api"],
        "worker_endpoint": facts["endpoints"]["worker"],
        "frontend_endpoint": facts["endpoints"]["frontend"],
    }
    if any(authority[field] != value for field, value in expected.items()):
        raise ValueError("恢复运行来源与登记的目标计划、scope 或三端端点不一致")
    execution = facts["product_execution"]
    adapter = execution["adapter"]
    if adapter is not None and not isinstance(adapter, dict):
        raise ValueError("target plan 的 product execution adapter 无效")
    expected_adapter = None if adapter is None else adapter.get("contract")
    expected_roots = {
        "source_backend": resolved["roots"]["backend_product"],
        "execution_backend": resolved["roots"]["backend_execution"],
        "frontend": resolved["roots"]["frontend"],
    }
    expected_builds = {
        "backend": _document_descriptor(backend_build),
        "frontend": _document_descriptor(frontend_build),
    }
    if (
        execution.get("roots") != expected_roots
        or execution.get("backend_product_sha") != authority["backend_product_sha"]
        or execution.get("backend_execution_sha") != authority["backend_execution_sha"]
        or execution.get("frontend_sha") != authority["frontend_sha"]
        or execution.get("builds") != expected_builds
        or expected_adapter != authority["backend_adapter_contract"]
    ):
        raise ValueError("恢复运行构建没有绑定 target plan 的 product execution")


def prepare_request(
    coordinator: Path,
    registration_path: Path,
    target_plan_path: Path,
    execution_backend: Path,
    frontend: Path,
    build_path: Path,
    bindings_path: Path,
    *,
    adapter_contract: str | None = None,
    product_backend: Path | None = None,
) -> tuple[Path, dict]:
    coordinator = repository(coordinator, "恢复运行协调器")
    target_plan = read_json_document(target_plan_path)
    target_descriptor = _document_descriptor(target_plan)
    _registration, facts, registration_documents = registration_binding(
        coordinator, registration_path, target_descriptor
    )
    backend_build = read_json_document(build_path)
    frontend_build = read_json_document(frontend / "dist" / FRONTEND_RECEIPT)
    bindings = read_json_document(bindings_path)
    record, manifest = require_bindings(bindings.value)
    resolved = resolve_runtime_sources(
        coordinator,
        execution_backend,
        frontend,
        backend_build,
        frontend_build,
        record["plan"]["frontend_sha"],
        adapter_contract=adapter_contract,
        product_backend=product_backend,
    )
    authority = runtime_authority(
        record, manifest, resolved["source"], facts["endpoints"]["frontend"]
    )
    _target_matches(authority, facts, resolved, backend_build, frontend_build)
    environment_document, private_environment = _environment_document(coordinator, facts)
    request = {
        "format_version": 2,
        "kind": "restore-runtime-launch-request",
        "runtime_directory": facts["runtime_directory"],
        "authority": authority,
        "registration": {
            "registration": _document_descriptor(registration_documents[0]),
            "target_plan": target_descriptor,
        },
        "roots": resolved["roots"],
        "paths": {
            "bindings": str(bindings.path),
            "backend_build": str(backend_build.path),
            "frontend_build": str(frontend_build.path),
        },
        "digests": {
            "bindings": bindings.sha256,
            "backend_build": backend_build.sha256,
            "frontend_build": frontend_build.sha256,
        },
        "artifacts": backend_build.value["artifacts"],
        "tools": _tools(coordinator),
        "environment": _environment(
            authority,
            Path(resolved["roots"]["backend_execution"]),
            environment_document,
            private_environment,
        ),
    }
    validate_request(request)
    for document in (
        *registration_documents,
        bindings,
        backend_build,
        frontend_build,
        environment_document,
    ):
        document.assert_unchanged()
    return Path(facts["runtime_directory"]), request


def _descriptor(value: object, label: str) -> dict:
    value = exact_fields(value, {"path", "bytes", "sha256"}, label)
    if (
        not isinstance(value["path"], str)
        or not Path(value["path"]).is_absolute()
        or type(value["bytes"]) is not int
        or value["bytes"] <= 0
        or not isinstance(value["sha256"], str)
        or HEX_64.fullmatch(value["sha256"]) is None
    ):
        raise ValueError(f"{label}不是完整文件描述")
    return value


def validate_request(value: object) -> dict:
    request = exact_fields(
        value,
        {
            "format_version",
            "kind",
            "runtime_directory",
            "authority",
            "registration",
            "roots",
            "paths",
            "digests",
            "artifacts",
            "tools",
            "environment",
        },
        "恢复运行启动请求",
    )
    if (
        type(request["format_version"]) is not int
        or request["format_version"] != 2
        or request["kind"] != "restore-runtime-launch-request"
    ):
        raise ValueError("恢复运行启动请求版本或类型不匹配")
    authority = validate_authority(request["authority"])
    _frontend_launch(authority)
    runtime = request["runtime_directory"]
    if not isinstance(runtime, str) or not Path(runtime).is_absolute() or ".." in Path(runtime).parts:
        raise ValueError("恢复运行目录必须是明确的绝对路径")
    registration = exact_fields(
        request["registration"], {"registration", "target_plan"}, "恢复运行登记绑定"
    )
    for label in ("registration", "target_plan"):
        _descriptor(registration[label], label)
    roots = exact_fields(
        request["roots"], {"backend_product", "backend_execution", "frontend"}, "运行来源路径"
    )
    paths = exact_fields(
        request["paths"], {"bindings", "backend_build", "frontend_build"}, "运行输入路径"
    )
    digests = exact_fields(request["digests"], set(paths), "运行输入摘要")
    if any(
        not isinstance(path, str) or not Path(path).is_absolute()
        for path in (*roots.values(), *paths.values())
    ):
        raise ValueError("恢复运行启动请求必须保存绝对输入路径")
    if any(
        not isinstance(digest, str) or HEX_64.fullmatch(digest) is None
        for digest in digests.values()
    ):
        raise ValueError("恢复运行启动请求摘要无效")
    artifacts = exact_fields(request["artifacts"], {"api", "worker"}, "恢复运行产物")
    for role in ("api", "worker"):
        artifact = exact_fields(
            artifacts[role], {"executable", "command", "bytes", "sha256"}, f"{role} 产物"
        )
        if (
            not isinstance(artifact["executable"], str)
            or not Path(artifact["executable"]).is_absolute()
            or not isinstance(artifact["command"], list)
            or type(artifact["bytes"]) is not int
            or artifact["bytes"] <= 0
            or not isinstance(artifact["sha256"], str)
            or HEX_64.fullmatch(artifact["sha256"]) is None
        ):
            raise ValueError(f"{role} 产物绑定无效")
    tools = exact_fields(
        request["tools"],
        {"coordinator_root", "coordinator_head", "inventory_sha256", "frontend_server"},
        "恢复运行工具来源",
    )
    server = _descriptor(tools["frontend_server"], "前端服务工具")
    environment = exact_fields(
        request["environment"], {"document", "configurations"}, "恢复运行环境"
    )
    _descriptor(environment["document"], "恢复运行环境文件")
    if (
        not Path(tools["coordinator_root"]).is_absolute()
        or not isinstance(tools["coordinator_head"], str)
        or HEX_40.fullmatch(tools["coordinator_head"]) is None
        or not isinstance(tools["inventory_sha256"], str)
        or HEX_64.fullmatch(tools["inventory_sha256"]) is None
    ):
        raise ValueError("恢复运行工具或环境来源无效")
    configurations = exact_fields(environment["configurations"], {"api", "worker"}, "运行角色配置")
    for snapshot in configurations.values():
        exact_fields(snapshot, {"directory", "files", "environment_sha256"}, "运行配置快照")
        if (not isinstance(snapshot["directory"], str) or not Path(snapshot["directory"]).is_absolute()
                or not isinstance(snapshot["files"], dict) or "app.test.toml" not in snapshot["files"]
                or HEX_64.fullmatch(str(snapshot["environment_sha256"])) is None):
            raise ValueError("恢复运行配置快照无效")
        for name, descriptor in snapshot["files"].items():
            if Path(name).name != name or not name.endswith(".toml"):
                raise ValueError("恢复运行配置快照文件名无效")
            exact_fields(descriptor, {"bytes", "sha256"}, "角色配置文件")
            _descriptor({"path": str(Path(snapshot["directory"]) / name), **descriptor}, "角色配置文件")
    same_backend = authority["backend_product_sha"] == authority["backend_execution_sha"]
    if same_backend != (roots["backend_product"] == roots["backend_execution"]):
        raise ValueError("恢复运行产品与执行来源路径不符合适配关系")
    with frontend_tool(server):
        pass
    return request


def commands(request: dict) -> dict[str, list[str]]:
    request = validate_request(request)
    authority, roots, tools = request["authority"], request["roots"], request["tools"]
    port, api_port, b0 = _frontend_launch(authority)
    return {
        "api": [request["artifacts"]["api"]["executable"], "--config-dir",
                request["environment"]["configurations"]["api"]["directory"]],
        "worker": [request["artifacts"]["worker"]["executable"], "--config-dir",
                   request["environment"]["configurations"]["worker"]["directory"]],
        "frontend": [
            tools["frontend_server"]["path"],
            "__restore-frontend-serve",
            "--frontend",
            roots["frontend"],
            "--receipt",
            request["paths"]["frontend_build"],
            "--receipt-sha256",
            request["digests"]["frontend_build"],
            "--expected-head",
            authority["frontend_sha"],
            "--host",
            "127.0.0.1",
            "--port",
            str(port),
            "--runtime-dir",
            request["runtime_directory"],
            "--api-port",
            str(api_port),
            *(["--b0"] if b0 else []),
        ],
    }


def verify_configuration(request: dict) -> None:
    request = validate_request(request)
    document, private = _environment_document(
        Path(request["tools"]["coordinator_root"]),
        {"fresh_target": {"environment": request["environment"]["document"]}},
    )
    if _environment(request["authority"], Path(request["roots"]["backend_execution"]),
                    document, private) != request["environment"]:
        raise ValueError("恢复运行私有配置与启动请求不一致")


def launch_environment(request: dict, role: str) -> dict[str, str]:
    if role not in ROLES:
        raise ValueError("未知恢复运行角色")
    verify_configuration(request)
    return configured({"APP_ENV": "test"} if role != "frontend" else {})
