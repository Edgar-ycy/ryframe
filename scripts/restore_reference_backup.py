"""正式备份与已发布共享导出的文件、库存和来源代次绑定。"""

from __future__ import annotations

from pathlib import Path

from devex_clone_export_verify import verify_source_export
from devex_clone_model import exact, local_path
from devex_clone_source_proof import bound_file
from restore_comparison_source import _source_export
from restore_reference_plan import plan_hash
from restore_runtime_evidence import read_json_document


def document_binding(document) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def bound_document(backend: Path, descriptor: dict):
    document = read_json_document(bound_file(backend, descriptor))
    if document_binding(document) != descriptor:
        raise ValueError("正式恢复文件绑定在读取期间变化")
    return document


def backup_source(backend: Path, plan: dict, inventory: dict, runtime_path: Path,
                  quiescence_path: Path, export_path: Path) -> dict:
    documents = [read_json_document(local_path(backend, str(path)))
                 for path in (runtime_path, quiescence_path, export_path)]
    runtime, quiescence, published = documents
    identity = _source_export(backend, document_binding(published))
    exported = verify_source_export(backend, identity["export"])
    generation, request = exported["generation"], exported["request"]
    expected_inventory = {"id": plan["id"], **exported["inventory"]}
    source = {key: value for key, value in plan["source"].items() if key != "frontend_url"}
    build = bound_document(backend, request["backend_build"])
    if (inventory != expected_inventory or request["source"] != source
            or request["tools"] != plan["tools"] or runtime.value["build"] != build.value
            or generation["source"] != runtime.value["build"]["sources"]["full"]["source"]["snapshot"]
            or any(generation[key] != runtime.value[key]
                   for key in ("runtime", "processes", "physical_binding"))
            or runtime.value["plan_sha256"] != plan_hash(plan)
            or quiescence.value["source_runtime_sha256"] != runtime.sha256):
        raise ValueError("正式备份与共享导出的库存、配置、构建或运行代次不同")
    for document in (*documents, build):
        document.assert_unchanged()
    return {"source_export": identity, "source_runtime": document_binding(runtime),
            "source_quiescence": document_binding(quiescence)}


def validate_backup_result(value: dict) -> None:
    exact(value, {"format_version", "kind", "reference_plan", "manifest", "backup_root",
                  "artifacts", "source_export", "source_runtime", "source_quiescence"})
    if (type(value["format_version"]) is not int or value["format_version"] != 1
            or value["kind"] != "restore-reference-backup"
            or type(value["artifacts"]) is not int or value["artifacts"] <= 0):
        raise ValueError("正式备份结果版本、类型或产物数量无效")
