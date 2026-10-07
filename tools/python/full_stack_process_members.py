"""持有本次 Job/session 的内核成员句柄，并等待所有已登记成员真实退出。"""
from __future__ import annotations

from contextlib import ExitStack
import ctypes
import os
from pathlib import Path
import select
import signal
import time

from full_stack_process import _windows_handle, _windows_identity, assert_identity, process_identity


def _same_process_creation(identity: dict | None, started: str) -> bool:
    """pidfd 打开后只用创建时刻排除 PID 复用；进程可在此期间完成 exec。"""
    return identity is not None and identity["started"] == started


def _matches_process_snapshot(identity: dict | None, fields: list[str]) -> bool:
    """进程快照过期或 PID 已复用时忽略它，不把新进程纳入原进程树。"""
    return _same_process_creation(identity, fields[19])


def job_name(operation_id: str) -> str:
    return "Local\\RyFrameFullStack-" + operation_id


def windows_kernel():
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    signatures = {
        "CreateJobObjectW": ((ctypes.c_void_p, wintypes.LPCWSTR), wintypes.HANDLE),
        "OpenJobObjectW": ((wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR), wintypes.HANDLE),
        "SetInformationJobObject": ((wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD), wintypes.BOOL),
        "QueryInformationJobObject": ((wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p), wintypes.BOOL),
        "AssignProcessToJobObject": ((wintypes.HANDLE, wintypes.HANDLE), wintypes.BOOL),
        "IsProcessInJob": ((wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)), wintypes.BOOL),
        "TerminateJobObject": ((wintypes.HANDLE, wintypes.UINT), wintypes.BOOL),
        "WaitForSingleObject": ((wintypes.HANDLE, wintypes.DWORD), wintypes.DWORD),
        "CloseHandle": ((wintypes.HANDLE,), wintypes.BOOL),
        "GetCurrentProcess": ((), wintypes.HANDLE),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(kernel, name)
        function.argtypes, function.restype = arguments, result
    return kernel


def join_windows_job(operation_id: str) -> None:
    kernel = windows_kernel()
    handle = kernel.OpenJobObjectW(0x0001, False, job_name(operation_id))
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if not kernel.AssignProcessToJobObject(handle, kernel.GetCurrentProcess()):
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        kernel.CloseHandle(handle)


class WindowsMembers:
    def __init__(self, operation_id: str, supervisor: dict):
        from full_stack_process_tree import _ExtendedLimitInformation

        self.kernel = windows_kernel()
        self.stack, self.members = ExitStack(), {}
        self.handle = self.kernel.CreateJobObjectW(None, job_name(operation_id))
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        if ctypes.get_last_error() == 183:
            self.kernel.CloseHandle(self.handle)
            raise ValueError("进程树 Job 名称已经被占用，拒绝接管")
        self.stack.callback(self.kernel.CloseHandle, self.handle)
        try:
            information = _ExtendedLimitInformation()
            information.BasicLimitInformation.LimitFlags = 0x2000
            if not self.kernel.SetInformationJobObject(self.handle, 9, ctypes.byref(information), ctypes.sizeof(information)):
                raise ctypes.WinError(ctypes.get_last_error())
            kernel, handle = self.stack.enter_context(_windows_handle(supervisor["pid"]))
            if not assert_identity(_windows_identity(kernel, handle, supervisor["pid"]), supervisor):
                raise ValueError("监督进程在建立树边界前退出")
            self.supervisor_handle = handle
            self.members[(supervisor["pid"], supervisor["started"])] = (supervisor, handle)
        except BaseException:
            self.stack.close()
            raise

    def joined(self) -> bool:
        from ctypes import wintypes

        value = wintypes.BOOL()
        if not self.kernel.IsProcessInJob(self.supervisor_handle, self.handle, ctypes.byref(value)):
            raise ctypes.WinError(ctypes.get_last_error())
        return bool(value.value)

    def supervisor_exited(self) -> bool:
        return self.signaled(self.supervisor_handle)

    def signaled(self, handle) -> bool:
        result = self.kernel.WaitForSingleObject(handle, 0)
        if result not in (0, 258):
            raise ctypes.WinError(ctypes.get_last_error())
        return result == 0

    def capture(self) -> int:
        from ctypes import wintypes

        capacity = 64
        while True:
            class ProcessIds(ctypes.Structure):
                _fields_ = [("assigned", wintypes.DWORD), ("count", wintypes.DWORD),
                            ("pids", ctypes.c_size_t * capacity)]
            value = ProcessIds()
            success = self.kernel.QueryInformationJobObject(self.handle, 3, ctypes.byref(value), ctypes.sizeof(value), None)
            if not success and ctypes.get_last_error() != 234:
                raise ctypes.WinError(ctypes.get_last_error())
            if success and value.count == value.assigned:
                break
            capacity = max(capacity * 2, value.assigned)
            if capacity > 131072:
                raise ValueError("进程树成员数超出受控上限")
        for pid in value.pids[:value.count]:
            if any(key[0] == pid and not self.signaled(handle) for key, (_, handle) in self.members.items()):
                continue
            kernel, handle = self.stack.enter_context(_windows_handle(pid))
            identity = _windows_identity(kernel, handle, pid)
            if identity is None:
                continue
            belongs = wintypes.BOOL()
            if not self.kernel.IsProcessInJob(handle, self.handle, ctypes.byref(belongs)):
                raise ctypes.WinError(ctypes.get_last_error())
            if not belongs.value:
                raise ValueError("进程树查询后成员 PID 已复用或离开原 Job")
            self.members[(pid, identity["started"])] = (identity, handle)
        return value.count

    def terminate(self) -> None:
        if not self.kernel.TerminateJobObject(self.handle, 137):
            raise ctypes.WinError(ctypes.get_last_error())

    def pending(self) -> bool:
        return any(not self.signaled(handle) for _, handle in self.members.values())

    def identities(self) -> list[dict]:
        return [identity for identity, _ in self.members.values()]

    def close(self) -> None:
        self.stack.close()


