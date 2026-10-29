"""复制目标初始化历史；不启动进程、不修改远端资源。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json, regular_file
from devex_clone_model import linked, local_path
from devex_clone_source_proof import bound_file
from devex_clone_target import context
from devex_clone_target_evidence import evidence
from devex_clone_transfer import DatabaseObservation
from restore_build import file_digest
from restore_reference_plan import plan_hash


def database_observation(value: dict) -> DatabaseObservation:
    return DatabaseObservation(**{**copy.deepcopy(value), "all_tables": tuple(value["all_tables"]),
                                  "ownership": tuple(copy.deepcopy(value["ownership"]))})


def unfailed(directory: Path, owned_lock_identity: int | None = None, *, allow_resolved_failure: bool = False) -> None:
    failure = directory / "failure.json"
    if linked(failure) or (failure.exists() and not allow_resolved_failure):
        raise ValueError("目标初始化或库存失败，不能消费残留成功文件")
    lock = directory / "initialize.lock"
    if owned_lock_identity is None:
        if lock.exists() or linked(lock):
            raise ValueError("目标仍被其他调用方持锁")
    elif (type(owned_lock_identity) is not int or owned_lock_identity <= 0 or linked(lock)
            or not lock.is_dir() or lock.stat().st_ino != owned_lock_identity):
        raise ValueError("当前调用方持有的真实目标锁缺失或身份变化")


def inventory_history(backend: Path, root: Path, initial: dict, request: dict, *, resumed: bool = False,
                      inventory_directory: Path | None = None) -> None:
    evidence("inventory-history", backend, root, initial=initial, request=request, resumed=resumed,
             inventory_directory=None if inventory_directory is None else str(inventory_directory))


def _initialization_evidence(backend: Path, filename: Path, owned_lock_identity: int | None = None) -> tuple[dict, dict]:
    """核验同一初始化内容、阶段及资源前后像；调用方负责其发布资格与文件树边界。"""
    root = local_path(backend, str(filename.parent))
    from devex_clone_target import unresolved_failure

    resumed = (root / "failure.json").exists() and not unresolved_failure(backend, root)
    unfailed(root, owned_lock_identity, allow_resolved_failure=resumed)
    result = read_json(filename)
    prepared = read_json(regular_file(root / "prepare.json"))
    request = read_json(bound_file(backend, prepared["request"]))
    if (result["status"] != "fresh_target_initialized" or result["controlled_generation_never_started"] is not True
            or result["prepare_sha256"] != file_digest(regular_file(root / "prepare.json"))["sha256"]
            or read_json(regular_file(root / "initialized-candidate.json")) != result
            or read_json(regular_file(root / "request.json")) != request or result["history"] != evidence("history", backend, root)
            or result["generation"] != prepared["generation"] or prepared["request_sha256"] != plan_hash(request)
            or result["id"] != request["id"] or result["scope_id"] != request["target"]["scope_id"]):
        raise ValueError("目标初始化发布、原请求或完整准备记录已变化")
    inventory_history(backend, root, result["inventory"], request, resumed=resumed)
    if evidence("reset-completed", backend, root) != result["reset"]:
        raise ValueError("目标 reset 未取得本代次完整完成及释放证明")
    return result, request


def initialization_history(backend: Path, filename: Path, owned_lock_identity: int | None = None) -> tuple[dict, dict]:
    """当前 fresh 目标始终要求 CLI 发布的完整树与工作区谱系，不按文件缺失降级。"""
    from devex_clone_target_binding import initialized_target_files

    root = local_path(backend, str(filename.parent))
    tree = initialized_target_files(backend, root, locked_guard=owned_lock_identity is not None)
    result, request = _initialization_evidence(backend, filename, owned_lock_identity)
    if initialized_target_files(
            backend, root, locked_guard=owned_lock_identity is not None) != tree:
        raise ValueError("目标初始化完整文件树在历史核验期间变化")
    return result, request


def target_history(backend: Path, binding: dict, output: Path, run, *,
                   owned_lock_identity: int | None = None, storage_run: Path | None = None) -> tuple[dict, dict, object]:
    """每次复核同一初始化历史及当前代次；复制后的业务行像由步骤观察单独检查。"""
    expected_binding = copy.deepcopy(binding)
    filename = bound_file(backend, expected_binding)
    if filename.name != "initialized.json":
        raise ValueError("目标必须绑定显式初始化入口的最终收据")
    result, request = initialization_history(backend, filename, owned_lock_identity)
    context_options = {"storage_run": storage_run,
                       "owned_lock_identity": owned_lock_identity}
    if owned_lock_identity is not None:
        context_options["lock_output"] = filename.parent
    descriptor = read_json(filename.parent / "prepare.json")["request"]
    observed, resources = context(backend, request, output, run, request_descriptor=descriptor, **context_options)
    evidence("storage-generation", backend, output, expected=result["generation"], actual=observed,
             runtime=resources.storage_runtime_binding, cache_runtime=resources.cache_runtime_binding)
    bound_file(backend, expected_binding)
    if binding != expected_binding or initialization_history(backend, filename, owned_lock_identity) != (result, request):
        raise ValueError("当前代次核验期间原始初始化证据变化")
    return result, request, resources
