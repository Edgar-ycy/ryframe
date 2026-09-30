"""解析恢复运行使用的产品、执行适配器、前端与备份来源身份。"""

from __future__ import annotations

from pathlib import Path

from restore_build import registered_source, repository, verify_build
from restore_frontend_build import validate_registered_frontend
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    JsonDocument,
    timestamp,
    valid_identifier,
    valid_scope_identifier,
    validate_authority,
)
from source_inventory import build_source_domains, capture_inventory


def _head(inventory: dict) -> str:
    return inventory["source"]["snapshot"]["head"]


def require_bindings(bindings: dict) -> tuple[dict, dict]:
    if not isinstance(bindings, dict) or set(bindings) != {"record", "manifest"}:
        raise ValueError("恢复 bindings 字段必须精确匹配当前格式")
    record, manifest = bindings["record"], bindings["manifest"]
    if not isinstance(record, dict) or not isinstance(manifest, dict) or not isinstance(record.get("plan"), dict):
        raise ValueError("恢复 bindings 缺少 record、manifest 或 plan")
    plan = record["plan"]
    checks = (
        record.get("status") == "data_verified",
        plan.get("backup_id") == manifest.get("id"),
        isinstance(record.get("plan_hash"), str) and bool(HEX_64.fullmatch(record["plan_hash"])),
        isinstance(manifest.get("source_sha"), str) and bool(HEX_40.fullmatch(manifest["source_sha"])),
        isinstance(plan.get("frontend_sha"), str) and bool(HEX_40.fullmatch(plan["frontend_sha"])),
    )
    if not all(checks):
        raise ValueError("运行产物必须绑定已完成数据校验的恢复演练")
    for field in ("id", "backup_id"):
        if not valid_identifier(plan.get(field)):
            raise ValueError(f"恢复 bindings 的 {field} 无效")
    if not valid_scope_identifier(plan.get("scope_id")):
        raise ValueError("恢复 bindings 的 scope_id 无效")
    timestamp(record.get("data_verified_at"), "恢复数据校验时间")
    return record, manifest


def runtime_authority(
    record: dict,
    manifest: dict,
    runtime_source: dict,
    frontend_endpoint: str,
) -> dict:
    plan = record["plan"]
    return validate_authority(
        {
            "format_version": 2,
            "kind": "restore-runtime-authority",
            "restore_id": plan["id"],
            "backup_id": plan["backup_id"],
            "plan_hash": record["plan_hash"],
            "scope_id": plan["scope_id"],
            "data_verified_at": record["data_verified_at"],
            "backup_source_sha": manifest["source_sha"],
            **runtime_source,
            "api_endpoint": plan["api_ready_url"],
            "worker_endpoint": plan["worker_ready_url"],
            "frontend_endpoint": frontend_endpoint,
        }
    )


def _backend_sources(
    coordinator: Path,
    execution: Path,
    receipt: JsonDocument,
    adapter_contract: str | None,
    product_backend: Path | None,
) -> tuple[Path, dict, dict]:
    execution_inventory = capture_inventory(execution)
    execution_head = _head(execution_inventory)
    verify_build(execution, receipt.value, execution_head)
    if receipt.value["sources"]["full"] != execution_inventory:
        raise ValueError("后端构建收据没有绑定精确执行来源")
    if adapter_contract is None:
        if product_backend is not None:
            raise ValueError("没有适配器的恢复运行不得另行指定产品来源")
        expected_product = build_source_domains(execution_inventory, "backend")["product"]
        if receipt.value["sources"]["product"] != expected_product:
            raise ValueError("普通恢复运行的产品来源与执行来源不同")
        return execution, execution_inventory, execution_inventory
    _source, registered_execution, product = registered_source(
        coordinator,
        execution,
        execution_head,
        adapter_contract=adapter_contract,
        product_backend=product_backend,
    )
    if product is None or registered_execution != execution_inventory:
        raise ValueError("恢复运行缺少已登记的后端适配关系")
    product_root, product_inventory = product
    expected_product = build_source_domains(product_inventory, "backend")["product"]
    if receipt.value["sources"]["product"] != expected_product:
        raise ValueError("后端适配构建没有保持登记产品输入")
    return product_root, product_inventory, execution_inventory


def resolve_runtime_sources(
    coordinator: Path,
    execution_backend: Path,
    frontend: Path,
    backend_receipt: JsonDocument,
    frontend_receipt: JsonDocument,
    expected_frontend_sha: str,
    *,
    adapter_contract: str | None = None,
    product_backend: Path | None = None,
) -> dict:
    """严格绑定运行来源；B0 适配器和普通 B1 使用同一结果模型。"""
    coordinator = repository(coordinator, "恢复协调器源码")
    execution = repository(execution_backend, "后端执行来源")
    frontend = repository(frontend, "前端运行来源")
    product, product_inventory, execution_inventory = _backend_sources(
        coordinator, execution, backend_receipt, adapter_contract, product_backend
    )
    frontend_inventory, observed_frontend = validate_registered_frontend(frontend, expected_frontend_sha)
    if (
        observed_frontend.path != frontend_receipt.path
        or observed_frontend.raw != frontend_receipt.raw
        or observed_frontend.state != frontend_receipt.state
    ):
        raise ValueError("前端构建收据在来源绑定期间被替换")
    identity = {
        "backend_product_sha": _head(product_inventory),
        "backend_execution_sha": _head(execution_inventory),
        "backend_adapter_contract": adapter_contract,
        "frontend_sha": _head(frontend_inventory),
    }
    if adapter_contract is None and identity["backend_product_sha"] != identity["backend_execution_sha"]:
        raise ValueError("普通恢复运行必须使用相同的产品与执行提交")
    if capture_inventory(product) != product_inventory or capture_inventory(execution) != execution_inventory:
        raise ValueError("绑定期间后端产品或执行来源发生变化")
    return {
        "roots": {
            "backend_product": str(product),
            "backend_execution": str(execution),
            "frontend": str(frontend),
        },
        "source": identity,
    }
