"""正式备份与已发布共享导出的文件、库存和来源代次绑定。"""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path

from devex_clone_export_verify import verify_source_export
from devex_clone_model import exact, local_path
from devex_clone_source_proof import bound_file
from restore_comparison_source import _source_export
from reference_fixture_successor import published_source
from restore_runtime_evidence import read_json_document, timestamp


def document_binding(document) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def bound_document(backend: Path, descriptor: dict):
    document = read_json_document(bound_file(backend, descriptor))
    if document_binding(document) != descriptor:
        raise ValueError("正式恢复文件绑定在读取期间变化")
    return document


def backup_source(backend: Path, plan: dict, inventory: dict, generation_path: Path,
                  export_path: Path, *, live: bool = False) -> dict:
    documents = [read_json_document(local_path(backend, str(path)))
                 for path in (generation_path, export_path)]
    published_generation, published = documents
    identity = _source_export(backend, document_binding(published))
    source = published_source(backend, identity["review_successor"], live_storage=live)
    exported = verify_source_export(backend, identity["export"])
    generation, request = exported["generation"], exported["request"]
    expected_inventory = {"id": plan["id"], **exported["inventory"]}
    selected = plan["source"]
    build = bound_document(backend, request["backend_build"])
    descriptor = document_binding(published_generation)
    if (identity["source_generation"] != descriptor or source["source_generation"] != descriptor
            or inventory != expected_inventory or request["source"] != selected
            or request["tools"] != plan["tools"] or request != source["request"] or generation != source["generation"]
            or generation["source"] != build.value["sources"]["full"]["source"]["snapshot"]):
        raise ValueError("正式备份与共享导出的库存、配置、构建或运行代次不同")
    runtime = bound_document(backend, published_generation.value["source_runtime"])
    quiescence = bound_document(backend, published_generation.value["runtime_evidence"])
    times = [datetime.fromisoformat(timestamp(value, "来源备份时间").replace("Z", "+00:00")) for value in (
        runtime.value["verified_at"], quiescence.value["observed_stopped_at"], inventory["quiesced_at"], inventory["captured_at"])]
    if times != sorted(times) or times[-1] > datetime.now(timezone.utc):
        raise ValueError("同代源验证、实际停止观察和正式库存采集时间顺序不成立")
    for document in (*documents, build, runtime, quiescence):
        document.assert_unchanged()
    return {"source_export": identity, "source_generation": descriptor}


def validate_backup_result(value: dict) -> None:
    exact(value, {"format_version", "kind", "reference_plan", "manifest", "backup_root",
                  "artifacts", "source_export", "source_generation"})
    if (type(value["format_version"]) is not int or value["format_version"] != 2
            or value["kind"] != "restore-reference-backup"
            or type(value["artifacts"]) is not int or value["artifacts"] <= 0):
        raise ValueError("正式备份结果版本、类型或产物数量无效")
