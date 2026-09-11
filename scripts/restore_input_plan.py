"""从已发布恢复证据生成正式参考计划、产品计划和运行绑定。"""

from __future__ import annotations

import argparse
import copy
from datetime import datetime, timezone
import json
from pathlib import Path
import sys

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, local_path
from devex_clone_seed_arm import verify_published_arm_input
from devex_clone_source_proof import REQUEST_FIELDS
from devex_clone_target_binding import execution_backend, request_binding
from restore_comparison_source import verify_comparison_sources
from restore_reference_backup import bound_document, document_binding
from restore_reference_plan import identifier, plan_hash, validate_plan
from restore_reference_target import (
    PRODUCT_FIELDS,
    _backup,
    _fresh,
    _plan,
    _product,
    verify_target_plan,
)
from restore_runtime_evidence import read_json_document, timestamp
from restore_runtime_source import require_bindings


def _document(backend: Path, path: Path):
    requested = path if path.is_absolute() else backend / path
    return read_json_document(local_path(backend, str(requested.absolute())))


def _new_path(backend: Path, path: Path, label: str) -> Path:
    requested = path if path.is_absolute() else backend / path
    result = local_path(backend, str(requested.absolute()), new=True)
    if not result.parent.is_dir():
        raise ValueError(f"{label}的父目录必须已经存在")
    return result


def _independent_work_dir(
    directory: Path,
    source_request: dict,
    selected: dict,
    arm: dict,
    fresh_path: Path,
) -> None:
    protected = [
        Path(source_request["source"]["runtime_dir"]),
        Path(selected["runtime_dir"]),
        fresh_path.parent,
    ]
    if isinstance(arm.get("manifest"), dict) and isinstance(
        arm["manifest"].get("copy_directory"), str
    ):
        protected.append(Path(arm["manifest"]["copy_directory"]))
    resolved = directory.resolve()
    if any(
        resolved == path.resolve()
        or resolved.is_relative_to(path.resolve())
        or path.resolve().is_relative_to(resolved)
        for path in protected
    ):
        raise ValueError("参考计划 work_dir 必须与来源、目标、复制和核验目录完全分离")


def _arm_inputs(backend: Path, arm_path: Path, fresh_path: Path, side: str):
    arm_document, fresh_document = (_document(backend, path) for path in (arm_path, fresh_path))
    arm = verify_published_arm_input(backend, arm_document.path, side)
    fresh = _fresh(backend, arm, fresh_document)
    request = arm["target"]
    _review, selected = request_binding(backend, request)
    execution, _binding = execution_backend(backend, request)
    if (
        arm["binding"] != document_binding(arm_document)
        or arm["target_side"] != side
        or request["side"] != side
        or str(execution) != selected["backend_dir"]
    ):
        raise ValueError("正式恢复输入未绑定同一 arm、目标侧或维护源码")
    arm_document.assert_unchanged()
    fresh_document.assert_unchanged()
    return arm, fresh, request, selected, fresh_document.path


def build_reference(
    backend: Path,
    arm_path: Path,
    fresh_path: Path,
    side: str,
    plan_id: str,
    work_dir: Path,
) -> dict:
    """只读推导参考计划；来源与工具只能来自已发布共享导出。"""
    backend = backend.resolve(strict=True)
    identifier(plan_id)
    arm, _fresh_inputs, request, selected, fresh_document_path = _arm_inputs(
        backend, arm_path, fresh_path, side
    )
    source_request = arm["source"]["request"]
    exact(source_request, REQUEST_FIELDS)
    target = {
        **copy.deepcopy(request["target"]),
        "runtime_dir": selected["runtime_dir"],
        "api_url": selected["api_url"],
        "worker_ready_url": selected["worker_ready_url"],
        "frontend_url": selected["frontend_url"],
    }
    directory = _new_path(backend, work_dir, "参考计划 work_dir")
    _independent_work_dir(directory, source_request, selected, arm, fresh_document_path)
    plan = {
        "format_version": 1,
        "id": plan_id,
        "target_side": side,
        "work_dir": str(directory),
        "tools": copy.deepcopy(source_request["tools"]),
        "source": copy.deepcopy(source_request["source"]),
        "target": target,
    }
    validate_plan(plan, backend)
    if (
        request["target"]
        != {key: plan["target"][key] for key in ("scope_id", "s3", "databases")}
        or arm["result"]["source_generation"]
        != arm["source"]["source_generation"]
    ):
        raise ValueError("参考计划没有绑定当前 source-generation 或 fresh 物理目标")
    return plan


