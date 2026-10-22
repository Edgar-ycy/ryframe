"""用独占受管文件把正式 Cargo 产物绑定到精确后端提交。"""

from __future__ import annotations

from contextlib import contextmanager
import os
from pathlib import Path
import re
import uuid


def build_process_environment(environment: dict[str, str] | None = None) -> dict[str, str]:
    """保留操作系统/第三方构建环境，移除项目配置和旧构建提交覆盖。"""
    source = os.environ if environment is None else environment
    result = {}
    for name, value in source.items():
        upper = name.upper()
        project = (
            upper.startswith("RYFRAME_")
            or upper.startswith("VITE_")
            or upper.startswith("SNOWFLAKE_")
            or (upper.startswith("APP_") and upper != "APP_ENV")
        )
        if not project:
            result[name] = value
    return result


@contextmanager
def bound_source(backend: Path, commit: str):
    """在 Cargo 运行期间发布固定来源；漂移时保留现场且不自动重放。"""
    if re.fullmatch(r"[a-f0-9]{40}", commit) is None:
        raise ValueError("构建来源必须是 40 位小写提交 SHA")
    backend = backend.resolve(strict=True)
    local = backend / ".local-tests"
    local.mkdir(exist_ok=True)
    directory = local / "build"
    directory.mkdir(exist_ok=True)
    for path in (backend, local, directory):
        if _linked(path) or not path.is_dir():
            raise ValueError("构建来源目录不能经过链接或重解析点")
    source = directory / "source-commit"
    lock = directory / "source-commit.lock"
    if source.exists() or lock.exists():
        raise ValueError("构建来源已有控制器或未确认现场；核对后再执行")
    lock_bytes = f"{os.getpid()}:{uuid.uuid4()}\n".encode()
    source_bytes = f"{commit}\n".encode()
    _write_new(lock, lock_bytes)
    try:
        _write_new(source, source_bytes)
    except BaseException:
        _remove_exact(lock, lock_bytes, "构建来源锁")
        raise
    try:
        yield source
    except BaseException as operation:
        try:
            _cleanup(source, source_bytes, lock, lock_bytes)
        except (OSError, ValueError) as cleanup:
            operation.add_note(f"构建来源清理失败：{cleanup}；保留现场，不自动重放")
        raise
    else:
        _cleanup(source, source_bytes, lock, lock_bytes)


def _write_new(path: Path, content: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def _cleanup(source: Path, source_bytes: bytes, lock: Path, lock_bytes: bytes) -> None:
    _remove_exact(source, source_bytes, "构建来源")
    _remove_exact(lock, lock_bytes, "构建来源锁")


def _remove_exact(path: Path, expected: bytes, label: str) -> None:
    if path.read_bytes() != expected:
        raise ValueError(f"{label}发生漂移；保留现场，不清理")
    path.unlink()


def _linked(path: Path) -> bool:
    metadata = path.lstat()
    return path.is_symlink() or bool((getattr(metadata, "st_file_attributes", 0) or 0) & 0x400)
