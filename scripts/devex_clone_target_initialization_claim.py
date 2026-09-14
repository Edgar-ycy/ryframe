"""首次 fresh 初始化的锁内最终文件树闭合。"""
from pathlib import Path

from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_target_binding import target_files, validate_initialization_delta
from devex_clone_target_prepared_files import validate_prepared_tree
from restore_build import file_digest


def prepared_baseline(backend: Path, output: Path, request_descriptor: dict,
                      request: dict) -> list[dict]:
    """在首次资源访问前确认 prepare 树恰好是已声明的只读证据。"""
    files = target_files(output, ignored={"initialize.lock"}, locked_guard=True)
    validate_prepared_tree(
        backend, output, files, request_descriptor, request, locked_guard=True)
    return files


def _inventory_paths(output: Path, result: dict,
                     evidence_files: tuple[dict, ...]) -> set[str]:
    directory = output / "inventory-initial"
    if not isinstance(evidence_files, tuple) or result.get("receipt") != binding(
            directory / "inventory.json"):
        raise ValueError("首次初始化库存没有返回精确文件闭合集")
    paths = {"inventory-initial"}
    seen = set()
    for item in evidence_files:
        exact(item, {"path", "type", "bytes", "sha256"})
        name = item["path"]
        if (item["type"] != "file" or not isinstance(name, str)
                or Path(name).name != name or name in seen
                or file_digest(directory / name) != {key: item[key] for key in ("bytes", "sha256")}):
            raise ValueError("首次初始化库存文件描述无效")
        seen.add(name)
        paths.add("inventory-initial/" + name)
    if "inventory.json" not in seen:
        raise ValueError("首次初始化库存闭合集缺少成功收据")
    return paths


def validate_complete(backend: Path, output: Path, baseline: list[dict], request: dict,
                      original: dict, initial: dict, inventory_files: tuple[dict, ...],
                      result: dict) -> list[dict]:
    """只接受固定创建、reset、迁移、库存和最终发布组成的完整树。"""
    from devex_clone_target import target_evidence_hooks
    from devex_clone_target_resume_evidence import (
        _creation_evidence, _initialization_paths, _reset_evidence,
        migration_operations, migration_prefix)

    creation = _creation_evidence(output, request)
    reset = _reset_evidence(target_evidence_hooks(), output, request, original)
    operations = migration_operations(original["maintenance"])
    completed = migration_prefix(output, operations, complete=True)["completed"]
    required = _initialization_paths(output, creation, reset, completed)
    required.discard("failure.json")
    required.update(_inventory_paths(output, initial, inventory_files))
    required.update({"initialized-candidate.json", "initialized.json"})
    if result != __import__("devex_clone").read_json(output / "initialized.json"):
        raise ValueError("首次初始化最终收据内容变化")
    current = target_files(output, ignored={"initialize.lock"}, locked_guard=True)
    validate_initialization_delta(output, current, baseline, request, required,
                                  observation_rounds=12 + len(completed),
                                  initialized_objects=result["objects"])
    return current
