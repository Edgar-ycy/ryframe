"""fresh 目标 CLI 的阶段文件树快照与完整绑定。"""
from pathlib import Path

from devex_clone_capture import read_bound_json, write_json
from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_target_binding import target_files
from devex_clone_target_prepared_files import validate_prepared_tree


def digest_binding(path: Path) -> dict:
    return {key: value for key, value in binding(path).items() if key != "path"}


def snapshot(workspace: Path, value: dict, stage: str, *, locked_guard: bool = False,
             initialize_lock_released: bool = False) -> dict:
    path = workspace / f"{stage}-files.json"
    before = binding(path)
    document = read_bound_json(path, before)
    exact(document, {"format_version", "kind", "registration", "predecessor", "files"})
    predecessor = binding(workspace / "prepared-files.json") if stage == "initialized" else None
    ignored = {"initialize.lock"} if locked_guard else None
    if (document["format_version"] != 1
            or document["kind"] != f"devex-clone-fresh-target-{stage}-files"
            or document["registration"] != binding(workspace / "registration.json")
            or document["predecessor"] != predecessor
            or document["files"] != target_files(
                Path(value["target_directory"]), ignored=ignored, locked_guard=locked_guard,
                initialize_lock_released=initialize_lock_released)
            or binding(path) != before):
        raise ValueError("fresh 目标阶段文件集合、内容或登记发生变化")
    return before


def write_snapshot(backend: Path, workspace: Path, value: dict, stage: str,
                   expected_files: list[dict] | None = None, *, expected_document: dict | None = None,
                   locked_guard: bool = False) -> dict:
    target = Path(value["target_directory"])
    first = target_files(target, locked_guard=locked_guard,
                         initialize_lock_released=locked_guard)
    if expected_files is not None and first != expected_files:
        raise ValueError("fresh 目标初始化锁释放后的最终文件树不同于执行结果")
    if target_files(target, locked_guard=locked_guard,
                    initialize_lock_released=locked_guard) != first:
        raise ValueError("fresh 目标目录在阶段快照期间变化")
    if stage == "prepared":
        registration = read_bound_json(
            workspace / "registration.json", binding(workspace / "registration.json")
        )
        request_descriptor = registration["request"]
        request = read_bound_json(Path(request_descriptor["path"]), request_descriptor)
        validate_prepared_tree(backend, target, first, request_descriptor, request)
        if expected_document is not None and read_bound_json(
                target / "prepare.json", binding(target / "prepare.json")) != expected_document:
            raise ValueError("fresh 目标 prepare 文件不同于叶子执行结果")
    predecessor = binding(workspace / "prepared-files.json") if stage == "initialized" else None
    result = {
        "format_version": 1,
        "kind": f"devex-clone-fresh-target-{stage}-files",
        "registration": binding(workspace / "registration.json"),
        "predecessor": predecessor,
        "files": first,
    }
    path = workspace / f"{stage}-files.json"
    write_json(path, result)
    descriptor = binding(path)
    if snapshot(workspace, value, stage, locked_guard=locked_guard,
                initialize_lock_released=locked_guard) != descriptor:
        raise ValueError("fresh 目标目录快照发布期间变化")
    return descriptor


def resume_prepared_files(workspace: Path, value: dict) -> dict:
    """返回完整 prepare 快照绑定；叶子执行器会再次核对登记和前像。"""
    path = workspace / "prepared-files.json"
    before = binding(path)
    document = read_bound_json(path, before)
    exact(document, {"format_version", "kind", "registration", "predecessor", "files"})
    if (document["format_version"] != 1
            or document["kind"] != "devex-clone-fresh-target-prepared-files"
            or document["registration"] != binding(workspace / "registration.json")
            or document["predecessor"] is not None
            or not isinstance(document["files"], list)
            or binding(path) != before):
        raise ValueError("fresh 目标 prepare 文件快照绑定无效")
    return {
        "descriptor": before,
        "registration": document["registration"],
        "predecessor": document["predecessor"],
    }