def _fault_time(value: str, captured_at: str) -> None:
    timestamp(value, "恢复故障时间")
    timestamp(captured_at, "备份采集时间")
    fault = datetime.fromisoformat(value.replace("Z", "+00:00"))
    captured = datetime.fromisoformat(captured_at.replace("Z", "+00:00"))
    now = datetime.now(timezone.utc)
    if fault < captured or fault > now or (fault - captured).total_seconds() > 86_400:
        raise ValueError("恢复故障时间必须位于实际备份采集后 24 小时内且不能在未来")


def build_product(
    backend: Path,
    reference_path: Path,
    backup_path: Path,
    comparison_path: Path,
    arm_path: Path,
    fresh_path: Path,
    side: str,
    restore_id: str,
    fault_at: str,
) -> dict:
    """只读生成产品 RestorePlan；探针、数据库和前端 SHA 不接受手工拼接。"""
    backend = backend.resolve(strict=True)
    identifier(restore_id)
    documents = [
        _document(backend, path)
        for path in (reference_path, backup_path, comparison_path)
    ]
    reference, backup, comparison = documents
    _plan(reference.value, backend)
    if reference.value["target_side"] != side:
        raise ValueError("产品计划侧别与参考计划不同")
    copied, manifest = _backup(backend, reference.value, backup)
    sources = verify_comparison_sources(backend, comparison.value)
    arm, _fresh_inputs, request, selected, _fresh_document_path = _arm_inputs(
        backend, arm_path, fresh_path, side
    )
    name = {"base": "b0", "candidate": "b1"}[side]
    source_arm = sources["arms"][name]
    result = arm["result"]
    if (
        sources["source_export"] != copied["source_export"]
        or result["source_export_result"] != sources["source_export"]["result"]
        or result["source_export"] != sources["source_export"]["export"]
        or request["target"]
        != {
            key: reference.value["target"][key]
            for key in ("scope_id", "s3", "databases")
        }
        or any(
            reference.value["target"][key] != selected[key]
            for key in ("runtime_dir", "api_url", "worker_ready_url", "frontend_url")
        )
    ):
        raise ValueError("产品计划未绑定同一备份、比较来源或 fresh 目标")
    _fault_time(fault_at, manifest["captured_at"])
    target = reference.value["target"]
    value = {
        "id": restore_id,
        "backup_id": reference.value["id"],
        "scope_id": target["scope_id"],
        "fault_at": fault_at,
        "databases": [
            {
                "source_key": database["key"],
                "target_key": database["key"],
                "server_uuid": database["server_uuid"],
                "database": database["database"],
            }
            for database in target["databases"]
        ],
        "object_endpoint": target["s3"]["endpoint"],
        "object_prefix": target["scope_id"] + "/",
        "api_ready_url": target["api_url"].rstrip("/") + "/readyz",
        "worker_ready_url": target["worker_ready_url"],
        "frontend_sha": source_arm["sources"]["frontend"]["source"]["snapshot"]["head"],
    }
    exact(value, PRODUCT_FIELDS)
    _product(reference.value, value, source_arm, selected)
    for document in documents:
        document.assert_unchanged()
    return value


