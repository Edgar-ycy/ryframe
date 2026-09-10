"""把已发布 seed 源与明确 fresh 目标组装为下一轮统一复制清单。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, linked, local_path, name
from devex_clone_run import _require_owned_run, target_lifecycle_binding, target_storage_control
from devex_clone_run_state import binding, load_state
from devex_clone_seed_source import published_source
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
SUCCESSOR_REQUEST_FIELDS = REQUEST_FIELDS | {"review_successor"}
RESULT_FIELDS = {
    "status", "request", "manifest", "source_registration", "source_request",
    "source_storage", "generation_verified", "initialized", "target_environment",
    "target_registration", "target_initialized_files", "target_storage_run",
    "target_side", "remote_writes", "outbox_drained", "restore_qualified",
}
SUCCESSOR_RESULT_FIELDS = RESULT_FIELDS | {"review_successor"}
EVIDENCE_SEQUENCE = ("request.json", "manifest.json")
EVIDENCE_FILES = set(EVIDENCE_SEQUENCE)


def _target_build(backend: Path, target: dict, bridges: list[dict]) -> None:
    if not isinstance(bridges, list):
        raise ValueError("arm 目标构建桥接必须是明确列表")
    execution, _ = execution_backend(backend, target)
    with artifact_sources(execution, bridges):
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
    if kind == "devex-clone-seed-arm-input":
        exact(request, REQUEST_FIELDS)
        return False
    if kind == "devex-clone-seed-successor-arm-input":
        exact(request, SUCCESSOR_REQUEST_FIELDS)
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


def _inputs(backend: Path, directory: Path | None, request: dict, *, live_storage: bool) -> dict:
    successor = _successor_request(request)
    if request["format_version"] != 1:
        raise ValueError("arm-input 请求类型无效")
    name(request["id"])
    source = _source(backend, request, live_storage=live_storage)
    if successor:
        if directory is None:
            raise ValueError("successor arm 必须在 C52 来源 run 的当前控制锁内执行")
        _require_owned_run(directory)
        if (source.get("directory") != directory
                or source["registration"].get("run_directory") != str(directory)):
            raise ValueError("successor arm 的 C52 来源不属于当前 held run")
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
    copy_directory = local_path(backend, request["copy_directory"], new=True)
    if copy_directory.exists() or linked(copy_directory) or not copy_directory.parent.is_dir():
        raise ValueError("arm 复制目录必须是已有父目录下的明确新路径")
    _target_build(backend, target, request["build_bridges"])
    return {"source": source, "initialized": initialized, "target": target,
            "target_registration": lifecycle["registration"],
            "target_storage_run": lifecycle["target_storage_run"],
            "target_side": side, "copy_directory": copy_directory}


def _manifest(request: dict, inputs: dict) -> dict:
    source = inputs["source"]
    result = {
        "format_version": 1,
        "kind": "devex-clone-run",
        "id": request["id"],
        "source_request": copy.deepcopy(source["registration"]["source_request"]),
        "source_export": None,
        "initialized": copy.deepcopy(request["initialized"]),
        "source_environment": copy.deepcopy(source["registration"]["source_environment"]),
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
    return result


def _published(backend: Path, directory: Path, number: int) -> list[dict]:
    results = []
    for attempt in load_state(directory)["attempts"]:
        if (attempt["number"] == number or attempt["stage"] != "seed-runtime"
                or attempt["mode"] != "arm-input" or attempt["status"] != "passed"):
            continue
        value = read_json(bound_file(backend, attempt["result"]))
        request = read_json(bound_file(backend, value.get("request", {})))
        successor = _successor_request(request)
        fields = SUCCESSOR_RESULT_FIELDS if successor else RESULT_FIELDS
        exact(value, fields)
        if (value["status"] != "seed_arm_input_published"
                or value["source_registration"] != request["source_registration"]
                or value.get("review_successor") != request.get("review_successor")):
            raise ValueError("历史 arm-input 外层结果类型无效")
        results.append(value)
    if len({item["target_side"] for item in results}) != len(results):
        raise ValueError("同一 seed 源出现重复 arm 目标侧")
    return results


def _existing_output(directory: Path, number: int) -> Path | None:
    found = []
    for attempt in load_state(directory)["attempts"]:
        if (attempt["number"] >= number or attempt["stage"] != "seed-runtime"
                or attempt["mode"] != "arm-input" or attempt["status"] != "failed"):
            continue
        path = directory / "seed-runtime" / f"attempt-{attempt['number']:04d}"
        if not path.exists() and not linked(path):
            continue
        if linked(path) or not path.is_dir() or any(item.is_dir() or linked(item) for item in path.iterdir()):
            raise ValueError("历史 arm-input 输出不是无链接普通文件目录")
        names = {item.name for item in path.iterdir()}
        prefixes = {frozenset(EVIDENCE_SEQUENCE[:index])
                    for index in range(len(EVIDENCE_SEQUENCE) + 1)}
        if names - EVIDENCE_FILES or frozenset(names) not in prefixes:
            raise ValueError("历史 arm-input 含未知或非前缀局部证据")
        found.append(path)
    if len(found) > 1:
        raise ValueError("多个历史 arm-input 目录可能含交接，不能猜测")
    return found[0] if found else None


def _write_or_match(path: Path, value: dict) -> None:
    if path.exists() or linked(path):
        if linked(path) or not path.is_file() or read_json(path) != value:
            raise ValueError("arm-input 已有局部证据不同，须保留现场核对")
        return
    write_json(path, value)


def publish_arm_input(backend: Path, directory: Path, request_file: Path,
                      number: int) -> dict:
    """只发布下一 run 清单；不初始化、重启、导出或复制任何远端资源。"""
    backend, directory = backend.resolve(strict=True), local_path(backend, str(directory))
    request_path = local_path(backend, str(request_file))
    original = binding(request_path)
    request = read_json(request_path)
    successor = _successor_request(request)
    initial = _inputs(backend, directory if successor else None, request, live_storage=False)
    manifest = _manifest(request, initial)
    from devex_clone_run import inherited_build_bridges, manifest as validate_manifest

    with target_storage_control(backend, directory, manifest):
        before = _inputs(backend, directory if successor else None, request, live_storage=True)
        if before != initial or _manifest(request, before) != manifest:
            raise ValueError("arm-input 取得存储锁后目标或源证据变化")
        validate_manifest(backend, manifest)
        inherited_build_bridges(backend, manifest)
        prior = _published(backend, directory, number)
        if any(item["source_registration"] != request["source_registration"] for item in prior):
            raise ValueError("同一 seed run 的 arm 目标必须继承同一已发布源")
        if any(item.get("review_successor") != request.get("review_successor") for item in prior):
            raise ValueError("同一 seed run 的 arm 目标必须继承同一 review successor")
        if any(item["target_side"] == before["target_side"] for item in prior):
            raise ValueError("同一 seed run 已发布该 arm 目标侧")
        output = _existing_output(directory, number)
        if output is None:
            output = directory / "seed-runtime" / f"attempt-{number:04d}"
            if output.exists() or linked(output) or not output.parent.is_dir():
                raise ValueError("arm-input 输出目录不为空或父目录无效")
            output.mkdir()
        _write_or_match(output / "request.json", request)
        _write_or_match(output / "manifest.json", manifest)

        after = _inputs(backend, directory if successor else None, request, live_storage=True)
        if (after != before or _manifest(request, after) != manifest
                or binding(request_path) != original):
            raise ValueError("arm-input 发布期间源、目标、存储或请求发生变化")
    result = {
        "status": "seed_arm_input_published",
        "request": binding(output / "request.json"),
        "manifest": binding(output / "manifest.json"),
        "source_registration": copy.deepcopy(request["source_registration"]),
        "source_request": copy.deepcopy(before["source"]["registration"]["source_request"]),
        "source_storage": copy.deepcopy(before["source"]["registration"]["source_storage"]),
        "generation_verified": copy.deepcopy(before["source"]["registration"]["generation_verified"]),
        "initialized": copy.deepcopy(request["initialized"]),
        "target_environment": copy.deepcopy(request["target_environment"]),
        "target_registration": copy.deepcopy(request["target_registration"]),
        "target_initialized_files": copy.deepcopy(request["target_initialized_files"]),
        "target_storage_run": copy.deepcopy(before["target_storage_run"]),
        "target_side": before["target_side"],
        "remote_writes": 0,
        "outbox_drained": True,
        "restore_qualified": False,
    }
    if successor:
        result["review_successor"] = copy.deepcopy(request["review_successor"])
    exact(result, SUCCESSOR_RESULT_FIELDS if successor else RESULT_FIELDS)
    return result
