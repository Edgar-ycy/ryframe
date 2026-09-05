"""显式只读导出开发性能测试的源数据；不初始化、修改源或目标，不登记备份恢复。"""
from __future__ import annotations

import argparse
import datetime as dt
import os
from pathlib import Path
import subprocess

from devex_clone import read_json
from devex_clone_capture import regular_file, write_json
from devex_clone_export import (capture_inventory, dump_databases, inventory_object_index, logical_inventory,
                                schema_models, schema_snapshot, validate_dump_state, verify_migrations)
from devex_clone_object_batch import capture_objects
from devex_clone_model import local_path
from devex_clone_source_proof import verify_generation
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import plan_hash


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def unchanged_generation(backend: Path, request: dict, original: dict, run) -> dict:
    actual = verify_generation(backend, request, run)
    if actual != original:
        raise ValueError("源导出期间源码、构建、配置、producer 或工具代次变化")
    return actual


def unchanged_exports(output: Path, dumps: list, objects: list) -> None:
    bindings = [item["artifact"] for item in dumps]
    bindings.extend(item[field] for bucket in objects for item in bucket["entries"] for field in ("artifact", "capture"))
    for binding in bindings:
        path = regular_file(output / binding["file"])
        if file_digest(path) != {key: binding[key] for key in ("bytes", "sha256")}:
            raise ValueError("源导出本地文件在完整验证前变化")


def export_source(backend: Path, request_path: Path, output: Path, run=subprocess.run) -> dict:
    output = local_path(backend, str(output), new=True)
    request_path = local_path(backend, str(request_path))
    if not output.parent.is_dir() or output == request_path:
        raise ValueError("源导出使用已有父目录中的新独立证据目录")
    output.mkdir()
    stage, request_digest = "request", None
    try:
        request_digest = file_digest(request_path)
        request = read_json(request_path)
        write_json(output / "request.json", request)
        stage = "source_generation_before"
        before = verify_generation(backend, request, run)
        write_json(output / "generation-before.json", before)
        tools = ExternalTools({"source": request["source"], "tools": request["tools"]}, output, run)
        stage = "source_identity_and_schema"
        tools.verify_databases("source")
        tools.verify_objects("source")
        models = schema_models(backend)
        verify_migrations(tools, backend, before["maintenance"], "migrate-before")
        schema_before = schema_snapshot(tools, models, "before")
        write_json(output / "schema-before.json", {"databases": schema_before})
        unchanged_generation(backend, request, before, run)
        stage, quiesced = "inventory_before", now()
        inventory_before = capture_inventory(tools, backend, request, before, models, "before", quiesced)
        stage = "dump_databases"
        dumps = dump_databases(tools, inventory_before, models)
        unchanged_generation(backend, request, before, run)
        stage = "source_row_state_before_objects"
        schedules_before = validate_dump_state(tools, models, dumps, inventory_object_index(inventory_before))
        stage = "capture_objects"
        objects, object_index = capture_objects(tools, request, inventory_before)
        stage = "source_row_state"
        schedules = validate_dump_state(tools, models, dumps, object_index)
        if schedules != schedules_before:
            raise ValueError("源调度行在对象抓取前后变化，拒绝不同 SQL 快照")
        stage = "inventory_after"
        unchanged_generation(backend, request, before, run)
        inventory_after = capture_inventory(tools, backend, request, before, models, "after", quiesced)
        if logical_inventory(inventory_before) != logical_inventory(inventory_after):
            raise ValueError("源前后逻辑表摘要、placement 或对象清单变化，拒绝拼接不同快照")
        stage = "source_generation_after"
        tools.verify_databases("source")
        tools.verify_objects("source")
        verify_migrations(tools, backend, before["maintenance"], "migrate-after")
        schema_after = schema_snapshot(tools, models, "after")
        write_json(output / "schema-after.json", {"databases": schema_after})
        if schema_before != schema_after:
            raise ValueError("源 schema 在导出期间变化")
        after = unchanged_generation(backend, request, before, run)
        write_json(output / "generation-after.json", after)
        unchanged_exports(output, dumps, objects)
        if file_digest(request_path) != request_digest:
            raise ValueError("源导出请求文件在执行期间变化")
        result = {"format_version": 1, "kind": "devex-clone-source-export", "status": "source_export_captured",
                  "id": request["id"], "scope_id": request["source"]["scope_id"], "request": {"path": str(request_path), **request_digest},
                  "source_snapshot": before["source"], "worktree_fingerprint": request["worktree_fingerprint"],
                  "source": request["source"], "artifact_root": str(output), "databases": dumps, "objects": objects,
                  "enabled_system_schedule_rows": schedules, "logical_inventory_sha256": plan_hash(logical_inventory(inventory_before)),
                  "evidence": {name: {"file": name + ".json", **file_digest(output / (name + ".json"))} for name in
                               ("request", "generation-before", "generation-after", "inventory-before", "inventory-after", "schema-before", "schema-after")},
                  "catalog_sha256": plan_hash({"control": models[0], "tenant": models[1]}),
                  "quiesced_at": quiesced, "completed_at": now(), "requires_offline_plan": True,
                  "operator_declared_producers_only": True, "remote_writes": 0, "clone_verified": False, "restore_qualified": False,
                  "dump_semantic_digest_verified": False,
                  "dump_validation": "固定外部工具与明确源连接、完整列、逐表行数、文件 SHA；实际目标导入后仍须匹配源 inventory 逻辑摘要"}
        stage = "publish_export"
        write_json(output / "export.json", result)
        return result
    except Exception as error:
        write_json(output / "failure.json", {"format_version": 1, "status": "source_export_failed", "stage": stage,
                   "error_type": type(error).__name__, "request_file": str(request_path), "request_digest": request_digest,
                   "reason": redact_object_diagnostic(str(error), os.environ) if isinstance(error, ValueError) else "阶段未完成，请核对对应诊断文件",
                   "stdout": redact_object_diagnostic(getattr(error, "stdout", b""), os.environ),
                   "stderr": redact_object_diagnostic(getattr(error, "stderr", b""), os.environ),
                   "clone_verified": False, "restore_qualified": False})
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--write", action="store_true", required=True, help="仅显式写入本地新证据，所有远端操作只读")
    args = parser.parse_args()
    try:
        result = export_source(args.backend_dir.resolve(strict=True), args.request, args.output_dir)
        print(f"status={result['status']} remote_writes=0 clone_verified=false restore_qualified=false")
        return 0
    except Exception as error:
        print(f"status=source_export_failed error_type={type(error).__name__}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
