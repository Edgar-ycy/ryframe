"""从严格登记与构建收据推导恢复运行启动请求；不启动进程。"""

from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path
from urllib.parse import urlsplit

from process_environment import configured
from devex_clone_source_proof import verify_api_address_environment
from process_sockets import endpoint
from restore_build import repository
from restore_frontend_build import FRONTEND_RECEIPT
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    artifact_snapshot,
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
    value = exact_fields(document.value, {"environment"}, "恢复运行目标环境")
    private = value["environment"]
    if not isinstance(private, dict) or any(
        not isinstance(key, str) or not isinstance(item, str) for key, item in private.items()
    ):
        raise ValueError("恢复运行目标环境必须是固定文本映射")
    if not document.path.is_relative_to((coordinator / ".local-tests").resolve(strict=True)):
        raise ValueError("恢复运行目标环境必须位于协调器忽略目录")
    return document, private


def _environment(authority: dict, execution: Path, document, private: dict) -> dict:
    if (
        private.get("APP_ENV") != "test"
        or private.get("APP_SCOPE_ID") != authority["scope_id"]
        or private.get("APP_JOBS_MODE") != "external"
    ):
        raise ValueError("恢复运行要求 test 环境、匹配 scope 且使用 external Worker")
    verify_api_address_environment(
        execution, authority["api_endpoint"].removesuffix("/readyz"), private
    )
    _family, worker_host, worker_port = endpoint(authority["worker_endpoint"])
    if (
        private.get("APP_JOBS_HEALTH_HOST") != worker_host
        or private.get("APP_JOBS_HEALTH_PORT") != str(worker_port)
        or any(
            urlsplit(authority[field]).scheme != "http"
            for field in ("api_endpoint", "worker_endpoint", "frontend_endpoint")
        )
    ):
        raise ValueError("恢复运行端点没有绑定当前 API、Worker 或本机 HTTP 配置")
    values = sorted(private.items())
    return {
        "document": _document_descriptor(document),
        "variables": [name for name, _value in values],
        "sha256": hashlib.sha256(canonical_digest(values).encode("utf-8")).hexdigest(),
    }


def _tools(coordinator: Path) -> dict:
    inventory = capture_inventory(coordinator)
    snapshot = inventory["source"]["snapshot"]
    if not snapshot["clean"]:
        raise ValueError("恢复运行协调工具必须来自干净工作树")
    server = artifact_snapshot(coordinator / "tools/python/restore_frontend_server.py")
    return {
        "coordinator_root": str(coordinator),
        "coordinator_head": snapshot["head"],
        "inventory_sha256": canonical_digest(inventory),
        "frontend_server": server.descriptor(),
    }


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
        "format_version": 1,
        "kind": "restore-runtime-launch-request",
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
        or request["format_version"] != 1
        or request["kind"] != "restore-runtime-launch-request"
    ):
        raise ValueError("恢复运行启动请求版本或类型不匹配")
    authority = validate_authority(request["authority"])
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
        request["environment"], {"document", "variables", "sha256"}, "恢复运行环境"
    )
    _descriptor(environment["document"], "恢复运行环境文件")
    if (
        not Path(tools["coordinator_root"]).is_absolute()
        or not isinstance(tools["coordinator_head"], str)
        or HEX_40.fullmatch(tools["coordinator_head"]) is None
        or not isinstance(tools["inventory_sha256"], str)
        or HEX_64.fullmatch(tools["inventory_sha256"]) is None
        or not isinstance(environment["variables"], list)
        or environment["variables"] != sorted(set(environment["variables"]))
        or not isinstance(environment["sha256"], str)
        or HEX_64.fullmatch(environment["sha256"]) is None
        or Path(server["path"]).name != "restore_frontend_server.py"
    ):
        raise ValueError("恢复运行工具或环境来源无效")
    same_backend = authority["backend_product_sha"] == authority["backend_execution_sha"]
    if same_backend != (roots["backend_product"] == roots["backend_execution"]):
        raise ValueError("恢复运行产品与执行来源路径不符合适配关系")
    return request


def commands(request: dict) -> dict[str, list[str]]:
    request = validate_request(request)
    authority, roots, tools = request["authority"], request["roots"], request["tools"]
    _family, host, port = endpoint(authority["frontend_endpoint"])
    return {
        "api": [request["artifacts"]["api"]["executable"]],
        "worker": [request["artifacts"]["worker"]["executable"]],
        "frontend": [
            str(Path(sys.executable).resolve(strict=True)),
            tools["frontend_server"]["path"],
            "__serve",
            "--frontend",
            roots["frontend"],
            "--receipt",
            request["paths"]["frontend_build"],
            "--host",
            host,
            "--port",
            str(port),
        ],
    }


def launch_environment(request: dict, role: str) -> dict[str, str]:
    request = validate_request(request)
    if role not in ROLES:
        raise ValueError("未知恢复运行角色")
    environment = read_json_document(Path(request["environment"]["document"]["path"]))
    if _document_descriptor(environment) != request["environment"]["document"]:
        raise ValueError("恢复运行目标环境文件已经变化")
    private = exact_fields(environment.value, {"environment"}, "恢复运行目标环境")["environment"]
    values = sorted(private.items()) if isinstance(private, dict) else []
    if (
        any(not isinstance(key, str) or not isinstance(value, str) for key, value in values)
        or request["environment"]["variables"] != [name for name, _value in values]
        or request["environment"]["sha256"]
        != hashlib.sha256(canonical_digest(values).encode("utf-8")).hexdigest()
    ):
        raise ValueError("恢复运行目标环境与启动请求不一致")
    if role != "frontend":
        result = configured(private)
        result["SNOWFLAKE_WORKER_ID"] = "1" if role == "api" else "2"
        environment.assert_unchanged()
        return result
    result = configured({"PYTHONUTF8": "1"})
    environment.assert_unchanged()
    return result
