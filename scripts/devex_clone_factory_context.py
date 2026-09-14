"""复制目标初始化历史；不启动进程、不修改远端资源。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json, regular_file
from devex_clone_inventory import KEYS
from devex_clone_model import linked, local_path
from devex_clone_source_proof import bound_file
from devex_clone_target import context, history
from devex_clone_target_reset_evidence import reset_completed
from devex_clone_target_storage import verify_storage_generation
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
    filename = bound_file(backend, initial["receipt"])
    initial_directory = root / "inventory-initial" if inventory_directory is None else inventory_directory
    resumed_directory = filename.parent.name.startswith("inventory-resume-")
    if filename.name != "inventory.json" or (filename.parent != initial_directory and not (resumed and resumed_directory)):
        raise ValueError("初始库存必须属于本初始化代次的精确目录")
    unfailed(filename.parent, allow_resolved_failure=resumed)
    receipt = read_json(filename)
    if (receipt.get("status") != "side_inventory_captured" or receipt.get("format_version") != 1
            or receipt.get("side") != "target" or receipt.get("scope_id") != request["target"]["scope_id"]
            or receipt.get("keys") != list(KEYS) or set(receipt["observations"]) != set(KEYS)):
        raise ValueError("原始库存未完整采集本侧四个目标")
    expected_names = {f"{phase}-target-{key}.json" for phase in ("before", "after") for key in KEYS}
    if set(receipt["inventories"]) != expected_names:
        raise ValueError("初始库存必须保留完整八份原始输出")
    for name, expected in receipt["inventories"].items():
        path = local_path(backend, str(filename.parent / name))
        if file_digest(regular_file(path)) != expected:
            raise ValueError("初始库存原始文件发生变化")
    for name in ("binding-before.json", "binding-after.json"):
        value = read_json(regular_file(filename.parent / name))
        if plan_hash(value) != receipt["binding_sha256"]:
            raise ValueError("初始库存来源前后绑定变化")
    expected_images = {}
    for db in request["target"]["databases"]:
        image = receipt["observations"][db["key"]]
        if image["resource"] != {"kind": "database", "scope_id": request["target"]["scope_id"],
                                  "server_uuid": db["server_uuid"], "database": db["database"]}:
            raise ValueError("初始库存目标物理身份不同")
        expected_images[db["database"]] = image
    if initial["observations"] != expected_images:
        raise ValueError("目标初始库存与原实际采集收据不同")


def initialization_history(backend: Path, filename: Path, owned_lock_identity: int | None = None) -> tuple[dict, dict]:
    root = local_path(backend, str(filename.parent))
    from devex_clone_target import unresolved_failure
    from devex_clone_target_binding import initialized_target_files

    tree = initialized_target_files(
        backend, root, locked_guard=owned_lock_identity is not None)
    resumed = (root / "failure.json").exists() and not unresolved_failure(backend, root)
    unfailed(root, owned_lock_identity, allow_resolved_failure=resumed)
    result = read_json(filename)
    prepared = read_json(regular_file(root / "prepare.json"))
    request = read_json(bound_file(backend, prepared["request"]))
    if (result["status"] != "fresh_target_initialized" or result["controlled_generation_never_started"] is not True
            or result["prepare_sha256"] != file_digest(regular_file(root / "prepare.json"))["sha256"]
            or read_json(regular_file(root / "initialized-candidate.json")) != result
            or read_json(regular_file(root / "request.json")) != request or result["history"] != history(root)
            or result["generation"] != prepared["generation"] or prepared["request_sha256"] != plan_hash(request)
            or result["id"] != request["id"] or result["scope_id"] != request["target"]["scope_id"]):
        raise ValueError("目标初始化发布、原请求或完整准备记录已变化")
    inventory_history(backend, root, result["inventory"], request, resumed=resumed)
    reset = read_json(regular_file(root / "reset-plan.json"))
    if reset_completed(root, reset["manifest"], reset["plan_hash"]) != result["reset"]:
        raise ValueError("目标 reset 未取得本代次完整完成及释放证明")
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
    observed, resources = context(backend, request, output, run, **context_options)
    verify_storage_generation(result["generation"], observed, resources.storage_runtime_binding, resources.cache_runtime_binding)
    bound_file(backend, expected_binding)
    if binding != expected_binding or initialization_history(backend, filename, owned_lock_identity) != (result, request):
        raise ValueError("当前代次核验期间原始初始化证据变化")
    return result, request, resources
