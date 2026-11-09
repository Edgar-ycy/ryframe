"""只读核对已发布的 seed ARM 输入与原目标。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, linked, local_path, name
from devex_clone_run import target_lifecycle_binding
from devex_clone_run_state import binding, load_state
from devex_clone_seed_source import published_source
from devex_clone_seed_export import require_export_binding
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import execution_backend, request_binding
from devex_clone_tools import verify as verify_tools
from restore_reference_plan import plan_hash
from source_fingerprints import artifact_sources


REQUEST_FIELDS = {
    "format_version", "kind", "source_registration", "id", "initialized",
    "target_registration", "target_initialized_files", "target_environment",
    "copy_directory", "build_bridges",
}
SUCCESSOR_REQUEST_FIELDS = REQUEST_FIELDS | {"review_successor", "source_export", "source_export_result", "source_generation"}
RESULT_FIELDS = {
    "status", "request", "manifest", "source_registration", "source_request",
    "source_storage", "generation_verified", "initialized", "target_environment",
    "target_registration", "target_initialized_files", "target_storage_run",
    "target_side", "remote_writes", "outbox_drained", "restore_qualified",
}
SUCCESSOR_RESULT_FIELDS = RESULT_FIELDS | {"review_successor", "source_export", "source_export_result", "source_generation"}


def _target_build(backend: Path, target: dict, bridges: list[dict]) -> None:
    if not isinstance(bridges, list):
        raise ValueError("arm 目标构建桥接必须是明确列表")
    execution, _ = execution_backend(backend, target)
    with artifact_sources(backend, bridges):
        receipt = verify_tools(execution, bound_file(execution, target["maintenance_build"]))
        source = receipt.get("source")
        if (receipt.get("kind") != "devex-clone-tool-build" or not isinstance(source, dict)
                or not isinstance(source.get("worktree_fingerprint"), str)):
            raise ValueError("arm 目标维护构建缺少完整产品来源")


def _environment(backend: Path, descriptor: dict) -> dict:
    value = read_json(bound_file(backend, descriptor))
    exact(value, {"environment"})
    if (not isinstance(value["environment"], dict)
            or any(not isinstance(key, str) or not isinstance(item, str)
                   for key, item in value["environment"].items())):
        raise ValueError("arm 目标环境必须是固定文本映射")
    return value


def _successor_request(request: dict) -> bool:
    kind = request.get("kind")
    rebind = {"source_rebind"} if "source_rebind" in request else set()
    if kind == "devex-clone-seed-arm-input":
        exact(request, REQUEST_FIELDS | rebind)
        return False
    if kind == "devex-clone-seed-successor-arm-input":
        exact(request, SUCCESSOR_REQUEST_FIELDS | rebind)
        return True
    raise ValueError("arm-input 请求类型无效")


def _source(backend: Path, request: dict, *, live_storage: bool) -> dict:
    if not _successor_request(request):
        return published_source(backend, request["source_registration"], live_storage=live_storage)
    from reference_fixture_successor import published_source as successor_source

    source = successor_source(backend, request["review_successor"], live_storage=live_storage)
    relationship = source.get("review_successor")
    if (source.get("review_successor_binding") != request["review_successor"]
            or not isinstance(relationship, dict)
            or relationship.get("source_result") != request["source_registration"]):
        raise ValueError("successor arm 来源未绑定请求指定的 C52 与 successor")
    return source


def _successor_target(source: dict, lifecycle: dict, target: dict, side: str) -> None:
    relationship = source["review_successor"]
    expected = relationship["requests"].get(side)
    actual = lifecycle["registration"].get("request")
    if (not isinstance(expected, dict) or set(expected) != {"path", "bytes", "sha256", "canonical_sha256"}
            or actual != {key: expected[key] for key in ("path", "bytes", "sha256")}
            or plan_hash(target) != expected["canonical_sha256"]
            or target.get("review") != relationship["successor_review"]):
        raise ValueError("successor arm 目标不是 relationship 对应侧的精确 fresh 请求")


def _validate_arm_request(
    backend: Path,
    request: dict,
    source: dict,
    *,
    live_storage: bool,
    require_new_copy_directory: bool = True,
) -> dict:
    successor = _successor_request(request)
    if request.get("source_rebind") != source.get("source_rebind"):
        raise ValueError("arm 请求必须冻结来源当前的显式重绑定收据")
    if "source_rebind" in request:
        bound_file(backend, request["source_rebind"])
    if successor:
        if source.get("source_generation") is None or request["source_generation"] != source["source_generation"]:
            raise ValueError("successor arm 必须绑定同一已发布 source-generation")
        require_export_binding(backend, source, request)
    if request["format_version"] != 1:
        raise ValueError("arm-input 请求类型无效")
    name(request["id"])
    lifecycle = target_lifecycle_binding(backend, request, live_storage=live_storage)
    initialized, target = lifecycle["initialized"], lifecycle["target"]
    target_review, _ = request_binding(backend, target)
    side = target["side"]
    if successor:
        _successor_target(source, lifecycle, target, side)
        source_review = None
    else:
        source_review, _ = request_binding(backend, source["seed_target"])
    source_config, target_config = source["request"]["source"], target["target"]
    source_databases = {(item["server_uuid"], item["database"].lower())
                        for item in source_config["databases"]}
    target_databases = {(item["server_uuid"], item["database"].lower())
                        for item in target_config["databases"]}
    if (side not in {"base", "candidate"}
            or not successor and target_review != source_review
            or source["seed_target"]["side"] != "seed"
            or target_config["scope_id"] == source_config["scope_id"]
            or source_databases & target_databases):
        raise ValueError("arm 目标不属于同一审阅计划的独立 base/candidate 侧")
    _environment(backend, request["target_environment"])
    copy_directory = local_path(
        backend, request["copy_directory"], new=require_new_copy_directory
    )
    invalid_existing = copy_directory.exists() and (
        linked(copy_directory) or not copy_directory.is_dir()
    )
    if (
        require_new_copy_directory and copy_directory.exists()
        or invalid_existing
        or not copy_directory.parent.is_dir()
    ):
        raise ValueError("arm 复制目录必须是已有父目录下的明确受控路径")
    _target_build(backend, target, request["build_bridges"])
    return {"source": source, "initialized": initialized, "target": target,
            "target_registration": lifecycle["registration"],
            "target_storage_run": lifecycle["target_storage_run"],
            "target_side": side, "copy_directory": copy_directory}


def validate_arm_request(backend: Path, request: dict, *, live_storage: bool) -> dict:
    """核对 arm 请求绑定的来源、fresh target、环境和构建证据，不取得运行锁。"""
    source = _source(backend, request, live_storage=live_storage)
    return _validate_arm_request(backend, request, source, live_storage=live_storage)


def verify_published_arm_input(backend: Path, result_path: Path, side: str) -> dict:
    """只读重建已发布 successor arm-input；允许复制目录尚未创建或已经创建。"""
    if side not in {"base", "candidate"}:
        raise ValueError("必须显式选择 base 或 candidate arm")
    backend = backend.resolve(strict=True)
    path = local_path(backend, str(result_path))
    original = binding(path)
    result = read_json(bound_file(backend, original))
    fields = SUCCESSOR_RESULT_FIELDS | (
        {"source_rebind"} if "source_rebind" in result else set()
    )
    exact(result, fields)
    if (
        result["status"] != "seed_arm_input_published"
        or result["target_side"] != side
        or result["remote_writes"] != 0
        or result["outbox_drained"] is not True
        or result["restore_qualified"] is not False
    ):
        raise ValueError("successor arm-input 外层结果状态或目标侧无效")
    request = read_json(bound_file(backend, result["request"]))
    if not _successor_request(request):
        raise ValueError("正式恢复只接受绑定共享 source-export 的 successor arm-input")
    source = _source(backend, request, live_storage=False)
    inputs = _validate_arm_request(
        backend,
        request,
        source,
        live_storage=False,
        require_new_copy_directory=False,
    )
    manifest_path = bound_file(backend, result["manifest"])
    manifest = read_json(manifest_path)
    if manifest != _manifest(request, inputs):
        raise ValueError("successor arm-input 清单与当前请求、来源或目标不一致")
    from devex_clone_run import manifest as validate_manifest

    validate_manifest(backend, manifest)
    expected = {
        "source_registration": request["source_registration"],
        "source_request": source["source_request"],
        "source_storage": source["registration"]["source_storage"],
        "generation_verified": source["generation_verified"],
        "initialized": request["initialized"],
        "target_environment": request["target_environment"],
        "target_registration": request["target_registration"],
        "target_initialized_files": request["target_initialized_files"],
        "target_storage_run": inputs["target_storage_run"],
        "target_side": inputs["target_side"],
        "review_successor": request["review_successor"],
        "source_export": request["source_export"],
        "source_export_result": request["source_export_result"],
        "source_generation": request["source_generation"],
    }
    if "source_rebind" in request:
        expected["source_rebind"] = request["source_rebind"]
    if any(result.get(key) != value for key, value in expected.items()):
        raise ValueError("successor arm-input 外层结果未完整绑定当前来源与 fresh 目标")
    if (
        binding(path) != original
        or bound_file(backend, result["request"]) != Path(result["request"]["path"])
        or bound_file(backend, result["manifest"]) != manifest_path
    ):
        raise ValueError("successor arm-input 在核验期间发生变化")
    return {
        "binding": original,
        "result": copy.deepcopy(result),
        "request": copy.deepcopy(request),
        "manifest": copy.deepcopy(manifest),
        "source": source,
        "target": inputs["target"],
        "initialized": inputs["initialized"],
        "target_storage_run": inputs["target_storage_run"],
        "target_side": side,
    }


def _manifest(request: dict, inputs: dict) -> dict:
    source = inputs["source"]
    result = {
        "format_version": 1,
        "kind": "devex-clone-run",
        "id": request["id"],
        "source_request": copy.deepcopy(source["source_request"]),
        "source_export": None,
        "initialized": copy.deepcopy(request["initialized"]),
        "source_environment": copy.deepcopy(source["source_environment"]),
        "target_environment": copy.deepcopy(request["target_environment"]),
        "copy_directory": str(inputs["copy_directory"]),
        "copy_stage": "seed_to_arm",
        "build_bridges": copy.deepcopy(request["build_bridges"]),
        "source_registration": copy.deepcopy(request["source_registration"]),
        "target_registration": copy.deepcopy(request["target_registration"]),
        "target_initialized_files": copy.deepcopy(request["target_initialized_files"]),
        "target_storage_run": copy.deepcopy(inputs["target_storage_run"]),
    }
    if _successor_request(request):
        result["review_successor"] = copy.deepcopy(request["review_successor"])
        result["source_export"] = copy.deepcopy(request["source_export"])
        result["source_export_result"] = copy.deepcopy(request["source_export_result"])
        result["source_generation"] = copy.deepcopy(request["source_generation"])
    if "source_rebind" in request:
        result["source_rebind"] = copy.deepcopy(request["source_rebind"])
    return result
