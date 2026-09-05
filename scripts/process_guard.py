"""本机工具控制器共享的内核互斥；进程退出即释放，固定文件不作为存活证据。"""
from contextlib import contextmanager
import os
from pathlib import Path


@contextmanager
def process_guard(directory: Path, filename: str):
    if (not directory.is_absolute() or not directory.is_dir() or Path(filename).name != filename
            or filename in {"", ".", ".."}):
        raise ValueError("控制互斥须使用明确目录与单一文件名")
    path = directory / filename
    if any(part.is_symlink() or (hasattr(part, "is_junction") and part.is_junction())
           for part in (path, *path.parents)):
        raise ValueError("控制互斥路径不能经过链接")
    with path.open("a+b") as guard:
        if guard.tell() == 0:
            guard.write(b"\0")
            guard.flush()
        guard.seek(0)
        try:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(guard.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(guard.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise ValueError("控制或锁恢复正在执行，拒绝并发操作") from error
        try:
            actual, opened = path.stat(), os.fstat(guard.fileno())
            if (actual.st_dev, actual.st_ino) != (opened.st_dev, opened.st_ino):
                raise ValueError("控制互斥文件已被替换，拒绝继续")
            yield
        finally:
            guard.seek(0)
            if os.name == "nt":
                msvcrt.locking(guard.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(guard.fileno(), fcntl.LOCK_UN)