def build_bindings(
    backend: Path,
    reference_path: Path,
    target_path: Path,
    backup_path: Path,
    record_path: Path,
) -> dict:
    """只读组装运行绑定；只接受同一目标计划的数据已核验记录。"""
    backend = backend.resolve(strict=True)
    reference, backup, record = (
        _document(backend, path)
        for path in (reference_path, backup_path, record_path)
    )
    _plan(reference.value, backend)
    target = verify_target_plan(backend, reference.value, target_path)
    backup_result, manifest = _backup(backend, reference.value, backup)
    exact(record.value, {
        "plan", "plan_hash", "status", "started_at", "data_verified_at",
        "completed_at", "recovered_at", "failure",
    })
    if (
        record.value["plan"] != target["product_plan"]
        or record.value["plan_hash"] != plan_hash(record.value["plan"])
        or target["backup_receipt"] != document_binding(backup)
        or backup_result["manifest"] != document_binding(bound_document(backend, backup_result["manifest"]))
    ):
        raise ValueError("运行绑定的产品计划、备份或数据核验记录不一致")
    value = {"record": copy.deepcopy(record.value), "manifest": copy.deepcopy(manifest)}
    require_bindings(value)
    for document in (reference, backup, record):
        document.assert_unchanged()
    return value


def _publish(backend: Path, output: Path, build, arguments: tuple) -> dict:
    target = _new_path(backend, output, "正式恢复输入")
    value = build(*arguments)
    if build(*arguments) != value:
        raise ValueError("正式恢复输入在发布前发生变化")
    write_json(target, value)
    if read_json(target) != value or build(*arguments) != value:
        raise ValueError("正式恢复输入在发布后发生变化；保留文件且禁止重放")
    return value


def _add_output(parser) -> None:
    parser.add_argument("--output", type=Path)
    parser.add_argument("--write", action="store_true")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    commands = parser.add_subparsers(dest="operation", required=True)
    reference = commands.add_parser("reference", allow_abbrev=False)
    reference.add_argument("--backend-dir", type=Path, required=True)
    reference.add_argument("--arm-input", type=Path, required=True)
    reference.add_argument("--fresh-target-verify", type=Path, required=True)
    reference.add_argument("--side", choices=("base", "candidate"), required=True)
    reference.add_argument("--id", required=True)
    reference.add_argument("--work-dir", type=Path, required=True)
    _add_output(reference)
    product = commands.add_parser("product", allow_abbrev=False)
    product.add_argument("--backend-dir", type=Path, required=True)
    for field in ("reference-plan", "backup-receipt", "comparison-sources", "arm-input", "fresh-target-verify"):
        product.add_argument("--" + field, type=Path, required=True)
    product.add_argument("--side", choices=("base", "candidate"), required=True)
    product.add_argument("--id", required=True)
    product.add_argument("--fault-at", required=True)
    _add_output(product)
    bindings = commands.add_parser("bindings", allow_abbrev=False)
    bindings.add_argument("--backend-dir", type=Path, required=True)
    for field in ("reference-plan", "target-plan", "backup-receipt", "record"):
        bindings.add_argument("--" + field, type=Path, required=True)
    _add_output(bindings)
    return parser


def main() -> None:
    parser = _parser()
    options = [value.partition("=")[0] for value in sys.argv[1:] if value.startswith("--")]
    if len(options) != len(set(options)):
        parser.error("正式恢复输入选项不能重复")
    args = parser.parse_args()
    if args.write != (args.output is not None):
        parser.error("预览不落盘；发布必须同时指定 --output 与 --write")
    backend = args.backend_dir.resolve(strict=True)
    if args.operation == "reference":
        build, arguments = build_reference, (
            backend, args.arm_input, args.fresh_target_verify, args.side, args.id, args.work_dir
        )
    elif args.operation == "product":
        build, arguments = build_product, (
            backend, args.reference_plan, args.backup_receipt, args.comparison_sources,
            args.arm_input, args.fresh_target_verify, args.side, args.id, args.fault_at
        )
    else:
        build, arguments = build_bindings, (
            backend, args.reference_plan, args.target_plan, args.backup_receipt, args.record
        )
    value = _publish(backend, args.output, build, arguments) if args.write else build(*arguments)
    print(json.dumps({
        "status": f"restore_{args.operation}_{'published' if args.write else 'planned'}",
        "sha256": plan_hash(value),
        "value": value,
    }, ensure_ascii=False))


if __name__ == "__main__":
    main()
