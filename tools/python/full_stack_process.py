"""用进程创建身份和操作系统句柄管理全栈测试进程，避免 PID 重用误清理。"""

from __future__ import annotations

import ctypes
import json
import os
import select
import signal
import uuid
from contextlib import contextmanager
from pathlib import Path

PROCESS_ROLES = frozenset({
    "alertmanager",
    "api",
    "frontend",
    "prometheus",
    "rustfs",
    "webhook",
    "worker",
})


def _linux_identity(pid: int) -> dict | None:
    try:
        process = Path(f"/proc/{pid}")
        fields = (process / "stat").read_text().rpartition(")")[2].split()
        return {"pid": pid, "started": fields[19], "executable": str((process / "exe").resolve(strict=True))}
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        return None


@contextmanager
def _windows_handle(pid: int, terminate: bool = False):
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    access = 0x1000 | 0x100000 | (0x0001 if terminate else 0)
    handle = kernel.OpenProcess(access, False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == 87:
            yield kernel, None
            return
        raise ctypes.WinError(error)
    try:
        yield kernel, handle
    finally:
        kernel.CloseHandle(handle)


def _windows_identity(kernel, handle, pid: int) -> dict | None:
    from ctypes import wintypes

    if handle is None:
        return None
    kernel.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    if kernel.WaitForSingleObject(handle, 0) == 0:
        return None
    kernel.GetProcessTimes.argtypes = (wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4))
    times = [wintypes.FILETIME() for _ in range(4)]
    if not kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
        raise ctypes.WinError(ctypes.get_last_error())
    kernel.QueryFullProcessImageNameW.argtypes = (wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD))
    size = wintypes.DWORD(32768)
    executable = ctypes.create_unicode_buffer(size.value)
    if not kernel.QueryFullProcessImageNameW(handle, 0, executable, ctypes.byref(size)):
        raise ctypes.WinError(ctypes.get_last_error())
    started = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
    return {"pid": pid, "started": str(started), "executable": str(Path(executable.value).resolve())}


def process_identity(pid: int) -> dict | None:
    if type(pid) is not int or pid <= 1:
        raise ValueError("测试进程 PID 必须大于 1")
    if os.name == "nt":
        with _windows_handle(pid) as (kernel, handle):
            return _windows_identity(kernel, handle, pid)
    return _linux_identity(pid)


def assert_identity(actual: dict | None, expected: dict) -> bool:
    if actual is None:
        return False
    if actual != expected:
        raise ValueError("测试进程身份已变化，拒绝按复用的 PID 发送信号")
    return True


def terminate_owned_process(expected: dict, *, crash: bool = False) -> bool:
    pid = expected.get("pid")
    if type(pid) is not int or pid <= 1:
        raise ValueError("测试进程 PID 必须大于 1")
    if os.name == "nt":
        with _windows_handle(pid, terminate=True) as (kernel, handle):
            if not assert_identity(_windows_identity(kernel, handle, pid), expected):
                return False
            from ctypes import wintypes

            kernel.TerminateProcess.argtypes = (wintypes.HANDLE, wintypes.UINT)
            if not kernel.TerminateProcess(handle, 137 if crash else 0):
                raise ctypes.WinError(ctypes.get_last_error())
            if kernel.WaitForSingleObject(handle, 5000) != 0:
                raise TimeoutError("测试进程未在期限内退出")
            return True
    # pidfd 在核验前打开；之后的发送和等待都绑定同一内核对象。
    try:
        descriptor = os.pidfd_open(pid)
    except ProcessLookupError:
        return False
    try:
        if not assert_identity(_linux_identity(pid), expected):
            return False
        signal.pidfd_send_signal(descriptor, signal.SIGKILL if crash else signal.SIGTERM)
        if not select.select([descriptor], [], [], 5)[0]:
            signal.pidfd_send_signal(descriptor, signal.SIGKILL)
            if not select.select([descriptor], [], [], 5)[0]:
                raise TimeoutError("测试进程未在期限内退出")
        return True
    except ProcessLookupError:
        return False
    finally:
        os.close(descriptor)


def write_receipt(path: Path, receipt: dict) -> None:
    # 临时文件只需要在同一目录内保持唯一。不要重复目标文件名，否则深层的
    # Windows 验收目录会让原子发布的临时路径超过 MAX_PATH。
    temporary = path.with_name(f".{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("x", encoding="utf-8", newline="\n") as output:
            output.write(json.dumps(receipt, indent=2) + "\n")
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def record_process(directory: Path, role: str, pid: int, executable: str, scope: str) -> dict:
    if role not in PROCESS_ROLES:
        raise ValueError("未知全栈进程角色")
    identity = process_identity(pid)
    if identity is None or Path(identity["executable"]) != Path(executable).resolve():
        raise ValueError("启动产物与进程的实际可执行文件不一致")
    receipt = {"format_version": 1, "role": role, "scope_id": scope, "identity": identity}
    write_receipt(directory / f"{role}.json", receipt)
    return receipt


def read_process(directory: Path, role: str, scope: str) -> dict:
    if role not in PROCESS_ROLES:
        raise ValueError("未知全栈进程角色")
    receipt = json.loads((directory / f"{role}.json").read_text(encoding="utf-8"))
    if any(receipt.get(key) != value for key, value in {
        "format_version": 1, "role": role, "scope_id": scope,
    }.items()) or not isinstance(receipt.get("identity"), dict):
        raise ValueError("测试进程收据的角色或隔离 scope 不匹配")
    return receipt["identity"]
