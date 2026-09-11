"""正式恢复监控投递的严格输入绑定与收据模型。"""

from __future__ import annotations

import hashlib
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

from devex_clone_factory_context import configured
from devex_clone_source_proof import require_closed_port
from process_sockets import endpoint
from restore_build import repository
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

TOOL_VERSIONS = {
    "prometheus": "prometheus, version 3.5.0",
    "promtool": "promtool, version 3.5.0",
    "alertmanager": "alertmanager, version 0.34.0",
    "amtool": "amtool, version 0.34.0",
}
ALERTS = frozenset(
    {
        "RyFrameBackupAging",
        "RyFrameBackupStale",
        "RyFrameBackupMissing",
        "RyFrameBackupExpired",
        "RyFrameBackupInvalid",
        "RyFrameBackupMetricMissing",
        "RyFrameRestoreFailed",
        "RyFrameRestoreOverdue",
    }
)
ALLOWED_WRITES = ["alertmanager-data", "configs", "evidence", "processes", "prometheus-data"]
BINDING_FIELDS = {
    "format_version",
    "kind",
    "status",
    "run_id",
    "scope_id",
    "authority",
    "coordinator",
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


def descriptor(path: Path) -> dict:
    return artifact_snapshot(path).descriptor()


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


def bound_artifact(value: object, label: str) -> object:
    binding = _receipt_descriptor(value, label)
    snapshot = artifact_snapshot(Path(binding["path"]))
    if snapshot.descriptor() != binding:
        raise ValueError(f"{label}与当前普通文件不同")
    return snapshot


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


def _ordinary_local_file(backend: Path, path: Path, label: str) -> Path:
    if not path.is_absolute():
        raise ValueError(f"{label}必须是绝对路径")
    reject_link_or_reparse(path)
    path = path.resolve(strict=True)
    local = (backend / ".local-tests").resolve(strict=True)
    if not path.is_file() or not path.is_relative_to(local):
        raise ValueError(f"{label}必须是协调器忽略目录内的普通文件")
    return path


def _read_secret(path: Path) -> tuple[str, int]:
    before = path.stat()
    if before.st_size <= 0 or before.st_size > 8192:
        raise ValueError("监控凭据文件大小无效")
    with path.open("rb") as stream:
        opened = os.fstat(stream.fileno())
        raw = stream.read(8193)
        after_read = os.fstat(stream.fileno())
    after = path.stat()
    state = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns)
    if state(before) != state(opened) or state(before) != state(after_read) or state(before) != state(after):
        raise ValueError("监控凭据在读取期间发生变化")
    try:
        value = raw.decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError as error:
        raise ValueError("监控凭据不是 UTF-8") from error
    if not value or "\x00" in value:
        raise ValueError("监控凭据不能为空")
    return value, len(raw)


def load_environment(backend: Path, authority: dict, credential_path: Path) -> tuple[dict, dict, object]:
    environment_binding = _receipt_descriptor(authority["environment"], "监控运行环境")
    document = read_json_document(Path(environment_binding["path"]))
    if descriptor(document.path) != environment_binding:
        raise ValueError("监控运行环境与正式启动绑定不同")
    if not document.path.is_relative_to((backend / ".local-tests").resolve(strict=True)):
        raise ValueError("监控运行环境必须位于协调器忽略目录")
    value = exact_fields(document.value, {"environment"}, "监控私有环境")["environment"]
    if not isinstance(value, dict) or any(not isinstance(key, str) or not isinstance(item, str) for key, item in value.items()):
        raise ValueError("监控私有环境必须是固定文本映射")
    credential = _ordinary_local_file(backend, credential_path, "监控指标凭据")
    secret, byte_count = _read_secret(credential)
    if value.get("APP_MONITOR_METRICS_BEARER_TOKEN") != secret:
        raise ValueError("监控指标凭据与正式运行环境不同")
    return value, {"path": str(credential), "bytes": byte_count}, document


