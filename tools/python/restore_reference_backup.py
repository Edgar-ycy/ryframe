"""正式备份与已发布共享导出的文件、库存和来源代次绑定。"""

from __future__ import annotations

from pathlib import Path

from devex_clone_export_verify import verify_source_export
from devex_clone_model import local_path
from devex_clone_source_proof import bound_file
from reference_fixture_successor import published_source
from restore_runtime_evidence import read_json_document
from restore_reference_plan import _native


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
    identity = _native("comparison-source-export", {
        "backend": str(backend), "descriptor": document_binding(published),
    })
    source = published_source(backend, identity["review_successor"], live_storage=live)
    exported = verify_source_export(backend, identity["export"])
    request = exported["request"]
    build = bound_document(backend, request["backend_build"])
    descriptor = document_binding(published_generation)
    runtime = bound_document(backend, published_generation.value["source_runtime"])
    quiescence = bound_document(backend, published_generation.value["runtime_evidence"])
    result = _native("reference-backup-source", {
        "plan": plan, "inventory": inventory, "identity": identity, "source": source,
        "exported": exported, "build": build.value, "descriptor": descriptor,
        "runtime": runtime.value, "quiescence": quiescence.value,
    })
    for document in (*documents, build, runtime, quiescence):
        document.assert_unchanged()
    return result


def validate_backup_result(value: dict) -> None:
    _native("reference-backup-result", {"value": value})
