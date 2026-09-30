"""核验正式监控凭据的所有权、访问边界和稳定字节身份。"""

from __future__ import annotations

import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import stat

from artifact_digests import protect_files
from restore_runtime_evidence import artifact_snapshot, exact_fields, reject_link_or_reparse


TOKEN_NAME = "metrics-token.txt"
MAX_TOKEN_BYTES = 8192
SYSTEM_SID = "S-1-5-18"
ADMINISTRATORS_SID = "S-1-5-32-544"
WRITE_RIGHTS = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x10000 | 0x40000 | 0x80000
READ_DATA = 0x1


class _AclSize(ctypes.Structure):
    _fields_ = [
        ("ace_count", wintypes.DWORD),
        ("bytes_in_use", wintypes.DWORD),
        ("bytes_free", wintypes.DWORD),
    ]


class _AceHeader(ctypes.Structure):
    _fields_ = [
        ("ace_type", wintypes.BYTE),
        ("ace_flags", wintypes.BYTE),
        ("ace_size", wintypes.WORD),
    ]


class _SidAndAttributes(ctypes.Structure):
    _fields_ = [("sid", ctypes.c_void_p), ("attributes", wintypes.DWORD)]


class _TokenUser(ctypes.Structure):
    _fields_ = [("user", _SidAndAttributes)]


def _windows_libraries():
    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    advapi.GetNamedSecurityInfoW.argtypes = [
        wintypes.LPWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_void_p),
    ]
    advapi.GetNamedSecurityInfoW.restype = wintypes.DWORD
    advapi.GetSecurityDescriptorControl.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(wintypes.WORD),
        ctypes.POINTER(wintypes.DWORD),
    ]
    advapi.GetSecurityDescriptorControl.restype = wintypes.BOOL
    advapi.GetAclInformation.argtypes = [
        ctypes.c_void_p,
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
    ]
    advapi.GetAclInformation.restype = wintypes.BOOL
    advapi.GetAce.argtypes = [ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p)]
    advapi.GetAce.restype = wintypes.BOOL
    advapi.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.LPWSTR)]
    advapi.ConvertSidToStringSidW.restype = wintypes.BOOL
    advapi.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    advapi.OpenProcessToken.restype = wintypes.BOOL
    advapi.GetTokenInformation.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD),
    ]
    advapi.GetTokenInformation.restype = wintypes.BOOL
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.LocalFree.argtypes = [ctypes.c_void_p]
    kernel.LocalFree.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    return advapi, kernel