def tool_binding(path: Path, name: str, backend: Path, run=subprocess.run) -> tuple[dict, object]:
    if name not in TOOL_VERSIONS:
        raise ValueError("未知监控工具")
    path = _ordinary_local_file(backend, path, f"监控工具 {name}")
    snapshot = artifact_snapshot(path)
    completed = run(
        [str(path), "--version"],
        cwd=backend,
        env=configured({"NO_PROXY": "127.0.0.1,localhost,::1"}),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=30,
        check=False,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    raw = completed.stdout if isinstance(completed.stdout, bytes) else str(completed.stdout).encode()
    if completed.returncode != 0 or not raw or len(raw) > 64 * 1024:
        raise ValueError(f"监控工具 {name} 无法报告固定版本")
    version = raw.decode("utf-8", errors="strict").splitlines()[0].strip()
    if len(version) > 512 or not version.isprintable() or not version.startswith(TOOL_VERSIONS[name]):
        raise ValueError(f"监控工具 {name} 版本不是固定合同")
    snapshot.assert_unchanged()
    return {**snapshot.descriptor(), "version": version}, snapshot


def coordinator_binding(backend: Path) -> dict:
    inventory = capture_inventory(backend)
    snapshot = inventory["source"]["snapshot"]
    if not snapshot["clean"] or not isinstance(snapshot["head"], str) or HEX_40.fullmatch(snapshot["head"]) is None:
        raise ValueError("监控验收必须使用精确干净协调后端")
    return {"root": str(backend), "head": snapshot["head"], "inventory_sha256": canonical_digest(inventory)}


def python_binding(backend: Path, run=subprocess.run) -> tuple[dict, object]:
    configured_python = os.environ.get("RYFRAME_PYTHON", "")
    if not configured_python or not Path(configured_python).is_absolute():
        raise ValueError("监控验收要求显式非空绝对 RYFRAME_PYTHON")
    requested = Path(configured_python).resolve(strict=True)
    current = Path(sys.executable).resolve(strict=True)
    if requested != current:
        raise ValueError("监控验收必须由 RYFRAME_PYTHON 指定的解释器执行")
    snapshot = artifact_snapshot(current)
    command = [str(current), "-X", "utf8", "-B", str(backend / "scripts/check_python_environment.py")]
    completed = run(
        command,
        cwd=backend,
        env=configured({"RYFRAME_PYTHON": str(current), "NO_PROXY": "127.0.0.1,localhost,::1"}),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=60,
        check=False,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    raw = completed.stdout if isinstance(completed.stdout, bytes) else str(completed.stdout).encode("utf-8")
    if completed.returncode != 0 or not raw or len(raw) > 64 * 1024:
        raise ValueError("监控验收 Python 环境检查失败")
    snapshot.assert_unchanged()
    return {
        **snapshot.descriptor(),
        "version": sys.version.split()[0],
        "environment_check_sha256": hashlib.sha256(raw).hexdigest(),
    }, snapshot


def _python_receipt(value: object) -> dict:
    value = exact_fields(
        value,
        {"path", "bytes", "sha256", "version", "environment_check_sha256"},
        "监控 Python",
    )
    _receipt_descriptor({key: value[key] for key in ("path", "bytes", "sha256")}, "监控 Python")
    if (
        not isinstance(value["version"], str)
        or not value["version"]
        or not isinstance(value["environment_check_sha256"], str)
        or HEX_64.fullmatch(value["environment_check_sha256"]) is None
    ):
        raise ValueError("监控 Python 版本或环境核验摘要无效")
    return value


def _tool_receipt(value: object, name: str) -> dict:
    value = exact_fields(value, {"path", "bytes", "sha256", "version"}, f"监控工具 {name}")
    _receipt_descriptor({key: value[key] for key in ("path", "bytes", "sha256")}, f"监控工具 {name}")
    if not isinstance(value["version"], str) or not value["version"].startswith(TOOL_VERSIONS[name]):
        raise ValueError(f"监控工具 {name} 版本绑定无效")
    return value


def validate_binding(value: object) -> dict:
    value = exact_fields(value, BINDING_FIELDS, "监控投递绑定")
    if value["format_version"] != 1 or value["kind"] != "restore-monitoring-binding" or value["status"] != "bound":
        raise ValueError("监控投递绑定版本、类型或状态无效")
    authority = validate_authority(value["authority"])
    if (
        not isinstance(value["run_id"], str)
        or not value["run_id"]
        or len(value["run_id"]) > 64
        or value["scope_id"] != authority["restore"]["scope_id"]
    ):
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
    _python_receipt(value["python"])
    _receipt_descriptor(value["runner"], "监控 runner")
    if value["environment"] != authority["environment"]:
        raise ValueError("监控绑定环境与正式运行权威不同")
    _receipt_descriptor(value["environment"], "监控运行环境")
    credential = exact_fields(value["credential"], {"path", "bytes"}, "监控凭据")
    if not isinstance(credential["path"], str) or not Path(credential["path"]).is_absolute() or type(credential["bytes"]) is not int or credential["bytes"] <= 0:
        raise ValueError("监控凭据绑定无效")
    tools = exact_fields(value["tools"], set(TOOL_VERSIONS), "监控工具集")
    for name in tools:
        _tool_receipt(tools[name], name)
    rules = exact_fields(value["rules"], {"alerts", "tests"}, "监控规则")
    for name in rules:
        _receipt_descriptor(rules[name], f"监控规则 {name}")
    endpoints = exact_fields(value["endpoints"], {"prometheus", "alertmanager", "webhook"}, "监控投递端点")
    ports = exact_fields(value["ports"], set(endpoints), "监控投递端口")
    for name, url in endpoints.items():
        family, host, port = endpoint(url)
        if family not in (2, 23) or host not in {"127.0.0.1", "::1"} or ports[name] != port:
            raise ValueError("监控投递只能使用绑定的 loopback 端口")
    if len(set(ports.values())) != 3 or any(type(port) is not int or not 1024 <= port <= 65535 for port in ports.values()):
        raise ValueError("监控投递端口必须互异且有效")
    policy = exact_fields(value["contact_policy"], {"loopback_only", "send_resolved", "external_receivers"}, "监控联系人策略")
    if policy != {"loopback_only": True, "send_resolved": True, "external_receivers": []}:
        raise ValueError("监控投递不得包含真实业务联系人")
    if value["allowed_writes"] != ALLOWED_WRITES:
        raise ValueError("监控投递写入边界无效")
    timestamp(value["created_at"], "监控绑定时间")
    return value


def verify_binding_inputs(backend: Path, binding: dict, preflight, run=subprocess.run) -> tuple[dict, tuple[object, ...]]:
    backend = repository(backend, "监控验收协调后端")
    value = validate_binding(binding)
    if coordinator_binding(backend) != value["coordinator"]:
        raise ValueError("监控协调源码在绑定后发生变化")
    authority = preflight(
        backend, Path(value["authority"]["runtime"]["path"]), Path(value["authority"]["target_plan"]["path"])
    )
    if authority != value["authority"]:
        raise ValueError("监控正式运行权威在绑定后发生变化")
    _private, credential, environment = load_environment(backend, authority, Path(value["credential"]["path"]))
    if credential != value["credential"]:
        raise ValueError("监控凭据文件在绑定后发生变化")
    snapshots = [environment]
    python, python_snapshot = python_binding(backend, run)
    if python != value["python"] or descriptor(Path(__file__).with_name("restore_monitoring_delivery.py")) != value["runner"]:
        raise ValueError("监控 Python 或 runner 在绑定后发生变化")
    snapshots.append(python_snapshot)
    for name, expected in value["tools"].items():
        actual, snapshot = tool_binding(Path(expected["path"]), name, backend, run)
        if actual != expected:
            raise ValueError(f"监控工具 {name} 在绑定后发生变化")
        snapshots.append(snapshot)
    for name, expected in value["rules"].items():
        snapshot = bound_artifact(expected, f"监控规则 {name}")
        snapshots.append(snapshot)
    return value, tuple(snapshots)


def build_binding(
    backend: Path,
    run_id: str,
    authority: dict,
    credential_path: Path,
    tools: dict[str, Path],
    ports: dict[str, int],
    *,
    run=subprocess.run,
    port_check=require_closed_port,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    authority = validate_authority(authority)
    if not isinstance(run_id, str) or not run_id or len(run_id) > 64 or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-" for character in run_id):
        raise ValueError("监控 run-id 只能使用小写字母、数字和连字符")
    private, credential, environment = load_environment(backend, authority, credential_path)
    if private.get("APP_SCOPE_ID") != authority["restore"]["scope_id"]:
        raise ValueError("监控私有环境与恢复 scope 不一致")
    if set(tools) != set(TOOL_VERSIONS) or set(ports) != {"prometheus", "alertmanager", "webhook"}:
        raise ValueError("监控工具或端口必须完整提供")
    product_ports = {endpoint(url)[2] for url in authority["endpoints"].values()}
    if len(set(ports.values())) != 3 or any(type(port) is not int or not 1024 <= port <= 65535 for port in ports.values()) or set(ports.values()) & product_ports:
        raise ValueError("监控端口必须互异且不能占用产品端口")
    endpoints = {name: f"http://127.0.0.1:{ports[name]}" for name in ports}
    for url in endpoints.values():
        port_check(url)
    python, python_snapshot = python_binding(backend, run)
    tool_receipts, snapshots = {}, [python_snapshot]
    for name, path in tools.items():
        tool_receipts[name], snapshot = tool_binding(path, name, backend, run)
        snapshots.append(snapshot)
    rules = {
        "alerts": descriptor(backend / "deploy/prometheus/ryframe-alerts.yml"),
        "tests": descriptor(backend / "deploy/prometheus/ryframe-alerts.test.yml"),
    }
    binding = {
        "format_version": 1,
        "kind": "restore-monitoring-binding",
        "status": "bound",
        "run_id": run_id,
        "scope_id": authority["restore"]["scope_id"],
        "authority": authority,
        "coordinator": coordinator_binding(backend),
        "python": python,
        "runner": descriptor(Path(__file__).with_name("restore_monitoring_delivery.py")),
        "environment": authority["environment"],
        "credential": credential,
        "tools": tool_receipts,
        "rules": rules,
        "endpoints": endpoints,
        "ports": ports,
        "contact_policy": {"loopback_only": True, "send_resolved": True, "external_receivers": []},
        "allowed_writes": ALLOWED_WRITES,
        "created_at": utc_now(),
    }
    validate_binding(binding)
    environment.assert_unchanged()
    for snapshot in snapshots:
        snapshot.assert_unchanged()
    return binding


def read_binding(backend: Path, path: Path) -> tuple[object, dict]:
    backend = repository(backend, "监控验收协调后端")
    document = read_json_document(path)
    if document.path.name != "binding.json" or not document.path.parent.is_relative_to((backend / ".local-tests").resolve(strict=True)):
        raise ValueError("监控绑定必须位于协调器忽略目录的独立 run 目录")
    return document, validate_binding(document.value)
