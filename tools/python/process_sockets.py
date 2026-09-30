"""只读核验测试探针的 loopback 监听端口归属，不按端口终止进程。"""

from __future__ import annotations

import ctypes
import ipaddress
import os
import socket
from pathlib import Path
from urllib.parse import urlsplit


def endpoint(url: str) -> tuple[int, str, int]:
    parsed = urlsplit(url)
    if (parsed.scheme not in ("http", "https") or parsed.hostname not in ("127.0.0.1", "::1")
            or parsed.username or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("恢复运行收据只接受明确的本机 loopback 端点")
    return (socket.AF_INET6 if parsed.hostname == "::1" else socket.AF_INET,
            parsed.hostname, parsed.port or (443 if parsed.scheme == "https" else 80))


def _linux_listener(pid: int, family: int, address: str, port: int) -> bool:
    root = Path(f"/proc/{pid}")
    sockets = set()
    for entry in (root / "fd").iterdir():
        try:
            target = os.readlink(entry)
        except FileNotFoundError:
            continue
        if target.startswith("socket:["):
            sockets.add(target[8:-1])
    table = "tcp6" if family == socket.AF_INET6 else "tcp"
    for line in (root / "net" / table).read_text().splitlines()[1:]:
        columns = line.split()
        raw_address, raw_port = columns[1].split(":")
        if columns[3] != "0A" or columns[9] not in sockets or int(raw_port, 16) != port:
            continue
        raw = bytes.fromhex(raw_address)
        host = socket.inet_ntop(family, b"".join(raw[i:i + 4][::-1] for i in range(0, len(raw), 4)))
        if host == address or ipaddress.ip_address(host).is_unspecified:
            return True
    return False


def _windows_listener(pid: int | None, family: int, address: str, port: int) -> bool:
    from ctypes import wintypes

    class IPv4Row(ctypes.Structure):
        _fields_ = [(name, wintypes.DWORD) for name in
                    ("state", "local_address", "local_port", "remote_address", "remote_port", "pid")]

    class IPv6Row(ctypes.Structure):
        _fields_ = [("local_address", ctypes.c_ubyte * 16), ("local_scope", wintypes.DWORD),
                    ("local_port", wintypes.DWORD), ("remote_address", ctypes.c_ubyte * 16),
                    ("remote_scope", wintypes.DWORD), ("remote_port", wintypes.DWORD),
                    ("state", wintypes.DWORD), ("pid", wintypes.DWORD)]

    api = ctypes.WinDLL("iphlpapi", use_last_error=True).GetExtendedTcpTable
    api.argtypes = (ctypes.c_void_p, ctypes.POINTER(wintypes.DWORD), wintypes.BOOL,
                    wintypes.ULONG, ctypes.c_int, wintypes.ULONG)
    size = wintypes.DWORD()
    result = api(None, ctypes.byref(size), False, family, 3, 0)  # TCP_TABLE_OWNER_PID_LISTENER
    if result not in (0, 122):
        raise OSError(result, "无法读取恢复进程监听表")
    for _ in range(3):
        buffer = ctypes.create_string_buffer(size.value)
        result = api(buffer, ctypes.byref(size), False, family, 3, 0)
        if result == 122:
            continue
        if result != 0:
            raise OSError(result, "无法读取恢复进程监听表")
        row_type = IPv6Row if family == socket.AF_INET6 else IPv4Row
        count = wintypes.DWORD.from_buffer(buffer).value
        for index in range(count):
            row = row_type.from_buffer(buffer, ctypes.sizeof(wintypes.DWORD) + index * ctypes.sizeof(row_type))
            if (pid is not None and row.pid != pid) or socket.ntohs(row.local_port & 0xFFFF) != port:
                continue
            raw = bytes(row.local_address) if family == socket.AF_INET6 else int(row.local_address).to_bytes(4, "little")
            host = socket.inet_ntop(family, raw)
            if host == address or ipaddress.ip_address(host).is_unspecified:
                return True
        return False
    raise ValueError("恢复进程监听表持续变化")


def verify_listener(pid: int, url: str) -> None:
    family, address, port = endpoint(url)
    found = (_windows_listener if os.name == "nt" else _linux_listener)(pid, family, address, port)
    if not found:
        raise ValueError("恢复探针端口不属于已登记的 API 或 Worker 进程")


def verify_windows_port_idle(url: str) -> None:
    """读取内核完整监听表；未知、权限错误与持续变化都拒绝，不发起连接或清理。"""
    if os.name != "nt":
        raise ValueError("Windows 监听表证明不能用于其他平台")
    family, address, port = endpoint(url)
    found = _windows_listener(None, family, address, port)
    # IPv6 wildcard 可能以 dual-stack 同时接收 IPv4；无法区分时保守拒绝。
    if family == socket.AF_INET:
        found = _windows_listener(None, socket.AF_INET6, "::", port) or found
    if found:
        raise ValueError("明确测试端口已有监听，不能声明生产者停止")
