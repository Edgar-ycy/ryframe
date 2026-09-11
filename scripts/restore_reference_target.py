"""双侧正式恢复目标计划：复核全部输入关系，显式发布不可覆盖的新文件。"""

from __future__ import annotations

import hashlib
from datetime import datetime
from pathlib import Path

from devex_clone_factory_context import inventory_history, unfailed
from devex_clone_inventory import expected_owners
from devex_clone_model import digest, exact, local_path
from devex_clone_seed_arm import verify_published_arm_input
from devex_clone_target_binding import execution_backend, request_binding
from restore_comparison_source import verify_comparison_sources
from restore_reference_backup import backup_source, bound_document, document_binding, validate_backup_result
from restore_reference_plan import BUCKETS, identifier, plan_hash, scope_identifier, validate_inventory, validate_plan, verify_artifacts
from restore_runtime_evidence import canonical_endpoint, read_json_document, reject_link_or_reparse, timestamp


FIELDS = {"format_version", "kind", "target_side", "reference_plan_sha256", "backup_receipt",
          "comparison_sources", "comparison_arm", "comparison_arm_sha256", "arm_input",
          "fresh_target", "maintenance_execution", "product_execution", "product_plan_file",
          "product_plan", "product_plan_sha256"}
FRESH_FIELDS = {"verify", "initialized", "registration", "initialized_files", "environment",
                "inventory", "ownership_sha256"}
PRODUCT_FIELDS = {"id", "backup_id", "scope_id", "fault_at", "databases", "object_endpoint",
                  "object_prefix", "api_ready_url", "worker_ready_url", "frontend_sha"}
EXECUTION_FIELDS = {"roots", "backend_product_sha", "backend_execution_sha", "frontend_sha", "builds", "adapter"}


def _product_execution(arm: dict) -> dict:
    """只投影已完整核验的比较来源；维护工具不参与产品运行身份。"""
    return {"roots": arm["roots"],
            "backend_product_sha": arm["sources"]["backend"]["source"]["snapshot"]["head"],
            "backend_execution_sha": arm["execution_sources"]["backend"]["source"]["snapshot"]["head"],
            "frontend_sha": arm["sources"]["frontend"]["source"]["snapshot"]["head"],
            "builds": arm["builds"], "adapter": arm["adapter"]}


def _plan(plan: dict, backend: Path) -> None:
    exact(plan, {"format_version", "id", "target_side", "work_dir", "tools", "source", "target"}
          | ({"dataset"} if "dataset" in plan else set()))
    if type(plan["format_version"]) is not int:
        raise ValueError("参考计划版本必须是整数")
    if "dataset" in plan:
        exact(plan["dataset"], {"timeout_seconds", "request_interval_ms", "api_validation_posts",
                               "post_batch_rows", "tenant_targets", "records", "object_count",
                               "object_bytes", "admin", "owner_password_env"})
        exact(plan["dataset"]["admin"], {"tenant_id", "username", "password_env"})
    exact(plan["tools"], {"mysql", "mysqldump", "aws", "node"})
    for tool in plan["tools"].values():
        exact(tool, {"path", "sha256"})
        reject_link_or_reparse(Path(tool["path"]))
    for name in ("source", "target"):
        side = plan[name]
        fields = {"scope_id", "runtime_dir", "api_url", "s3", "databases"}
        if name == "target":
            fields |= {"worker_ready_url", "frontend_url"}
        exact(side, fields)
        exact(side["s3"], {"endpoint", "region", "access_key_env", "secret_key_env"})
        local_path(backend, side["runtime_dir"])
        for database in side["databases"]:
            exact(database, {"key", "kind", "mode", "database", "server_uuid", "defaults_file", "defaults_sha256"})
            reject_link_or_reparse(Path(database["defaults_file"]))
    local_path(backend, plan["work_dir"])
    validate_plan(plan, backend)


def _input(backend: Path, path: Path):
    return read_json_document(local_path(backend, str(path.absolute())))


