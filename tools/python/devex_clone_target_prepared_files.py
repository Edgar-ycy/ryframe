"""仍在迁移的初始化消费者复用唯一原生 prepare 历史校验。"""
from contextlib import contextmanager
import hashlib
from pathlib import Path

from devex_clone_target_evidence import evidence


@contextmanager
def _prepare_files(target: Path, files: list[dict], *, locked_guard: bool):
    if not locked_guard:
        yield files
        return
    from devex_clone_target_state import generation_checkpoint

    generation_checkpoint(target)
    guard = {"path": "copy-target.guard", "type": "file", "bytes": 1,
             "sha256": hashlib.sha256(b"\0").hexdigest()}
    guards = [item for item in files if isinstance(item, dict)
              and item.get("path") == "copy-target.guard"]
    if guards != [guard] or type(guards[0]["bytes"]) is not int:
        raise ValueError("prepare 持锁快照缺少唯一精确的控制互斥条目；核对当前租约与原始文件，禁止续作")
    # 租约通过已锁句柄核验实际文件；Windows 不能由子进程重读该锁定字节。
    observed = [item for item in files if item is not guards[0]]
    try:
        yield observed
    except BaseException as error:
        try:
            generation_checkpoint(target)
        except Exception as changed:
            error.add_note("prepare 证据失败且控制租约复核失败：" + type(changed).__name__)
        raise
    else:
        generation_checkpoint(target)


def validate_prepared_tree(backend: Path, target: Path, files: list[dict],
                           request_descriptor: dict, request: dict, *, locked_guard: bool = False) -> None:
    with _prepare_files(target, files, locked_guard=locked_guard) as observed:
        evidence("validate-prepared-tree", backend, target, arguments={
            "files": observed, "request_descriptor": request_descriptor, "request": request})


def active_observations(backend: Path, target: Path, files: list[dict]) -> list[dict]:
    locked_guard = any(isinstance(item, dict) and item.get("path") == "copy-target.guard"
                       for item in files)
    with _prepare_files(target, files, locked_guard=locked_guard) as observed:
        return evidence("prepared-observations", backend, target, arguments={"files": observed})["files"]
