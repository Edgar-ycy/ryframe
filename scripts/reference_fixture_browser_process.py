"""运行 Device 前端命令并严格绑定完整进程树证据。"""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import threading

from full_stack_process_monitor import completion_binding
from full_stack_process_tree import (
    launch_supervised_process,
    read_process_tree,
    terminate_owned_process_tree,
)
from restore_runtime_evidence import artifact_snapshot, process_document, read_json_document


def _bound(path: Path) -> dict:
    return artifact_snapshot(path).descriptor()


class OutputCapture:
    """先在内存中收集受控子进程输出，脱敏后才写入证据文件。"""

    def __init__(self, stream, maximum: int = 64 * 1024 * 1024):
        self.stream = stream
        self.maximum = maximum
        self.chunks: list[bytes] = []
        self.size = 0
        self.overflow = False
        self.error: BaseException | None = None
        self.thread = threading.Thread(target=self._read, name="device-browser-log", daemon=False)

    def _read(self) -> None:
        try:
            while block := self.stream.read(64 * 1024):
                if not self.overflow and self.size + len(block) <= self.maximum:
                    self.chunks.append(block)
                    self.size += len(block)
                else:
                    self.overflow = True
        except BaseException as error:
            self.error = error

    def start(self) -> None:
        self.thread.start()

    def finish(self, path: Path, secrets: tuple[str, ...]) -> None:
        self.thread.join(timeout=15)
        if self.thread.is_alive():
            raise TimeoutError("Device 前端日志管道未在进程树停止后关闭")
        if self.error is not None:
            raise RuntimeError("Device 前端日志读取失败") from self.error
        raw = b"".join(self.chunks)
        for secret in secrets:
            raw = raw.replace(secret.encode("utf-8"), b"[REDACTED]")
        with path.open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        if self.overflow:
            raise ValueError("Device 前端日志超过 64 MiB，完整输出未写入证据")


def process_evidence(directory: Path, scope_id: str, expected_tree: dict | None = None) -> dict:
    tree = read_process_tree(directory, "frontend", scope_id)
    if expected_tree is not None and tree != expected_tree:
        raise ValueError("Device 前端进程树与本次受控启动不一致")
    process, identity = process_document(directory / "frontend.json", "frontend", scope_id)
    if identity != tree["process"]:
        raise ValueError("Device 前端产品进程身份与进程树不一致")
    completion = completion_binding(tree)
    tree_path = directory / "frontend-tree.json"
    result = {"directory": str(directory), "process": _bound(process.path),
              "tree": _bound(tree_path), "completion": completion}
    process.assert_unchanged()
    if read_process_tree(directory, "frontend", scope_id) != tree:
        raise ValueError("Device 前端进程树在证据绑定期间发生变化")
    return result


def command(binding: dict, arguments: list[str]) -> list[str]:
    corepack = binding["tools"]["corepack"]["path"]
    if os.name != "nt":
        return [corepack, "pnpm", *arguments]
    raw = subprocess.list2cmdline([corepack, "pnpm", *arguments])
    return [binding["tools"]["launcher"]["path"], "/d", "/s", "/c", raw]


def run_frontend_command(binding: dict, frontend: Path, arguments: list[str], environment: dict,
                         log: Path, process_dir: Path, timeout: float,
                         secrets: tuple[str, ...]) -> dict:
    process_dir.mkdir()
    process = None
    capture = None
    error = None
    try:
        process = launch_supervised_process(
            process_dir, "frontend", binding["scope_id"], command(binding, arguments),
            frontend, environment, subprocess.PIPE,
        )
        if process.supervisor.stdout is None:
            raise ValueError("Device 前端监督进程没有提供受控日志管道")
        capture = OutputCapture(process.supervisor.stdout)
        capture.start()
        exit_code = process.wait(timeout=timeout)
        evidence = process_evidence(process_dir, binding["scope_id"], process.tree)
        if exit_code != 0:
            raise subprocess.CalledProcessError(exit_code, ["corepack", "pnpm", *arguments])
        return evidence
    except BaseException as caught:
        error = caught
        if process is not None:
            try:
                terminate_owned_process_tree(process.tree, crash=True)
                process.wait(timeout=10)
                completion_binding(process.tree)
            except BaseException as cleanup:
                caught.add_note("Device 前端进程树回收失败：" + type(cleanup).__name__)
        raise
    finally:
        if capture is not None:
            try:
                capture.finish(log, secrets)
            except BaseException as cleanup:
                if error is None:
                    raise
                error.add_note("Device 前端日志脱敏失败：" + type(cleanup).__name__)


def failure_process(directory: Path, scope_id: str) -> dict:
    """绑定失败阶段已有的全部进程证据；完整关闭时返回严格成功证据。"""
    try:
        return process_evidence(directory, scope_id)
    except (FileNotFoundError, ValueError):
        result = {"directory": str(directory)}
        for name, label in (("frontend.json", "process"), ("frontend-tree.json", "tree")):
            path = directory / name
            if path.is_file():
                result[label] = _bound(path)
        tree_path = directory / "frontend-tree.json"
        if tree_path.is_file():
            value = read_json_document(tree_path).value
            operation = value.get("operation_id") if isinstance(value, dict) else None
            if isinstance(operation, str):
                for suffix, label in (("result", "tree_result"), ("stopped", "completion")):
                    prefix = "frontend-tree" if suffix == "result" else "frontend-members"
                    path = directory / f"{prefix}-{operation}-{suffix}.json"
                    if path.is_file():
                        result[label] = _bound(path)
        return result