def _backup(backend: Path, plan: dict, document) -> tuple[dict, dict]:
    receipt = document.value
    exact(receipt, {"command", "plan_sha256", "started_at", "status", "completed_at", "result"})
    result = receipt["result"]
    validate_backup_result(result)
    original = result["reference_plan"]
    _plan(original, backend)
    root = local_path(backend, result["backup_root"])
    owner = _input(backend, Path(original["work_dir"]) / "reference-owner.json")
    expected_owner = {"format_version": 1, "id": original["id"], "plan_sha256": plan_hash(original)}
    if (receipt["command"] != "backup" or receipt["status"] != "completed"
            or receipt["plan_sha256"] != plan_hash(original) or owner.value != expected_owner
            or document.path != Path(original["work_dir"]) / "backup.json"
            or root != Path(original["work_dir"]) / "backup"
            or any(original[key] != plan[key] for key in ("id", "source", "tools"))):
        raise ValueError("目标计划未绑定同一已完成正式备份、来源或目录 ownership")
    for field in ("started_at", "completed_at"):
        timestamp(receipt[field], "备份收据时间")
    manifest = bound_document(backend, result["manifest"])
    if manifest.path != root / "manifest.json" or result["artifacts"] != len(manifest.value["artifacts"]):
        raise ValueError("正式备份清单位置或产物数量不同")
    validate_inventory(original, manifest.value)
    verify_artifacts(root, manifest.value)
    inventory = {key: value for key, value in manifest.value.items()
                 if key not in {"completed_at", "retention_until", "artifacts"}}
    inputs = backup_source(backend, original, inventory, Path(result["source_generation"]["path"]),
                           Path(result["source_export"]["result"]["path"]))
    if any(result[key] != value for key, value in inputs.items()):
        raise ValueError("正式备份来源证明或共享导出摘要已变化")
    for item in (document, owner, manifest):
        item.assert_unchanged()
    return result, manifest.value


def _ownership(arm: dict) -> str:
    initialized, target = arm["initialized"], arm["target"]["target"]
    observations = initialized["inventory"]["observations"]
    if set(observations) != {db["database"] for db in target["databases"]}:
        raise ValueError("fresh target ownership 未覆盖全部数据库")
    owners = {}
    for database in target["databases"]:
        actual = observations[database["database"]]["ownership"]
        expected = list(expected_owners(target["scope_id"], database["kind"] == "combined"))
        if actual != expected:
            raise ValueError("fresh target 数据库 ownership 不属于精确目标")
        owners[database["key"]] = actual
    objects = initialized["objects"]
    exact(objects, BUCKETS)
    for bucket, value in objects.items():
        marker = f"ryframe-owner:v1:{target['scope_id']}:object-storage:{bucket}".encode()
        expected = {"keys": [target["scope_id"] + "/.ryframe-owner"],
                    "owner": {"bytes": len(marker), "sha256": hashlib.sha256(marker).hexdigest()}}
        if value != expected:
            raise ValueError("fresh target 对象 ownership 与唯一空前缀不符")
    return plan_hash({"databases": owners, "objects": objects, "redis": initialized["redis"]})


def _fresh(backend: Path, arm: dict, document) -> dict:
    value, result = document.value, arm["result"]
    unfailed(document.path.parent)
    exact(value, {"status", "initialized", "inventory", "remote_writes", "target_ready", "clone_verified", "restore_qualified"})
    exact(value["inventory"], {"receipt", "observations"})
    if (document.path.name != "verify.json" or value["status"] != "fresh_target_reverified"
            or value["initialized"] != result["initialized"]
            or type(value["remote_writes"]) is not int or value["remote_writes"] != 0
            or any(value[field] is not False for field in ("target_ready", "clone_verified", "restore_qualified"))
            or value["inventory"]["observations"] != arm["initialized"]["inventory"]["observations"]):
        raise ValueError("fresh-target verify 未绑定同一初始化目标及完整前像")
    inventory_path = document.path.parent / "inventory"
    inventory_document = bound_document(backend, value["inventory"]["receipt"])
    inventory_value = inventory_document.value
    flags = {"producer_stopped_proven", "fresh_target_proven", "target_ready", "clone_verified", "restore_qualified"}
    exact(inventory_value, {"format_version", "status", "side", "scope_id", "binding_sha256", "keys",
                            "inventories", "observations", "remote_writes"} | flags)
    if (type(inventory_value["remote_writes"]) is not int or inventory_value["remote_writes"] != 0
            or any(inventory_value[field] is not False for field in flags)):
        raise ValueError("fresh-target 库存收据必须保持只读且不能冒充恢复证明")
    inventory_history(backend, document.path.parent, value["inventory"], arm["target"],
                      inventory_directory=inventory_path)
    inventory_document.assert_unchanged()
    document.assert_unchanged()
    return {"verify": document_binding(document), "initialized": result["initialized"],
            "registration": result["target_registration"], "initialized_files": result["target_initialized_files"],
            "environment": result["target_environment"], "inventory": value["inventory"]["receipt"],
            "ownership_sha256": _ownership(arm)}


