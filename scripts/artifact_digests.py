"""仅在明确复制阶段保护已登记的 Windows 二进制，复用不可写文件的摘要。"""
from __future__ import annotations

from contextlib import contextmanager
import ctypes
from ctypes import wintypes
import hashlib
import os
from pathlib import Path
import re
import struct
import threading

_LOCK = threading.RLock()
_ACTIVE = None
_WINDOWS = os.name == "nt"


def filesystem_path(path: Path | str) -> str:
    """返回仅供文件系统调用使用的路径，保留调用方原始 Path 的比较语义。"""
    value = os.fspath(path)
    if not _WINDOWS or value.startswith("\\\\?\\") or not os.path.isabs(value):
        return value
    if value.startswith("\\\\"):
        return "\\\\?\\UNC\\" + value[2:]
    return "\\\\?\\" + value


def _plain_digest(path: Path) -> dict:
    try:
        stream = path.open("rb")
    except OSError:
        if not _WINDOWS:
            raise
        stream = open(filesystem_path(path), "rb")
    with stream:
        sha = hashlib.file_digest(stream, "sha256").hexdigest()
        size = os.fstat(stream.fileno()).st_size
    return {"bytes": size, "sha256": sha}


def _binary_path(path: Path) -> Path:
    if (not path.is_absolute() or not re.fullmatch(r"[a-zA-Z]:", path.drive)
            or path.suffix.lower() != ".exe" or ":" in str(path)[2:]
            or any(item.is_symlink() or getattr(item.lstat(), "st_file_attributes", 0) & 0x400
                   for item in (path, *path.parents))):
        raise ValueError("阶段摘要只接受无链接的本机绝对二进制路径")
    resolved = path.resolve(strict=True)
    if not resolved.is_file():
        raise ValueError("登记的二进制必须是普通文件")
    return resolved


def _bindings(values: list[dict]) -> dict:
    if not isinstance(values, list) or not values:
        raise ValueError("阶段二进制必须是非空的明确登记集合")
    result = {}
    for value in values:
        if (not isinstance(value, dict) or set(value) != {"path", "sha256"}
                or not isinstance(value["path"], str) or not isinstance(value["sha256"], str)
                or not re.fullmatch(r"[a-f0-9]{64}", value["sha256"])):
            raise ValueError("阶段二进制须绑定明确路径和 SHA-256")
        path = _binary_path(Path(value["path"]))
        key = os.path.normcase(str(path))
        if key in result and result[key][1] != value["sha256"]:
            raise ValueError("同一二进制路径登记了不同摘要")
        result[key] = (path, value["sha256"])
    return result


class _FileInfo(ctypes.Structure):
    _fields_ = [("attributes", wintypes.DWORD), ("created", wintypes.FILETIME),
                ("accessed", wintypes.FILETIME), ("written", wintypes.FILETIME),
                ("volume", wintypes.DWORD), ("size_high", wintypes.DWORD), ("size_low", wintypes.DWORD),
                ("links", wintypes.DWORD), ("index_high", wintypes.DWORD), ("index_low", wintypes.DWORD)]


def _kernel():
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                  ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    kernel.CreateFileW.restype = wintypes.HANDLE
    kernel.GetFileInformationByHandle.argtypes = [wintypes.HANDLE, ctypes.POINTER(_FileInfo)]
    kernel.GetFileInformationByHandle.restype = wintypes.BOOL
    kernel.GetFinalPathNameByHandleW.argtypes = [wintypes.HANDLE, wintypes.LPWSTR, wintypes.DWORD, wintypes.DWORD]
    kernel.GetFinalPathNameByHandleW.restype = wintypes.DWORD
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    return kernel


def _open(kernel, path: Path):
    # 只共享读取；已有写句柄或可写映射也必须导致获取失败。句柄不继承到子进程。
    handle = kernel.CreateFileW(str(path), 0x80000000, 0x00000001, None, 3, 0x00200000, None)
    if handle == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    return handle


def _identity(kernel, handle) -> tuple:
    info = _FileInfo()
    if not kernel.GetFileInformationByHandle(handle, ctypes.byref(info)):
        raise ctypes.WinError(ctypes.get_last_error())
    if info.attributes & (0x10 | 0x400):
        raise ValueError("二进制句柄不能指向目录或重解析点")
    length = kernel.GetFinalPathNameByHandleW(handle, None, 0, 0)
    if not length:
        raise ctypes.WinError(ctypes.get_last_error())
    buffer = ctypes.create_unicode_buffer(length + 1)
    used = kernel.GetFinalPathNameByHandleW(handle, buffer, len(buffer), 0)
    if not used or used >= len(buffer):
        raise ValueError("无法稳定核对二进制句柄的真实路径")
    return (info.volume, info.index_high, info.index_low, info.size_high, info.size_low,
            os.path.normcase(buffer.value.removeprefix("\\\\?\\")))


