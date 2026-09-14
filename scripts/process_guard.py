"""本机工具控制器共享的内核互斥；进程退出即释放，固定文件不作为存活证据。"""
from contextlib import contextmanager
import os
from pathlib import Path
import stat


_CHANGED = "控制互斥文件已被替换或改变，拒绝继续"


def _identity(metadata):
    return metadata.st_dev, metadata.st_ino


def _link_like(path: Path, metadata) -> bool:
    attributes = getattr(metadata, "st_file_attributes", 0)
    reparse = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    is_junction = getattr(os.path, "isjunction", lambda _path: False)
    return (stat.S_ISLNK(metadata.st_mode) or bool(attributes & reparse)
            or bool(is_junction(path)))


def _metadata(path: Path, message: str):
    try:
        return path.lstat()
    except OSError as error:
        raise ValueError(message) from error


def _parent_identity(directory: Path):
    result = None
    for index, part in enumerate((directory, *directory.parents)):
        metadata = _metadata(part, "控制互斥父目录无法核验，拒绝继续")
        if _link_like(part, metadata) or not stat.S_ISDIR(metadata.st_mode):
            raise ValueError("控制互斥路径不能经过链接或非目录")
        if index == 0:
            result = _identity(metadata)
    return result


def _guard_metadata(path: Path):
    metadata = _metadata(path, _CHANGED)
    if (_link_like(path, metadata) or not stat.S_ISREG(metadata.st_mode)
            or metadata.st_nlink != 1):
        raise ValueError(_CHANGED)
    return metadata


def _opened_state(path: Path, descriptor: int, expected=None, size=None):
    actual = _guard_metadata(path)
    try:
        opened = os.fstat(descriptor)
    except OSError as error:
        raise ValueError(_CHANGED) from error
    if (not stat.S_ISREG(opened.st_mode) or opened.st_nlink != 1
            or _identity(actual) != _identity(opened)
            or expected is not None and _identity(opened) != expected
            or size is not None and (actual.st_size != size or opened.st_size != size)):
        raise ValueError(_CHANGED)
    return _identity(opened)


def _open_guard(path: Path, directory: Path, parent_identity):
    common = (os.O_RDWR | getattr(os, "O_BINARY", 0) | getattr(os, "O_CLOEXEC", 0)
              | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    descriptor = None
    created = False
    before = None
    try:
        try:
            descriptor = os.open(path, common | os.O_CREAT | os.O_EXCL, 0o600)
            created = True
        except FileExistsError:
            before = _guard_metadata(path)
            descriptor = os.open(path, common)
        if _parent_identity(directory) != parent_identity:
            raise ValueError("控制互斥父目录已被替换，拒绝继续")
        expected = None if before is None else _identity(before)
        identity = _opened_state(path, descriptor, expected, 0 if created else 1)
        if created:
            if os.write(descriptor, b"\0") != 1:
                raise ValueError("无法初始化控制互斥文件")
        os.lseek(descriptor, 0, os.SEEK_SET)
        if (_parent_identity(directory) != parent_identity
                or _opened_state(path, descriptor, identity, 1) != identity):
            raise ValueError(_CHANGED)
        stream = os.fdopen(descriptor, "r+b", buffering=0)
        descriptor = None
        return stream, identity
    except OSError as error:
        raise ValueError("控制互斥文件无法安全打开，拒绝继续") from error
    finally:
        if descriptor is not None:
            os.close(descriptor)


class GuardLease:
    """持有已锁句柄，并在关键边界复核文件与父目录身份。"""

    def __init__(self, path: Path, directory: Path, stream, identity, parent_identity):
        self.path = path
        self.directory = directory
        self.stream = stream
        self.identity = identity
        self.parent_identity = parent_identity

    def check(self) -> None:
        if _parent_identity(self.directory) != self.parent_identity:
            raise ValueError("控制互斥父目录已被替换，拒绝继续")
        _opened_state(self.path, self.stream.fileno(), self.identity, 1)
        try:
            self.stream.seek(0)
            marker = self.stream.read(2)
            self.stream.seek(0)
        except OSError as error:
            raise ValueError(_CHANGED) from error
        if marker != b"\0":
            raise ValueError("控制互斥文件内容无效，拒绝继续")
        if (_parent_identity(self.directory) != self.parent_identity
                or _opened_state(self.path, self.stream.fileno(), self.identity, 1)
                != self.identity):
            raise ValueError(_CHANGED)


@contextmanager
def process_guard(directory: Path, filename: str):
    if (not directory.is_absolute() or not directory.is_dir() or Path(filename).name != filename
            or filename in {"", ".", ".."}):
        raise ValueError("控制互斥须使用明确目录与单一文件名")
    parent_identity = _parent_identity(directory)
    path = directory / filename
    guard, identity = _open_guard(path, directory, parent_identity)
    # Windows msvcrt.locking 使用底层文件位置；无缓冲可保证 seek 与锁区间一致。
    with guard:
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
        lease = GuardLease(path, directory, guard, identity, parent_identity)
        try:
            lease.check()
            yield lease
        finally:
            try:
                lease.check()
            finally:
                guard.seek(0)
                if os.name == "nt":
                    msvcrt.locking(guard.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    fcntl.flock(guard.fileno(), fcntl.LOCK_UN)
