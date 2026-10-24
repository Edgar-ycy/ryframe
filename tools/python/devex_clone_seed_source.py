"""只读核验已发布 seed 来源登记及其固定证据。"""
from __future__ import annotations

from collections.abc import Callable
import copy
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, validate_request
from devex_clone_storage import current_storage_binding
from devex_clone_target_binding import request_binding
from restore_reference_plan import plan_hash

REGISTRATION_FIELDS = {
    "format_version", "kind", "run_directory", "run_manifest", "source_to_seed",
    "post_copy", "post_verify", "seed_registration", "seed_registration_stage",
    "capacity", "departments", "identity", "seed_handoff", "seed_close",
    "producer_lineage", "source_storage", "source_request", "generation_verified",
    "source_environment", "remote_writes", "outbox_drained", "restore_qualified",
}
STORAGE_FIELDS = {
    "format_version", "kind", "run_directory", "run_manifest", "side", "storage",
}
RESULT_FIELDS = {
    "status", "registration", "source_request", "source_storage", "generation_verified",
    "remote_writes", "outbox_drained", "restore_qualified",
}
def _validate_evidence_bindings(backend: Path, value) -> None:
    """递归复核登记中所有显式文件绑定，不把普通业务对象误作收据。"""
    if isinstance(value, dict):
        if set(value) == {"path", "bytes", "sha256"}:
            bound_file(backend, value)
            return
        for item in value.values():
            _validate_evidence_bindings(backend, item)
    elif isinstance(value, list):
        for item in value:
            _validate_evidence_bindings(backend, item)


def _published_initialization(backend: Path, manifest: dict) -> tuple[dict, dict]:
    """发布源只消费已锚定的阶段证据；当前扫描用于防漂移，不补称历史全树封存。"""
    from devex_clone_factory_context import _initialization_evidence
    from devex_clone_target_binding import target_files

    descriptor = manifest["initialized"]
    filename = bound_file(backend, descriptor)
    tree = target_files(filename.parent)
    result = _initialization_evidence(backend, filename)
    if bound_file(backend, descriptor) != filename or target_files(filename.parent) != tree:
        raise ValueError("已发布 seed 的初始化内容或当前文件树在核验期间变化")
    return result


