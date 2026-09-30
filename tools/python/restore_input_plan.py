"""从已发布恢复证据生成正式参考计划、产品计划和运行绑定。"""

from __future__ import annotations

import copy
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import sys
import uuid

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
    return result.parent.resolve(strict=True) / result.name


def _canonical_json_bytes(value: dict) -> bytes:
    content = (
        json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    if len(content) > 16 * 1024 * 1024:
        raise ValueError("正式恢复输入不能超过 16 MiB")
    return content


def _verify_published(path: Path, value: dict, content: bytes) -> None:
    with path.open("rb") as stream:
        observed = stream.read(16 * 1024 * 1024 + 1)
    if observed != content:
        raise ValueError("正式恢复输入写后字节与规范结果不同")
    if json.loads(observed) != value:
        raise ValueError("正式恢复输入写后内容与规范结果不同")


def _publish_json(target: Path, value: dict) -> None:
    """在同目录完整落盘后以 create-new 语义发布，绝不删除已发布结果。"""
    content = _canonical_json_bytes(value)
    pending = target.with_name(f".{target.name}.{uuid.uuid4().hex}.pending")
    published = False
    try:
        with pending.open("xb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        _verify_published(pending, value, content)
        try:
            os.link(pending, target)
        except FileExistsError as error:
            raise ValueError("正式恢复输入已被并发创建，禁止覆盖") from error
        published = True
        try:
            _verify_published(target, value, content)
            if target.resolve(strict=True) != target:
                raise ValueError("正式恢复输入写后规范路径发生变化")
        except (OSError, ValueError) as error:
            raise ValueError(
                "正式恢复输入已经创建但写后复核失败；必须保留并核对"
            ) from error
    finally:
        try:
            pending.unlink(missing_ok=True)
        except OSError as error:
            if not published:
                raise
            print(f"正式恢复输入已发布，但临时文件清理失败：{error}", file=sys.stderr)


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
    _publish_json(target, value)
    if build(*arguments) != value:
        raise ValueError("正式恢复输入在发布后发生变化；保留文件且禁止重放")
    return value


PROTOCOL_KEY = "RYFRAME_XTASK_RECOVERY_INPUTS"
PROTOCOL_PREFIX = "RYFRAME_XTASK_RECOVERY_"
OPERATIONS = frozenset(("reference", "product", "bindings"))


class RestoreInputsProtocolError(ValueError):
    """Rust 与恢复输入私有实现之间的参数协议无效。"""


@dataclass(frozen=True)
class RestoreInputsRequest:
    operation: str
    backend_dir: Path
    write: bool
    output: Path | None = None
    arm_input: Path | None = None
    fresh_target_verify: Path | None = None
    side: str | None = None
    id: str | None = None
    work_dir: Path | None = None
    reference_plan: Path | None = None
    backup_receipt: Path | None = None
    comparison_sources: Path | None = None
    fault_at: str | None = None
    target_plan: Path | None = None
    record: Path | None = None


def _unique_protocol_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise RestoreInputsProtocolError(f"恢复输入私有协议字段重复：{key}")
        result[key] = value
    return result


def _exact_protocol(value: object, fields: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise RestoreInputsProtocolError(f"{label}字段不完整或含未知字段")
    return value


def _protocol_path(value: object, label: str) -> Path:
    path = Path(value) if isinstance(value, str) else None
    if (not isinstance(value, str) or not value or path is None or not path.is_absolute()
            or ".." in path.parts
            or any(character in value for character in ("\n", "\r", "\0"))):
        raise RestoreInputsProtocolError(f"{label}必须是无父目录跳转和控制字符的非空绝对路径")
    return path


def _request_fields(operation: str, write: bool) -> set[str]:
    common = {"backend_dir", "format_version", "operation", "write"}
    fields = {
        "reference": {"arm_input", "fresh_target_verify", "side", "id", "work_dir"},
        "product": {"reference_plan", "backup_receipt", "comparison_sources", "arm_input",
                    "fresh_target_verify", "side", "id", "fault_at"},
        "bindings": {"reference_plan", "target_plan", "backup_receipt", "record"},
    }[operation]
    return common | fields | ({"output"} if write else set())


def private_protocol_request(argv: list[str] | None = None,
                             environment: dict[str, str] | None = None) -> RestoreInputsRequest:
    arguments = sys.argv[1:] if argv is None else argv
    values = os.environ if environment is None else environment
    raw = values.pop(PROTOCOL_KEY, None)
    if arguments:
        raise RestoreInputsProtocolError("恢复输入私有脚本不接受命令行参数")
    if any(name.startswith(PROTOCOL_PREFIX) for name in values):
        raise RestoreInputsProtocolError("恢复输入私有环境含未知或串线协议")
    if (not isinstance(raw, str) or not raw or len(raw) > 65_536
            or any(character in raw for character in ("\n", "\r", "\0"))):
        raise RestoreInputsProtocolError("恢复输入私有协议缺失、为空、过长或含控制字符")
    try:
        protocol = json.loads(raw, object_pairs_hook=_unique_protocol_object)
    except RestoreInputsProtocolError:
        raise
    except (TypeError, json.JSONDecodeError) as error:
        raise RestoreInputsProtocolError("恢复输入私有协议不是有效 JSON") from error
    return _request_from_protocol(protocol)


def _request_from_protocol(protocol: object) -> RestoreInputsRequest:
    outer = _exact_protocol(protocol, {"format_version", "kind", "request"}, "恢复输入私有协议")
    if (type(outer["format_version"]) is not int or outer["format_version"] != 1
            or outer["kind"] != "ryframe-xtask-recovery-inputs"):
        raise RestoreInputsProtocolError("恢复输入私有协议版本或类型无效")
    request = outer["request"]
    if not isinstance(request, dict):
        raise RestoreInputsProtocolError("恢复输入私有请求必须是对象")
    operation, write = request.get("operation"), request.get("write")
    if not isinstance(operation, str) or operation not in OPERATIONS:
        raise RestoreInputsProtocolError("恢复输入私有协议操作无效")
    if type(write) is not bool:
        raise RestoreInputsProtocolError("恢复输入私有协议写入授权必须是布尔值")
    values = _exact_protocol(request, _request_fields(operation, write),
                             f"恢复输入 {operation} 私有请求")
    if type(values["format_version"]) is not int or values["format_version"] != 1:
        raise RestoreInputsProtocolError("恢复输入私有请求版本无效")
    return _restore_inputs_request(values, operation, write)


def _restore_inputs_request(values: dict, operation: str, write: bool) -> RestoreInputsRequest:
    paths = {field: _protocol_path(values[field], field) for field in (
        "backend_dir", "output", "arm_input", "fresh_target_verify", "work_dir",
        "reference_plan", "backup_receipt", "comparison_sources", "target_plan", "record"
    ) if field in values}
    side = values.get("side")
    if side is not None and (not isinstance(side, str) or side not in {"base", "candidate"}):
        raise RestoreInputsProtocolError("恢复输入 side 无效")
    plan_id = values.get("id")
    if plan_id is not None:
        try:
            identifier(plan_id)
        except (TypeError, ValueError) as error:
            raise RestoreInputsProtocolError("恢复输入 id 无效") from error
    fault_at = values.get("fault_at")
    if fault_at is not None:
        try:
            timestamp(fault_at, "恢复故障时间")
        except (TypeError, ValueError) as error:
            raise RestoreInputsProtocolError("恢复输入 fault_at 无效") from error
    return RestoreInputsRequest(operation=operation, write=write, side=side, id=plan_id,
                                fault_at=fault_at, **paths)


def main(args: RestoreInputsRequest) -> None:
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
    try:
        main(private_protocol_request())
    except RestoreInputsProtocolError:
        print("restore_inputs_protocol_error：私有参数协议无效。", file=sys.stderr)
        raise SystemExit(2)
    except Exception:
        print("restore_inputs_failed：输入推导失败，请核对绑定来源；不自动覆盖或重放。",
              file=sys.stderr)
        raise SystemExit(1)