def _product(plan: dict, value: dict, comparison: dict, selected: dict) -> None:
    exact(value, PRODUCT_FIELDS)
    identifier(value["id"])
    identifier(value["backup_id"])
    scope_identifier(value["scope_id"])
    timestamp(value["fault_at"], "恢复故障时间")
    fault = datetime.fromisoformat(value["fault_at"].replace("Z", "+00:00"))
    precision = "seconds" if not fault.microsecond else "milliseconds" if fault.microsecond % 1000 == 0 else "microseconds"
    if value["fault_at"] != fault.isoformat(timespec=precision).replace("+00:00", "Z") or not value["fault_at"].endswith("Z"):
        raise ValueError("产品恢复故障时间必须使用 Rust 原生 UTC Z 格式及秒、毫秒或微秒精度")
    for database in value["databases"]:
        exact(database, {"source_key", "target_key", "server_uuid", "database"})
    databases = [{"source_key": db["key"], "target_key": db["key"],
                  "server_uuid": db["server_uuid"], "database": db["database"]} for db in plan["target"]["databases"]]
    target = plan["target"]
    if (value["backup_id"] != plan["id"] or value["scope_id"] != target["scope_id"]
            or value["databases"] != databases or value["object_endpoint"] != target["s3"]["endpoint"]
            or value["object_prefix"] != target["scope_id"] + "/"
            or value["frontend_sha"] != comparison["sources"]["frontend"]["source"]["snapshot"]["head"]
            or value["api_ready_url"] != selected["api_url"].rstrip("/") + "/readyz"
            or value["worker_ready_url"] != selected["worker_ready_url"]):
        raise ValueError("产品 restore plan 未绑定精确目标、探针或对应 arm 前端 SHA")
    for field in ("api_ready_url", "worker_ready_url"):
        canonical_endpoint(value[field], "/readyz", "恢复探针")
    if value["api_ready_url"] == value["worker_ready_url"]:
        raise ValueError("恢复 API 和 Worker 探针不得重叠")


