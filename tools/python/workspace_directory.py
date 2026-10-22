"""为 Windows 测试创建继承工作区权限的临时目录。"""

import os
import shutil
import stat
import uuid
from pathlib import Path


class WorkspaceDirectory:
    """避免 Python 3.13+ 在 Windows 上为 0700 临时目录创建隔离 ACL。"""

    def __init__(
        self,
        parent: Path | None = None,
        prefix: str = "tmp-",
        *,
        dir: Path | None = None,
    ) -> None:
        if parent is not None and dir is not None:
            raise TypeError("只能指定 parent 或 dir")
        location = parent if parent is not None else dir
        if location is None:
            raise TypeError("必须指定临时目录父路径")
        parent = Path(location)
        parent.mkdir(parents=True, exist_ok=True)
        self.path = (parent / f"{prefix}{uuid.uuid4().hex}").resolve()
        self.path.mkdir()
        self.name = str(self.path)
        self._root_path = self.path
        self._root_stat = self.path.lstat()

    def cleanup(self) -> None:
        if os.name != "nt":
            if self.path.exists():
                shutil.rmtree(self.path)
            return
        if self.path != self._root_path:
            raise PermissionError("临时目录根路径已改变", str(self.path))
        try:
            self.path.lstat()
        except FileNotFoundError:
            return
        self._checked_cleanup_path(self.path)
        for directory, directories, files in os.walk(self.path, onerror=self._walk_error):
            for name in directories + files:
                self._checked_cleanup_path(Path(directory) / name)
        shutil.rmtree(self.path, onerror=self._retry_readonly_file)

    @staticmethod
    def _walk_error(error: OSError) -> None:
        raise error

    def _checked_cleanup_path(self, path: Path) -> os.stat_result:
        root = self._root_path
        candidate = Path(os.path.abspath(path))
        if self.path != root or not path.is_absolute() or not candidate.is_relative_to(root):
            raise PermissionError("拒绝清理临时目录根以外的路径", str(path))
        for current in reversed((candidate, *candidate.parents)):
            information = current.lstat()
            attributes = getattr(information, "st_file_attributes", 0)
            if stat.S_ISLNK(information.st_mode) or attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT:
                raise PermissionError("拒绝清理包含链接或重解析点的路径", str(current))
            if current != candidate and not stat.S_ISDIR(information.st_mode):
                raise PermissionError("临时目录父路径已被替换", str(current))
            if current == root and (
                not stat.S_ISDIR(information.st_mode)
                or not os.path.samestat(information, self._root_stat)
            ):
                raise PermissionError("临时目录根身份已改变", str(root))
        if candidate.resolve(strict=True) != candidate:
            raise PermissionError("临时目录规范路径已改变", str(candidate))
        return information

    def _retry_readonly_file(self, operation, path, exc_info) -> None:
        error = exc_info[1]
        if (
            os.name != "nt"
            or not isinstance(error, PermissionError)
            or operation not in (os.unlink, os.remove)
        ):
            raise error
        candidate = Path(path)
        information = self._checked_cleanup_path(candidate)
        if (
            not stat.S_ISREG(information.st_mode)
            or not information.st_file_attributes & stat.FILE_ATTRIBUTE_READONLY
            or information.st_nlink != 1
        ):
            raise error
        # Windows 的 chmod 仅调整只读位；共享硬链接文件不属于本目录独占。
        os.chmod(candidate, information.st_mode | stat.S_IWRITE)
        current = self._checked_cleanup_path(candidate)
        if not os.path.samestat(information, current):
            raise PermissionError("临时文件身份已改变", str(candidate))
        operation(path)

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, _exc_type, _exc_value, _traceback) -> None:
        self.cleanup()