def _registered_source(backend: Path, descriptor: dict, *, live_storage: bool,
                      validate_seed_target: Callable[[Path, dict], tuple[dict, dict]]) -> dict:
    """恢复发布源，并由调用方选择 seed 初始化请求的执行资格规则。"""
    backend = backend.resolve(strict=True)
    result_path = bound_file(backend, descriptor)
    if result_path.parent.name != "results":
        raise ValueError("seed 源外层结果不属于统一运行 results 目录")
    directory = local_path(backend, str(result_path.parent.parent))
    records = [item for item in load_state(directory)["attempts"]
               if item["stage"] == "seed-runtime" and item["mode"] == "source-register"]
    if (not records or records[-1]["status"] != "passed" or records[-1]["result"] != descriptor
            or result_path != directory / "results" / f"{records[-1]['number']:04d}.json"):
        raise ValueError("seed 源必须绑定最新已通过的 source-register 外层结果")

    result = read_json(result_path)
    exact(result, RESULT_FIELDS)
    if (result["status"] != "seed_source_registered" or result["remote_writes"] != 0
            or result["outbox_drained"] is not True or result["restore_qualified"] is not False):
        raise ValueError("seed 源外层结果没有完整排空并发布")
    registration_path = bound_file(backend, result["registration"])
    if (registration_path.name != "source-registration.json"
            or registration_path.parent.parent != directory / "seed-runtime"):
        raise ValueError("seed 源登记文件不属于原统一运行的固定 attempt")
    registration = read_json(registration_path)
    exact(registration, REGISTRATION_FIELDS)
    if (registration["format_version"] != 1
            or registration["kind"] != "devex-clone-seed-source-registration"
            or registration["run_directory"] != str(directory)
            or registration["run_manifest"] != binding(directory / "manifest.json")
            or registration["source_request"] != result["source_request"]
            or registration["source_storage"] != result["source_storage"]
            or registration["generation_verified"] != result["generation_verified"]
            or registration["remote_writes"] != 0 or registration["outbox_drained"] is not True
            or registration["restore_qualified"] is not False):
        raise ValueError("seed 源内层登记与外层发布结果不一致")
    _validate_evidence_bindings(backend, registration)

    request = read_json(bound_file(backend, registration["source_request"]))
    validate_request(backend, request)
    storage = read_json(bound_file(backend, registration["source_storage"]))
    exact(storage, STORAGE_FIELDS)
    if (storage["format_version"] != 1 or storage["kind"] != "devex-clone-seed-source-storage"
            or storage["run_directory"] != str(directory)
            or storage["run_manifest"] != registration["run_manifest"] or storage["side"] != "target"):
        raise ValueError("seed 源存储登记不属于原 run 的 target 代次")
    generation = read_json(bound_file(backend, registration["generation_verified"]))
    if (generation.get("request_sha256") != plan_hash(request)
            or generation.get("worktree_fingerprint") != request["worktree_fingerprint"]
            or generation.get("external_writers_discovered") is not False
            or generation.get("clone_verified") is not False
            or generation.get("restore_qualified") is not False):
        raise ValueError("seed 源 generation 收据不属于已发布请求")

    original_manifest = read_json(bound_file(backend, registration["run_manifest"]))
    if original_manifest.get("copy_stage") != "source_to_seed":
        raise ValueError("seed 源只能继承完整 source_to_seed 运行")
    initialization, seed_target = _published_initialization(backend, original_manifest)
    validate_seed_target(backend, seed_target)
    handoff = read_json(bound_file(backend, registration["seed_handoff"]))
    expected_source = {**copy.deepcopy(seed_target["target"]),
                       "runtime_dir": request["source"]["runtime_dir"],
                       "api_url": handoff["api_url"]}
    if (seed_target["side"] != "seed" or request["source"] != expected_source
            or request["maintenance_build"] != seed_target["maintenance_build"]
            or {"mysql": request["tools"]["mysql"]} != seed_target["tools"]
            or storage["storage"]["api_url"] != request["source"]["s3"]["endpoint"]):
        raise ValueError("seed 源请求没有精确派生自原 seed 初始化")
    environment = read_json(bound_file(backend, registration["source_environment"]))
    exact(environment, {"environment"})
    if (not isinstance(environment["environment"], dict)
            or any(not isinstance(key, str) or not isinstance(value, str)
                   for key, value in environment["environment"].items())):
        raise ValueError("seed 源环境必须是固定文本映射")
    return {"directory": directory, "result": result, "registration": registration,
            "request": request, "storage": storage, "generation": generation,
            "manifest": original_manifest, "seed_target": seed_target,
            "initialization": initialization, "environment": environment}


def _published_source(backend: Path, descriptor: dict, *, live_storage: bool,
                      validate_seed_target: Callable[[Path, dict], tuple[dict, dict]]) -> dict:
    from devex_clone_seed_rebind import resolve_storage

    source = _registered_source(backend, descriptor, live_storage=False,
                                validate_seed_target=validate_seed_target)
    state = load_state(source["directory"])
    result = resolve_storage(backend, descriptor, source, state, live_storage=live_storage,
                              current_storage=current_storage_binding)
    from devex_clone_seed_generation import resolve_generation

    result = resolve_generation(backend, descriptor, result, state, live=live_storage)
    if load_state(source["directory"]) != state:
        raise ValueError("seed 发布源核验期间阶段历史变化")
    return result


def published_source(backend: Path, descriptor: dict, *, live_storage: bool = False) -> dict:
    """从已发布外层结果恢复唯一 ready seed 源；可选复核其当前 RustFS 代次。"""
    return _published_source(backend, descriptor, live_storage=live_storage,
                             validate_seed_target=request_binding)
