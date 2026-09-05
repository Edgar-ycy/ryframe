"""串行复制中的明确环境和初始化历史；不启动进程、不修改远端资源。"""
from __future__ import annotations

from contextlib import contextmanager
import copy
import os
from pathlib import Path
import threading

from devex_clone_capture import read_json, regular_file
from devex_clone_inventory import KEYS
from devex_clone_model import linked, local_path
from devex_clone_source_proof import bound_file
from devex_clone_target import context, history, reset_completed
from devex_clone_target_storage import verify_storage_generation
from devex_clone_transfer import DatabaseObservation
from restore_build import file_digest
from restore_reference_plan import plan_hash

_ENVIRONMENT_LOCK = threading.RLock()
_SYSTEM_NAMES = {
    "APPDATA", "COMMONPROGRAMFILES", "COMMONPROGRAMFILES(X86)", "COMMONPROGRAMW6432", "COMSPEC",
    "HOME", "HOMEDRIVE", "HOMEPATH", "LANG", "LOCALAPPDATA", "NUMBER_OF_PROCESSORS", "OS", "PATH",
    "PATHEXT", "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432", "SHELL", "SYSTEMDRIVE",
    "SYSTEMROOT", "TEMP", "TMP", "TMPDIR", "USER", "USERPROFILE", "WINDIR",
}


def configured(private: dict, inherited: dict | None = None) -> dict:
    """将私有侧配置叠加到安全的进程基础环境，不继承业务配置、凭据或代理。"""
    inherited = os.environ if inherited is None else inherited
    for values in (private, inherited):
        if (not isinstance(values, dict) and values is not os.environ) or any(
                not isinstance(key, str) or not isinstance(value, str) for key, value in values.items()):
            raise ValueError("复制环境必须是文本映射")
    folded = [key.upper() for key in private]
    if len(folded) != len(set(folded)):
        raise ValueError("私有环境包含大小写重复字段")
    private_names = set(folded)
    safe = {}
    for key, value in inherited.items():
        upper = key.upper()
        if upper in private_names:
            continue
        if upper in _SYSTEM_NAMES or upper.startswith("PROCESSOR_") or upper.startswith("LC_"):
            safe[key] = value
    safe.update(private)
    return safe


class Environments:
    """独立 CLI 串行切换两侧配置；阻止同一 factory 跨线程混用环境。"""
    def __init__(self, source: dict, target: dict):
        self.creator = threading.get_ident()
        self.values = {"source": copy.deepcopy(dict(source)), "target": copy.deepcopy(dict(target))}
        if any(not isinstance(key, str) or not isinstance(value, str)
               for environment in self.values.values() for key, value in environment.items()):
            raise ValueError("复制两侧环境必须是明确的文本映射")
        self.bindings = {side: plan_hash(value) for side, value in self.values.items()}

    @contextmanager
    def use(self, side: str):
        if threading.get_ident() != self.creator or side not in self.values:
            raise ValueError("同一复制代次只允许创建线程串行使用明确侧环境")
        if plan_hash(self.values[side]) != self.bindings[side]:
            raise ValueError("复制侧环境在登记后变化")
        with _ENVIRONMENT_LOCK:
            original = dict(os.environ)
            installed = False
            try:
                os.environ.clear()
                os.environ.update(self.values[side])
                installed = True
                yield self.values[side]
            finally:
                changed = installed and (dict(os.environ) != self.values[side] or plan_hash(self.values[side]) != self.bindings[side])
                os.environ.clear()
                os.environ.update(original)
                if changed:
                    raise ValueError("复制命令期间环境发生漂移，已恢复调用方环境")


def database_observation(value: dict) -> DatabaseObservation:
    return DatabaseObservation(**{**copy.deepcopy(value), "all_tables": tuple(value["all_tables"]),
                                  "ownership": tuple(copy.deepcopy(value["ownership"]))})


def unfailed(directory: Path, owned_lock_identity: int | None = None) -> None:
    failure = directory / "failure.json"
    if failure.exists() or linked(failure):
        raise ValueError("目标初始化或库存失败，不能消费残留成功文件")
    lock = directory / "initialize.lock"
    if owned_lock_identity is None:
        if lock.exists() or linked(lock):
            raise ValueError("目标仍被其他调用方持锁")
    elif (type(owned_lock_identity) is not int or owned_lock_identity <= 0 or linked(lock)
            or not lock.is_dir() or lock.stat().st_ino != owned_lock_identity):
        raise ValueError("当前调用方持有的真实目标锁缺失或身份变化")


def inventory_history(backend: Path, root: Path, initial: dict, request: dict) -> None:
    filename = bound_file(backend, initial["receipt"])
    if filename != root / "inventory-initial/inventory.json":
        raise ValueError("初始库存必须属于本初始化代次的精确目录")
    unfailed(filename.parent)
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
    unfailed(root, owned_lock_identity)
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
    inventory_history(backend, root, result["inventory"], request)
    reset = read_json(regular_file(root / "reset-plan.json"))
    if reset_completed(root, reset["manifest"], reset["plan_hash"]) != result["reset"]:
        raise ValueError("目标 reset 未取得本代次完整完成及释放证明")
    return result, request


def target_history(backend: Path, binding: dict, output: Path, run, *,
                   owned_lock_identity: int | None = None, storage_run: Path | None = None) -> tuple[dict, dict, object]:
    """每次复核同一初始化历史及当前代次；复制后的业务行像由步骤观察单独检查。"""
    expected_binding = copy.deepcopy(binding)
    filename = bound_file(backend, expected_binding)
    if filename.name != "initialized.json":
        raise ValueError("目标必须绑定显式初始化入口的最终收据")
    result, request = initialization_history(backend, filename, owned_lock_identity)
    observed, resources = context(backend, request, output, run, storage_run=storage_run,
                                  owned_lock_identity=owned_lock_identity)
    verify_storage_generation(result["generation"], observed, resources.storage_runtime_binding, resources.cache_runtime_binding)
    bound_file(backend, expected_binding)
    if binding != expected_binding or initialization_history(backend, filename, owned_lock_identity) != (result, request):
        raise ValueError("当前代次核验期间原始初始化证据变化")
    return result, request, resources