def capture_target_plan(backend: Path, plan: dict, *, backup_receipt: Path,
                        comparison_sources: Path, arm_input: Path, fresh_target_verify: Path,
                        product_plan: Path, read_only: bool = True) -> dict:
    """推导同一输入快照；预览只读，实际执行重建 B0 适配树。"""
    _plan(plan, backend)
    documents = [_input(backend, path) for path in
                 (backup_receipt, comparison_sources, arm_input, fresh_target_verify, product_plan)]
    backup, comparison, arm_document, fresh, product = documents
    copied, _manifest = _backup(backend, plan, backup)
    sources = verify_comparison_sources(backend, comparison.value, read_only=read_only)
    side = plan["target_side"]
    name = {"base": "b0", "candidate": "b1"}[side]
    source_arm = sources["arms"][name]
    arm = verify_published_arm_input(backend, arm_document.path, side)
    result, request = arm["result"], arm["target"]
    _review, selected = request_binding(backend, request)
    execution, maintenance_binding = execution_backend(backend, request)
    if (sources["source_export"] != copied["source_export"]
            or result["source_export_result"] != sources["source_export"]["result"]
            or result["source_export"] != sources["source_export"]["export"]
            or arm["binding"] != document_binding(arm_document)
            or arm["target_side"] != side or request["side"] != side
            or request["target"] != {key: plan["target"][key] for key in ("scope_id", "s3", "databases")}
            or any(plan["target"][key] != selected[key]
                   for key in ("runtime_dir", "api_url", "worker_ready_url", "frontend_url"))
            or str(execution) != selected["backend_dir"]):
        raise ValueError("目标计划的比较 arm、共享导出、维护源码或物理目标不一致")
    maintenance = bound_document(backend, request["maintenance_build"])
    _product(plan, product.value, source_arm, selected)
    target = _fresh(backend, arm, fresh)
    for document in (*documents, maintenance):
        document.assert_unchanged()
    return {"format_version": 1, "kind": "restore-reference-target-plan", "target_side": side,
            "reference_plan_sha256": plan_hash(plan), "backup_receipt": document_binding(backup),
            "comparison_sources": document_binding(comparison), "comparison_arm": name,
            "comparison_arm_sha256": plan_hash(source_arm), "arm_input": document_binding(arm_document),
            "fresh_target": target,
            "maintenance_execution": {"root": str(execution), "binding": maintenance_binding,
                                      "build": document_binding(maintenance)},
            "product_execution": _product_execution(source_arm), "product_plan_file": document_binding(product),
            "product_plan": product.value, "product_plan_sha256": plan_hash(product.value)}


def verify_target_plan(backend: Path, plan: dict, path: Path, *, read_only: bool = True) -> dict:
    document = _input(backend, path)
    value = document.value
    exact(value, FIELDS)
    exact(value["fresh_target"], FRESH_FIELDS)
    exact(value["maintenance_execution"], {"root", "binding", "build"})
    exact(value["product_execution"], EXECUTION_FIELDS)
    exact(value["product_execution"]["roots"], {"source_backend", "execution_backend", "frontend"})
    exact(value["product_execution"]["builds"], {"backend", "frontend"})
    if (type(value["format_version"]) is not int or value["format_version"] != 1
            or value["kind"] != "restore-reference-target-plan"
            or value["target_side"] != plan["target_side"]
            or value["comparison_arm"] != {"base": "b0", "candidate": "b1"}[plan["target_side"]]
            or value["reference_plan_sha256"] != plan_hash(plan)
            or value["product_plan_sha256"] != plan_hash(value["product_plan"])):
        raise ValueError("正式恢复目标计划类型、侧别或摘要不同")
    descriptors = [value[key] for key in ("backup_receipt", "comparison_sources", "arm_input", "product_plan_file")]
    descriptors += [value["fresh_target"][key] for key in FRESH_FIELDS - {"ownership_sha256"}]
    descriptors += [value["maintenance_execution"]["build"], *value["product_execution"]["builds"].values()]
    for descriptor in descriptors:
        bound_document(backend, descriptor).assert_unchanged()
    digest(value["comparison_arm_sha256"])
    digest(value["fresh_target"]["ownership_sha256"])
    expected = capture_target_plan(backend, plan,
        backup_receipt=Path(value["backup_receipt"]["path"]), comparison_sources=Path(value["comparison_sources"]["path"]),
        arm_input=Path(value["arm_input"]["path"]), fresh_target_verify=Path(value["fresh_target"]["verify"]["path"]),
        product_plan=Path(value["product_plan_file"]["path"]), read_only=read_only)
    if expected != value:
        raise ValueError("正式恢复目标计划与当前完整来源、ownership 或产品计划不同")
    document.assert_unchanged()
    return value


def plan_output(backend: Path, path: Path, value: dict) -> Path:
    output = local_path(backend, str(path.absolute()), new=True)
    if not output.parent.is_dir():
        raise ValueError("目标计划输出父目录必须已经存在")
    protected = [Path(value["fresh_target"]["initialized"]["path"]).parent,
                 Path(value["backup_receipt"]["path"]).parent / "backup"]
    if any(output.is_relative_to(directory) for directory in protected):
        raise ValueError("目标计划输出不得混入备份或 fresh-target 初始化证据")
    return output