class UnixMembers:
    def __init__(self, _operation_id: str, supervisor: dict):
        self.supervisor, self.members = supervisor, {}
        self.supervisor_fd = os.pidfd_open(supervisor["pid"])
        try:
            if not assert_identity(process_identity(supervisor["pid"]), supervisor):
                raise ValueError("监督进程在建立树边界前退出")
            self.members[(supervisor["pid"], supervisor["started"])] = (supervisor, os.dup(self.supervisor_fd))
        except BaseException:
            os.close(self.supervisor_fd)
            raise

    def joined(self) -> bool:
        pid = self.supervisor["pid"]
        return os.getsid(pid) == pid and os.getpgid(pid) == pid

    def supervisor_exited(self) -> bool:
        return bool(select.select([self.supervisor_fd], [], [], 0)[0])

    def capture(self) -> int:
        count = 0
        for path in Path("/proc").iterdir():
            if not path.name.isdigit():
                continue
            try:
                fields = (path / "stat").read_text().rpartition(")")[2].split()
                if int(fields[2]) != self.supervisor["pid"] or int(fields[3]) != self.supervisor["pid"]:
                    continue
                if fields[0] == "Z":
                    continue
                identity = process_identity(int(path.name))
                if identity is None:
                    continue
                key = (identity["pid"], identity["started"])
                if key not in self.members:
                    descriptor = os.pidfd_open(identity["pid"])
                    actual = process_identity(identity["pid"])
                    if not _matches_process_snapshot(actual, fields):
                        os.close(descriptor)
                        continue
                    key = (actual["pid"], actual["started"])
                    self.members[key] = (actual, descriptor)
                count += 1
            except (FileNotFoundError, ProcessLookupError):
                continue
        return count

    def terminate(self) -> None:
        actual = process_identity(self.supervisor["pid"])
        if actual is not None and actual != self.supervisor:
            raise ValueError("原进程组创建身份已复用，拒绝发送信号")
        try:
            os.killpg(self.supervisor["pid"], signal.SIGKILL)
        except ProcessLookupError:
            pass

    def pending(self) -> bool:
        return any(not select.select([descriptor], [], [], 0)[0] for _, descriptor in self.members.values())

    def identities(self) -> list[dict]:
        return [identity for identity, _ in self.members.values()]

    def close(self) -> None:
        for _, descriptor in self.members.values():
            os.close(descriptor)
        os.close(self.supervisor_fd)


def finish_members(members, timeout: float = 5) -> list[dict]:
    """先冻结实际成员句柄再终止；空成员列表不能替代已捕获句柄的退出确认。"""
    members.capture()
    members.terminate()
    deadline = time.monotonic() + timeout
    while True:
        remaining = members.capture()
        if not remaining and not members.pending():
            return sorted(members.identities(), key=lambda identity: (identity["pid"], identity["started"]))
        if time.monotonic() >= deadline:
            raise TimeoutError("完整进程树成员未在期限内退出")
        time.sleep(0.01)