def _close(kernel, handle) -> None:
    if not kernel.CloseHandle(handle):
        raise ctypes.WinError(ctypes.get_last_error())


def _release(files) -> BaseException | None:
    failure = None
    for item in reversed(list(files)):
        try:
            item.close()
        except BaseException as error:
            failure = failure or error
    return failure


class _Protected:
    def __init__(self, path: Path, expected: str):
        self.path, self.kernel, self.handle = path, _kernel(), None
        before = _plain_digest(path)
        if before["sha256"] != expected:
            raise ValueError("持有保护句柄前二进制摘要与登记不符")
        self.handle = _open(self.kernel, path)
        try:
            self.identity = _identity(self.kernel, self.handle)
            with path.open("rb") as stream:
                # 当前入口仅适用于本机 PE 产物，不能用于 SQL、凭据或业务文件。
                if stream.read(2) != b"MZ":
                    raise ValueError("登记文件不是 Windows 二进制")
                stream.seek(0x3C)
                header = stream.read(4)
                if len(header) != 4:
                    raise ValueError("Windows 二进制头不完整")
                offset = struct.unpack("<I", header)[0]
                if not 64 <= offset <= before["bytes"] - 4:
                    raise ValueError("Windows 二进制头位置无效")
                stream.seek(offset)
                if stream.read(4) != b"PE\0\0":
                    raise ValueError("登记文件缺少真实 PE 标识")
            self.value = _plain_digest(path)
            if self.value != before:
                raise ValueError("获取保护句柄期间二进制内容变化")
            self.check()
        except BaseException as error:
            cleanup = _release([self])
            if cleanup is not None:
                error.add_note(f"二进制保护初始化收尾同时失败：{type(cleanup).__name__}")
            raise

    def check(self) -> None:
        if self.handle is None or _binary_path(self.path) != self.path:
            raise ValueError("二进制保护句柄已释放或路径变化")
        if _identity(self.kernel, self.handle) != self.identity:
            raise ValueError("持有二进制句柄的身份变化")
        current = _open(self.kernel, self.path)
        try:
            if _identity(self.kernel, current) != self.identity:
                raise ValueError("当前路径不再指向受保护的实际二进制")
        finally:
            _close(self.kernel, current)

    def close(self) -> None:
        handle, self.handle = self.handle, None
        if handle is not None:
            _close(self.kernel, handle)


def protected_digest(path: Path) -> dict | None:
    """非登记文件继续原完整读取；命中时每次核对路径、卷和实际文件身份。"""
    with _LOCK:
        if not _WINDOWS or _ACTIVE is None:
            return None
        key = os.path.normcase(os.path.abspath(path))
        protected = _ACTIVE["files"].get(key)
        if protected is None:
            return None
        protected.check()
        return dict(protected.value)


def file_digest(path: Path) -> dict:
    """摘要只接受普通文件；未登记的文件每次完整读取，不复用路径或时间缓存。"""
    path = Path(path)
    native = filesystem_path(path)
    if path.is_symlink() or os.path.islink(native) or not os.path.isfile(native):
        raise ValueError("产物必须是普通文件")
    protected = protected_digest(path)
    return protected if protected is not None else _plain_digest(path)


@contextmanager
def protect_binaries(bindings: list[dict]):
    """保护只活在本进程的明确阶段内；嵌套不得扩大集合，Linux 不复用摘要。"""
    global _ACTIVE
    if not _WINDOWS:
        yield
        return
    values = _bindings(bindings)
    with _LOCK:
        if _ACTIVE is not None:
            if _ACTIVE["thread"] != threading.get_ident() or _ACTIVE["bindings"] != values:
                raise ValueError("嵌套二进制保护必须属于同一控制器和相同登记集合")
            for item in _ACTIVE["files"].values():
                item.check()
            nested = True
        else:
            nested = False
            protected = {}
            try:
                for key, (path, expected) in values.items():
                    protected[key] = _Protected(path, expected)
            except BaseException as error:
                cleanup = _release(protected.values())
                if cleanup is not None:
                    error.add_note(f"二进制保护集合收尾同时失败：{type(cleanup).__name__}")
                raise
            _ACTIVE = {"thread": threading.get_ident(), "bindings": values, "files": protected}
    primary = None
    try:
        yield
    except BaseException as error:
        primary = error
        raise
    finally:
        cleanup = None
        with _LOCK:
            try:
                for item in _ACTIVE["files"].values():
                    item.check()
            except BaseException as error:
                cleanup = error
            if not nested:
                try:
                    release_error = _release(_ACTIVE["files"].values())
                    cleanup = cleanup or release_error
                finally:
                    _ACTIVE = None
        if cleanup is not None:
            if primary is None:
                raise cleanup
            primary.add_note(f"阶段二进制保护收尾同时失败：{type(cleanup).__name__}")
