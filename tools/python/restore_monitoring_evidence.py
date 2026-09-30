"""正式恢复监控投递的严格输入绑定与收据模型。"""

from __future__ import annotations

import os
import re
import subprocess
from datetime import datetime, timezone
from pathlib import Path

from devex_clone_source_proof import require_closed_port
from process_sockets import endpoint
from restore_build import repository
from restore_monitoring_rules import bind_rules, expected_paths, verify_rules
from restore_monitoring_permissions import read_private_token
from restore_monitoring_staging import (
    DIRECTORY as STAGING_DIRECTORY,
    MANIFEST as STAGING_MANIFEST,
    RUNNER as STAGED_RUNNER,
    TOKEN_NAME,
    TOOL_VERSIONS,
    create_staging,
    verify_staging,
)
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    artifact_snapshot,
    exact_fields,
    process_identity_record,
    read_json_document,
    reject_link_or_reparse,
    timestamp,
)
from source_inventory import canonical_digest, capture_inventory

RUN_ID = re.compile(r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*")
ALLOWED_WRITES = ["alertmanager-data", "configs", "evidence", "processes", "prometheus-data"]
BINDING_FIELDS = {
    "format_version",
    "kind",
    "status",
    "run_id",
    "scope_id",
    "authority",
    "coordinator",
    "staging",
    "python",
    "runner",
    "environment",
    "credential",
    "tools",
    "rules",
    "endpoints",
    "ports",
    "contact_policy",
    "allowed_writes",
    "created_at",
}


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z")


def canonical_run_id(value: object) -> str:
    if not isinstance(value, str) or len(value) > 64 or RUN_ID.fullmatch(value) is None:
        raise ValueError("监控 run-id 必须是规范的小写连字符标识")
    return value


def descriptor(path: Path) -> dict:
    return artifact_snapshot(path).descriptor()


def require_run_members(directory: Path, expected: set[str]) -> None:
    reject_link_or_reparse(directory)
    if not directory.is_dir():
        raise ValueError("监控 run 路径必须是已存在的普通目录")
    observed = {item.name for item in directory.iterdir()}
    if observed != expected:
        raise ValueError("监控 run 目录包含未知文件或缺少固定文件")


def _receipt_descriptor(value: object, label: str) -> dict:
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


def _source(value: object) -> dict:
    value = exact_fields(
        value,
        {
            "backup_source_sha",
            "backend_product_sha",
            "backend_execution_sha",
            "backend_adapter_contract",
            "frontend_sha",
        },
        "监控恢复来源",
    )
    for name in ("backup_source_sha", "backend_product_sha", "backend_execution_sha", "frontend_sha"):
        if not isinstance(value[name], str) or HEX_40.fullmatch(value[name]) is None:
            raise ValueError("监控恢复来源缺少精确提交")
    same_backend = value["backend_product_sha"] == value["backend_execution_sha"]
    if (same_backend and value["backend_adapter_contract"] is not None) or (
        not same_backend and value["backend_adapter_contract"] != "legacy-stable-readiness-b0-v1"
    ):
        raise ValueError("监控恢复来源与适配合同不一致")
    return value


def _maintenance_execution(value: object) -> dict:
    maintenance = exact_fields(value, {"root", "binding", "build"}, "监控维护来源")
    if not isinstance(maintenance["root"], str) or not Path(maintenance["root"]).is_absolute():
        raise ValueError("监控维护源码根无效")
    maintenance_binding = maintenance["binding"]
    if not isinstance(maintenance_binding, dict) or maintenance_binding.get("path") != maintenance["root"]:
        raise ValueError("监控维护绑定与执行源码根不同")
    if maintenance_binding.get("kind") == "current-backend":
        exact_fields(maintenance_binding, {"kind", "path"}, "监控维护绑定")
    elif maintenance_binding.get("kind") == "device-fixture":
        exact_fields(maintenance_binding, {"kind", "path", "fixture", "source"}, "监控维护绑定")
        _receipt_descriptor(maintenance_binding["fixture"], "监控维护 Device fixture")
        if not isinstance(maintenance_binding["source"], dict):
            raise ValueError("监控维护 Device 来源无效")
    else:
        raise ValueError("监控维护绑定类型无效")
    _receipt_descriptor(maintenance["build"], "监控维护构建")
    return maintenance


def _product_execution(value: object, source: dict) -> dict:
    product = exact_fields(
        value,
        {"roots", "backend_product_sha", "backend_execution_sha", "frontend_sha", "builds", "adapter"},
        "监控产品执行来源",
    )
    roots = exact_fields(product["roots"], {"source_backend", "execution_backend", "frontend"}, "监控产品源码根")
    if any(not isinstance(path, str) or not Path(path).is_absolute() for path in roots.values()):
        raise ValueError("监控产品源码根必须是绝对路径")
    for name in ("backend_product_sha", "backend_execution_sha", "frontend_sha"):
        if product[name] != source[name]:
            raise ValueError("监控产品执行来源与运行来源不同")
    builds = exact_fields(product["builds"], {"backend", "frontend"}, "监控产品构建")
    for name in builds:
        _receipt_descriptor(builds[name], f"监控产品构建 {name}")
    if (product["adapter"] is None) != (source["backend_adapter_contract"] is None):
        raise ValueError("监控产品适配来源不一致")
    if product["adapter"] is not None:
        adapter = exact_fields(
            product["adapter"],
            {
                "contract", "base_backend_sha", "base_frontend_sha", "reference_adapter_sha",
                "adapter_tree", "reconstructed_tree", "adapter_paths", "patch",
            },
            "监控 B0 产品适配来源",
        )
        if adapter["contract"] != source["backend_adapter_contract"]:
            raise ValueError("监控 B0 产品适配合同不同")
    return product


def validate_authority(value: object) -> dict:
    value = exact_fields(
        value,
        {
            "format_version",
            "kind",
            "runtime",
            "target_plan",
            "launch",
            "environment",
            "restore",
            "source",
            "endpoints",
            "maintenance_execution",
            "product_execution",
            "processes",
            "source_generation",
            "dataset_lineage",
        },
        "监控运行权威",
    )
    if value["format_version"] != 1 or value["kind"] != "restore-monitoring-authority":
        raise ValueError("监控运行权威版本或类型无效")
    for name in ("runtime", "target_plan", "launch", "environment", "source_generation", "dataset_lineage"):
        _receipt_descriptor(value[name], f"监控权威 {name}")
    restore = exact_fields(
        value["restore"], {"id", "backup_id", "plan_hash", "scope_id", "data_verified_at"}, "监控恢复身份"
    )
    if (
        not isinstance(restore["id"], str)
        or not restore["id"]
        or not isinstance(restore["backup_id"], str)
        or not restore["backup_id"]
        or not isinstance(restore["scope_id"], str)
        or not restore["scope_id"]
        or not isinstance(restore["plan_hash"], str)
        or HEX_64.fullmatch(restore["plan_hash"]) is None
    ):
        raise ValueError("监控恢复身份无效")
    timestamp(restore["data_verified_at"], "监控数据核验时间")
    source = _source(value["source"])
    endpoints = exact_fields(
        value["endpoints"],
        {"api_ready", "worker_ready", "frontend", "api_metrics", "worker_metrics"},
        "监控产品端点",
    )
    expected_paths = {
        "api_ready": "/readyz",
        "worker_ready": "/readyz",
        "frontend": "",
        "api_metrics": "/api/v1/monitor/metrics",
        "worker_metrics": "/metrics",
    }
    from restore_runtime_evidence import canonical_endpoint

    for name, path in expected_paths.items():
        canonical_endpoint(endpoints[name], path, f"监控产品端点 {name}")
    _maintenance_execution(value["maintenance_execution"])
    _product_execution(value["product_execution"], source)
    processes = exact_fields(value["processes"], {"api", "worker", "frontend"}, "监控产品进程")
    for name, identity in processes.items():
        process_identity_record(identity, f"监控产品进程 {name}")
    return value


def load_environment(
    backend: Path, authority: dict, credential_path: Path, *, acl_reader=None
) -> tuple[dict, dict, object]:
    environment_binding = _receipt_descriptor(authority["environment"], "监控运行环境")
    document = read_json_document(Path(environment_binding["path"]))
    if descriptor(document.path) != environment_binding:
        raise ValueError("监控运行环境与正式启动绑定不同")
    if not document.path.is_relative_to((backend / ".local-tests").resolve(strict=True)):
        raise ValueError("监控运行环境必须位于协调器忽略目录")
    value = exact_fields(document.value, {"environment"}, "监控私有环境")["environment"]
    if not isinstance(value, dict) or any(
        not isinstance(key, str) or not isinstance(item, str) for key, item in value.items()
    ):
        raise ValueError("监控私有环境必须是固定文本映射")
    secret, credential = read_private_token(credential_path, acl_reader=acl_reader)
    if value.get("APP_MONITOR_METRICS_BEARER_TOKEN") != secret:
        raise ValueError("监控指标凭据与正式运行环境不同")
    return value, credential, document


def coordinator_binding(backend: Path) -> dict:
    inventory = capture_inventory(backend)
    snapshot = inventory["source"]["snapshot"]
    if not snapshot["clean"] or not isinstance(snapshot["head"], str) or HEX_40.fullmatch(snapshot["head"]) is None:
        raise ValueError("监控验收必须使用精确干净协调后端")
    return {"root": str(backend), "head": snapshot["head"], "inventory_sha256": canonical_digest(inventory)}


def _python_receipt(value: object) -> dict:
    value = exact_fields(
        value,
        {"path", "bytes", "sha256", "device", "inode", "version", "environment_check_sha256"},
        "监控 Python",
    )
    _staged_receipt({key: value[key] for key in ("path", "bytes", "sha256", "device", "inode")}, "监控 Python")
    if (
        not isinstance(value["version"], str)
        or not value["version"]
        or not isinstance(value["environment_check_sha256"], str)
        or HEX_64.fullmatch(value["environment_check_sha256"]) is None
    ):
        raise ValueError("监控 Python 版本或环境核验摘要无效")
    return value


def _tool_receipt(value: object, name: str) -> dict:
    value = exact_fields(
        value, {"path", "bytes", "sha256", "device", "inode", "version"}, f"监控工具 {name}"
    )
    _staged_receipt(
        {key: value[key] for key in ("path", "bytes", "sha256", "device", "inode")},
        f"监控工具 {name}",
    )
    if not isinstance(value["version"], str) or not value["version"].startswith(TOOL_VERSIONS[name]):
        raise ValueError(f"监控工具 {name} 版本绑定无效")
    return value


def _staged_receipt(value: object, label: str) -> dict:
    value = exact_fields(value, {"path", "bytes", "sha256", "device", "inode"}, label)
    _receipt_descriptor({key: value[key] for key in ("path", "bytes", "sha256")}, label)
    if type(value["device"]) is not int or type(value["inode"]) is not int:
        raise ValueError(f"{label}文件身份无效")
    return value


def validate_binding(value: object) -> dict:
    value = exact_fields(value, BINDING_FIELDS, "监控投递绑定")
    if value["format_version"] != 1 or value["kind"] != "restore-monitoring-binding" or value["status"] != "bound":
        raise ValueError("监控投递绑定版本、类型或状态无效")
    authority = validate_authority(value["authority"])
    canonical_run_id(value["run_id"])
    if value["scope_id"] != authority["restore"]["scope_id"]:
        raise ValueError("监控投递 run 或 scope 无效")
    coordinator = exact_fields(value["coordinator"], {"root", "head", "inventory_sha256"}, "监控协调源码")
    if (
        not isinstance(coordinator["root"], str)
        or not Path(coordinator["root"]).is_absolute()
        or not isinstance(coordinator["head"], str)
        or HEX_40.fullmatch(coordinator["head"]) is None
        or not isinstance(coordinator["inventory_sha256"], str)
        or HEX_64.fullmatch(coordinator["inventory_sha256"]) is None
    ):
        raise ValueError("监控协调源码绑定无效")
    staging = _receipt_descriptor(value["staging"], "监控 staging 清单")
    staging_path = Path(staging["path"])
    if staging_path.name != STAGING_MANIFEST or staging_path.parent.name != STAGING_DIRECTORY:
        raise ValueError("监控 staging 清单路径无效")
    staging_root = staging_path.parent
    run_directory = staging_root.parent
    local = (Path(coordinator["root"]) / ".local-tests").resolve(strict=True)
    if not run_directory.resolve(strict=True).is_relative_to(local):
        raise ValueError("监控 staging 必须位于协调器忽略目录")
    _python_receipt(value["python"])
    runner = _staged_receipt(value["runner"], "监控 runner")
    if Path(runner["path"]) != staging_root / Path(*STAGED_RUNNER.split("/")):
        raise ValueError("监控 runner 必须来自固定 staging 路径")
    if value["environment"] != authority["environment"]:
        raise ValueError("监控绑定环境与正式运行权威不同")
    _receipt_descriptor(value["environment"], "监控运行环境")
    credential = exact_fields(
        value["credential"],
        {"path", "bytes", "sha256", "device", "inode", "security_sha256"},
        "监控凭据",
    )
    if (
        Path(credential["path"]) != run_directory / TOKEN_NAME
        or type(credential["bytes"]) is not int
        or credential["bytes"] <= 0
        or not isinstance(credential["sha256"], str)
        or HEX_64.fullmatch(credential["sha256"]) is None
        or type(credential["device"]) is not int
        or type(credential["inode"]) is not int
        or not isinstance(credential["security_sha256"], str)
        or HEX_64.fullmatch(credential["security_sha256"]) is None
    ):
        raise ValueError("监控凭据绑定无效")
    tools = exact_fields(value["tools"], set(TOOL_VERSIONS), "监控工具集")
    for name in tools:
        _tool_receipt(tools[name], name)
        expected_name = name + (".exe" if os.name == "nt" else "")
        if Path(tools[name]["path"]) != staging_root / "tools" / expected_name:
            raise ValueError(f"监控工具 {name} 必须来自固定 staging 路径")
    expected_python = staging_root / "python" / ("python.exe" if os.name == "nt" else "python")
    if Path(value["python"]["path"]) != expected_python:
        raise ValueError("监控 Python 必须来自固定 staging 路径")
    rules = exact_fields(value["rules"], {"alerts", "tests"}, "监控规则")
    for name in rules:
        _receipt_descriptor(rules[name], f"监控规则 {name}")
    if {name: rules[name]["path"] for name in rules} != expected_paths(Path(coordinator["root"])):
        raise ValueError("监控规则路径不是协调器内固定的正式文件")
    endpoints = exact_fields(value["endpoints"], {"prometheus", "alertmanager", "webhook"}, "监控投递端点")
    ports = exact_fields(value["ports"], set(endpoints), "监控投递端口")
    for name, url in endpoints.items():
        family, host, port = endpoint(url)
        if family not in (2, 23) or host not in {"127.0.0.1", "::1"} or ports[name] != port:
            raise ValueError("监控投递只能使用绑定的 loopback 端口")
    if len(set(ports.values())) != 3 or any(
        type(port) is not int or not 1024 <= port <= 65535 for port in ports.values()
    ):
        raise ValueError("监控投递端口必须互异且有效")
    policy = exact_fields(value["contact_policy"], {"loopback_only", "send_resolved", "external_receivers"}, "监控联系人策略")
    if policy != {"loopback_only": True, "send_resolved": True, "external_receivers": []}:
        raise ValueError("监控投递不得包含真实业务联系人")
    if value["allowed_writes"] != ALLOWED_WRITES:
        raise ValueError("监控投递写入边界无效")
    timestamp(value["created_at"], "监控绑定时间")
    return value


def verify_binding_inputs(
    backend: Path,
    binding: dict,
    preflight,
    run=subprocess.run,
    *,
    acl_reader=None,
) -> tuple[dict, tuple[object, ...]]:
    backend = repository(backend, "监控验收协调后端")
    value = validate_binding(binding)
    if coordinator_binding(backend) != value["coordinator"]:
        raise ValueError("监控协调源码在绑定后发生变化")
    authority = preflight(
        backend, Path(value["authority"]["runtime"]["path"]), Path(value["authority"]["target_plan"]["path"])
    )
    if authority != value["authority"]:
        raise ValueError("监控正式运行权威在绑定后发生变化")
    _private, credential, environment = load_environment(
        backend, authority, Path(value["credential"]["path"]), acl_reader=acl_reader
    )
    if credential != value["credential"]:
        raise ValueError("监控凭据文件在绑定后发生变化")
    snapshots = [environment]
    run_directory = Path(value["staging"]["path"]).parent.parent
    staged = verify_staging(
        run_directory, value["staging"], value["coordinator"], run=run, acl_reader=acl_reader
    )
    if any(staged[name] != value[name] for name in ("python", "runner", "tools", "credential")):
        raise ValueError("监控 staging 执行输入在绑定后发生变化")
    snapshots.extend(verify_rules(backend, value["rules"]))
    return value, tuple(snapshots)


def build_binding(
    backend: Path,
    run_id: str,
    authority: dict,
    run_directory: Path,
    credential_path: Path,
    tools: dict[str, Path],
    ports: dict[str, int],
    *,
    run=subprocess.run,
    port_check=require_closed_port,
    acl_reader=None,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    authority = validate_authority(authority)
    run_id = canonical_run_id(run_id)
    private, credential, environment = load_environment(
        backend, authority, credential_path, acl_reader=acl_reader
    )
    if private.get("APP_SCOPE_ID") != authority["restore"]["scope_id"]:
        raise ValueError("监控私有环境与恢复 scope 不一致")
    if set(tools) != set(TOOL_VERSIONS) or set(ports) != {"prometheus", "alertmanager", "webhook"}:
        raise ValueError("监控工具或端口必须完整提供")
    product_ports = {endpoint(url)[2] for url in authority["endpoints"].values()}
    if (
        len(set(ports.values())) != 3
        or any(type(port) is not int or not 1024 <= port <= 65535 for port in ports.values())
        or set(ports.values()) & product_ports
    ):
        raise ValueError("监控端口必须互异且不能占用产品端口")
    endpoints = {name: f"http://127.0.0.1:{ports[name]}" for name in ports}
    for url in endpoints.values():
        port_check(url)
    coordinator = coordinator_binding(backend)
    rules, rule_snapshots = bind_rules(backend)
    staging = create_staging(
        backend,
        run_directory,
        coordinator,
        credential_path,
        tools,
        run=run,
        acl_reader=acl_reader,
    )
    staged = verify_staging(
        run_directory, staging, coordinator, run=run, acl_reader=acl_reader
    )
    binding = {
        "format_version": 1,
        "kind": "restore-monitoring-binding",
        "status": "bound",
        "run_id": run_id,
        "scope_id": authority["restore"]["scope_id"],
        "authority": authority,
        "coordinator": coordinator,
        "staging": staging,
        "python": staged["python"],
        "runner": staged["runner"],
        "environment": authority["environment"],
        "credential": credential,
        "tools": staged["tools"],
        "rules": rules,
        "endpoints": endpoints,
        "ports": ports,
        "contact_policy": {"loopback_only": True, "send_resolved": True, "external_receivers": []},
        "allowed_writes": ALLOWED_WRITES,
        "created_at": utc_now(),
    }
    validate_binding(binding)
    environment.assert_unchanged()
    if staged["credential"] != credential:
        raise ValueError("监控 staging 凭据与预检凭据不同")
    for snapshot in rule_snapshots:
        snapshot.assert_unchanged()
    return binding


def read_binding(backend: Path, path: Path) -> tuple[object, dict]:
    backend = repository(backend, "监控验收协调后端")
    if not path.is_absolute():
        raise ValueError("监控绑定必须使用绝对路径")
    require_run_members(path.parent, {"binding.json", TOKEN_NAME, STAGING_DIRECTORY})
    document = read_json_document(path)
    if document.path.name != "binding.json" or not document.path.parent.is_relative_to(
        (backend / ".local-tests").resolve(strict=True)
    ):
        raise ValueError("监控绑定必须位于协调器忽略目录的独立 run 目录")
    value = validate_binding(document.value)
    require_run_members(document.path.parent, {"binding.json", TOKEN_NAME, STAGING_DIRECTORY})
    document.assert_unchanged()
    return document, value
