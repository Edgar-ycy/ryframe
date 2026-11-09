"""剩余Python消费端只读核对正式阶段文件快照；不执行初始化或续作。"""
from pathlib import Path

from devex_clone_capture import read_bound_json
from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_target_binding import target_files


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