def _sid_string(advapi, kernel, pointer) -> str:
    value = wintypes.LPWSTR()
    if not advapi.ConvertSidToStringSidW(pointer, ctypes.byref(value)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return value.value
    finally:
        kernel.LocalFree(value)


def _current_sid(advapi, kernel) -> str:
    token = wintypes.HANDLE()
    if not advapi.OpenProcessToken(kernel.GetCurrentProcess(), 0x0008, ctypes.byref(token)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        required = wintypes.DWORD()
        advapi.GetTokenInformation(token, 1, None, 0, ctypes.byref(required))
        if not required.value:
            raise ctypes.WinError(ctypes.get_last_error())
        buffer = ctypes.create_string_buffer(required.value)
        if not advapi.GetTokenInformation(token, 1, buffer, len(buffer), ctypes.byref(required)):
            raise ctypes.WinError(ctypes.get_last_error())
        return _sid_string(advapi, kernel, ctypes.cast(buffer, ctypes.POINTER(_TokenUser)).contents.user.sid)
    finally:
        kernel.CloseHandle(token)


def _acl_entries(advapi, kernel, dacl) -> list[dict]:
    information = _AclSize()
    if not advapi.GetAclInformation(dacl, ctypes.byref(information), ctypes.sizeof(information), 2):
        raise ctypes.WinError(ctypes.get_last_error())
    result = []
    for index in range(information.ace_count):
        pointer = ctypes.c_void_p()
        if not advapi.GetAce(dacl, index, ctypes.byref(pointer)):
            raise ctypes.WinError(ctypes.get_last_error())
        header = _AceHeader.from_address(pointer.value)
        if header.ace_type not in {0, 1} or header.ace_size < 12:
            raise ValueError("监控凭据 ACL 包含无法核验的 ACE 类型")
        result.append(
            {
                "sid": _sid_string(advapi, kernel, pointer.value + 8),
                "type": header.ace_type,
                "rights": ctypes.c_uint32.from_address(pointer.value + 4).value,
                "inherited": bool(header.ace_flags & 0x10),
            }
        )
    return result


def _windows_acl(path: Path) -> dict:
    advapi, kernel = _windows_libraries()
    owner = ctypes.c_void_p()
    dacl = ctypes.c_void_p()
    descriptor = ctypes.c_void_p()
    code = advapi.GetNamedSecurityInfoW(
        str(path), 1, 0x00000001 | 0x00000004, ctypes.byref(owner), None,
        ctypes.byref(dacl), None, ctypes.byref(descriptor),
    )
    if code:
        raise ctypes.WinError(code)
    try:
        control, revision = wintypes.WORD(), wintypes.DWORD()
        if not dacl or not advapi.GetSecurityDescriptorControl(
            descriptor, ctypes.byref(control), ctypes.byref(revision)
        ):
            raise ValueError("监控凭据缺少可核验的 Windows DACL")
        value = {
            "current_sid": _current_sid(advapi, kernel),
            "owner_sid": _sid_string(advapi, kernel, owner),
            "protected": bool(control.value & 0x1000),
            "entries": _acl_entries(advapi, kernel, dacl),
        }
        return validate_windows_acl(value)
    finally:
        kernel.LocalFree(descriptor)


def validate_windows_acl(value: object) -> dict:
    value = exact_fields(value, {"current_sid", "owner_sid", "protected", "entries"}, "监控凭据 ACL")
    current = value["current_sid"]
    owner = value["owner_sid"]
    entries = value["entries"]
    if (
        not isinstance(current, str)
        or not current.startswith("S-1-")
        or owner not in {current, SYSTEM_SID, ADMINISTRATORS_SID}
        or value["protected"] is not True
        or not isinstance(entries, list)
        or not entries
    ):
        raise ValueError("监控凭据必须由当前用户、System 或 Administrators 独占管理")
    allowed = {current, SYSTEM_SID, ADMINISTRATORS_SID}
    readable = False
    normalized = []
    for entry in entries:
        entry = exact_fields(entry, {"sid", "type", "rights", "inherited"}, "监控凭据 ACL 项")
        if (
            entry["sid"] not in allowed
            or entry["type"] != 0
            or type(entry["rights"]) is not int
            or entry["rights"] < 0
            or type(entry["inherited"]) is not bool
            or entry["inherited"]
        ):
            raise ValueError("监控凭据 ACL 包含未授权或继承的主体")
        if entry["sid"] == current and entry["type"] == 0 and entry["rights"] & READ_DATA:
            readable = True
        normalized.append(entry)
    if not readable:
        raise ValueError("当前用户没有读取监控凭据的明确权限")
    return {
        "platform": "windows",
        "current_sid": current,
        "owner_sid": owner,
        "entries": sorted(normalized, key=lambda item: (item["sid"], item["type"], item["rights"])),
    }


def _posix_permissions(path: Path) -> dict:
    metadata = path.stat()
    if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
        raise ValueError("监控凭据必须只允许当前用户访问")
    return {"platform": "posix", "owner": metadata.st_uid, "mode": stat.S_IMODE(metadata.st_mode)}


def _security(path: Path, acl_reader=None) -> dict:
    if acl_reader is not None:
        return acl_reader(path)
    if os.name == "nt":
        return _windows_acl(path)
    return _posix_permissions(path)


def read_private_token(path: Path, *, acl_reader=None) -> tuple[str, dict]:
    """在 ACL 和 Windows 保护句柄内重复读取，不返回或记录凭据原文。"""
    path = path.absolute()
    if path.name != TOKEN_NAME:
        raise ValueError(f"监控凭据必须使用固定文件名 {TOKEN_NAME}")
    reject_link_or_reparse(path)
    before_security = _security(path, acl_reader)
    snapshot = artifact_snapshot(path)
    if snapshot.bytes <= 0 or snapshot.bytes > MAX_TOKEN_BYTES:
        raise ValueError("监控凭据文件大小无效")
    binding = {"path": str(path), "sha256": snapshot.sha256}
    with protect_files([binding]):
        with path.open("rb") as stream:
            opened = os.fstat(stream.fileno())
            first = stream.read(MAX_TOKEN_BYTES + 1)
            stream.seek(0)
            second = stream.read(MAX_TOKEN_BYTES + 1)
            after_read = os.fstat(stream.fileno())
        if first != second or (opened.st_dev, opened.st_ino) != (after_read.st_dev, after_read.st_ino):
            raise ValueError("监控凭据在读取期间发生变化")
        snapshot.assert_unchanged()
        after_security = _security(path, acl_reader)
    if before_security != after_security:
        raise ValueError("监控凭据权限在读取期间发生变化")
    try:
        secret = first.decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError as error:
        raise ValueError("监控凭据不是 UTF-8") from error
    if not secret or "\x00" in secret:
        raise ValueError("监控凭据不能为空")
    security_raw = json.dumps(before_security, sort_keys=True, separators=(",", ":")).encode()
    return secret, {
        **snapshot.descriptor(),
        "device": snapshot.state[0],
        "inode": snapshot.state[1],
        "security_sha256": hashlib.sha256(security_raw).hexdigest(),
    }
