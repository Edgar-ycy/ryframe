"""仅通过固定受管绑定调用 Rust WHATWG origin 校验器，逐次复核来源。"""

from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import threading

from artifact_digests import file_digest, protect_binary_invocation
from restore_frontend_context import exact, hex_value
from restore_frontend_paths import descriptor, failure, json_object, ordinary, read_bytes

ROOT = Path(__file__).absolute().parents[2]
BINDING = ".local-tests/frontend-origin-tool.json"
POLICY = "xtask/source-inventory.json"
TIMEOUT = 10
METADATA_LIMIT = 4 * 1024 * 1024
STDERR_LIMIT = 64 * 1024
INPUT_LIMIT = 64 * 1024 * 1024
# URL 规范化与 JSON 转义预留三倍字节预算，另保留固定协议开销。
RESPONSE_LIMIT = INPUT_LIMIT * 3 + 64 * 1024
SAFE_ERRORS = {
    "地址为空：请提供 HTTP(S) origin，或显式允许空值。": "地址为空",
    "地址格式无效：请提供有效的绝对 HTTP(S) origin 并核对主机与端口。": "地址格式无效",
    "地址协议无效：请使用具有非空 origin 的 HTTP 或 HTTPS 地址。": "地址协议无效",
    "地址包含身份信息：请移除非空用户名和密码。": "地址包含身份信息",
    "地址包含额外内容：请移除根路径以外的路径及非空查询和片段。": "地址包含额外内容",
}


class _ToolFailure(ValueError):
    """已脱敏且带固定修复建议的工具错误。"""


def _error(label: str, reason: str) -> _ToolFailure:
    return _ToolFailure(
        f"{label}：Rust origin 校验 {reason}；请核对所指文件，"
        "由主控重新编译当前源码并通过 __frontend-origin describe 重建固定绑定后重试"
    )


def _executable_suffix(raw: str, *, platform: str) -> bool:
    return platform != "nt" or PureWindowsPath(raw).suffix.lower() == ".exe"


def _path(raw: object, label: str, *, executable: bool = False) -> Path:
    if (
        not isinstance(raw, str)
        or not raw
        or not Path(raw).is_absolute()
        or any(char in raw for char in "\0\r\n")
        or any(part in (".", "..") for part in raw.replace("\\", "/").split("/"))
        or os.name == "nt"
        and (not re.fullmatch(r"[a-zA-Z]:", Path(raw).drive) or ":" in raw[2:])
        or executable
        and not _executable_suffix(raw, platform=os.name)
    ):
        raise _error(label, "路径无效，必须使用无 ParentDir 跳转的普通本机绝对路径") from None
    try:
        return ordinary(raw)
    except (OSError, ValueError):
        raise _error(label, "路径不可读或含链接、重解析点、非普通文件") from None


def _state(path: Path) -> tuple:
    info = path.stat(follow_symlinks=False)
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _snapshot(path: Path) -> tuple:
    label = str(path)
    try:
        _path(label, label)
        before = _state(path)
        raw = read_bytes(label)
        after = _state(path)
        if before != after:
            raise _error(label, "读取期间文件身份或内容发生漂移") from None
        return before, raw, descriptor(ROOT, path, raw)["sha256"]
    except _ToolFailure:
        raise
    except (OSError, ValueError):
        raise _error(label, "文件读取失败或读取期间发生漂移") from None


def _metadata(raw: bytes, label: str) -> dict:
    try:
        if len(raw) > METADATA_LIMIT:
            raise ValueError
        value = exact(json_object(raw, label), {"format_version", "kind", "executable", "sources"}, label)
        executable = exact(value["executable"], {"path", "sha256"}, label)
        if (
            type(value["format_version"]) is not int
            or value["format_version"] != 1
            or value["kind"] != "frontend-origin-tool"
            or not hex_value(executable["sha256"], 64)
            or not isinstance(value["sources"], list)
            or not value["sources"]
        ):
            raise ValueError
        previous = ""
        for item in value["sources"]:
            item = exact(item, {"path", "sha256"}, label)
            if (not isinstance(item["path"], str) or item["path"] <= previous
                or any(char in item["path"] for char in "\\:\0\r\n")
                or any(part in ("", ".", "..") for part in item["path"].split("/"))
                or not hex_value(item["sha256"], 64)):
                raise ValueError
            previous = item["path"]
        if POLICY not in [item["path"] for item in value["sources"]]:
            raise ValueError
        return value
    except (ValueError, TypeError, KeyError):
        raise _error(label, "绑定 UTF8/JSON、精确字段、版本、来源顺序或摘要无效") from None


def _source_paths() -> list[str]:
    policy = exact(json_object(_snapshot(ROOT / POLICY)[1], POLICY), {"format_version", "files", "trees", "excluded_directories"}, POLICY)
    if type(policy["format_version"]) is not int or policy["format_version"] != 1:
        raise ValueError("来源规则版本无效")
    for key in ("files", "trees", "excluded_directories"):
        if not isinstance(policy[key], list) or any(not isinstance(name, str) or not name or any(c in name for c in "\\:\0\r\n") or any(p in ("", ".", "..") for p in name.split("/")) for name in policy[key]):
            raise ValueError("来源规则路径无效")
    if policy["excluded_directories"]:
        raise ValueError("来源规则不得按目录名排除文件")
    paths = set()
    def walk(relative):
        file = ROOT / relative
        info = file.lstat()
        if file.is_symlink() or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError("来源链接无效")
        if file.is_dir():
            for child in file.iterdir():
                walk(relative + "/" + child.name)
        else:
            _path(str(file), relative)
            paths.add(relative)
    for name in policy["files"] + policy["trees"]:
        walk(name)
    return sorted(paths)


def _verify_source_set(value):
    try:
        if _source_paths() != [item["path"] for item in value["sources"]]:
            raise ValueError
    except (OSError, ValueError, TypeError, KeyError):
        raise _error(str(ROOT / POLICY), "完整来源文件集合漂移") from None


def _unchanged(inputs: list) -> None:
    for path, before in inputs:
        if _snapshot(path) != before:
            raise _error(str(path), "绑定或源码输入发生身份或内容漂移") from None


def _binary(path: Path, expected: str) -> tuple:
    label = str(path)
    try:
        _path(label, label, executable=True)
        before = _state(path)
        digest = file_digest(path)
        if _state(path) != before or digest["sha256"] != expected:
            raise _error(label, "程序身份或 SHA-256 与绑定不符，二进制已过时或被替换") from None
        return before, digest
    except _ToolFailure:
        raise
    except (OSError, ValueError):
        raise _error(label, "程序摘要或文件身份无法读取") from None


def _executable_paths(raw: object, label: str) -> tuple[Path, Path | None]:
    if os.name == "nt" and isinstance(raw, str) and raw.startswith("\\\\?\\"):
        if re.match(r"^\\\\\?\\[a-zA-Z]:\\", raw) is None:
            raise _error(label, "路径无效，device 路径仅允许显式本机 drive，不接受 UNC 或其他设备") from None
        # 原绑定与路径字节不变；普通路径仍完整经过链接和重解析点检查。
        return _path(raw[4:], label, executable=True), Path(raw)
    return _path(raw, label, executable=True), None


def _program_state(path: Path, alias: Path | None, expected: str) -> tuple:
    binary = _binary(path, expected)
    if alias is None:
        return binary, None
    try:
        before = _state(alias)
        if not alias.samefile(path):
            raise _error(str(path), "device 原路径与普通路径不是同一文件") from None
        digest = file_digest(alias)
        if _state(alias) != before or digest["sha256"] != expected:
            raise _error(str(path), "device 原路径文件身份或实际 SHA-256 不符") from None
        return binary, (before, digest)
    except _ToolFailure:
        raise
    except (OSError, ValueError):
        raise _error(str(path), "device 原路径 samefile 或实际摘要复核失败") from None


@contextmanager
def _verified_tool():
    binding = ROOT / BINDING
    observed = _snapshot(binding)
    value = _metadata(observed[1], str(binding))
    inputs = [(binding, observed)]
    for item in value["sources"]:
        path = ROOT / item["path"]
        source = _snapshot(path)
        if source[2] != item["sha256"]:
            raise _error(str(path), "源码 SHA-256 与编译来源不符，二进制已过时") from None
        inputs.append((path, source))
    _verify_source_set(value)
    executable = value["executable"]
    path, alias = _executable_paths(executable["path"], str(binding))
    before = _program_state(path, alias, executable["sha256"])
    protected = {"path": str(path), "sha256": executable["sha256"]}
    try:
        with protect_binary_invocation(protected):
            try:
                _unchanged(inputs)
                _verify_source_set(value)
                if _program_state(path, alias, executable["sha256"]) != before:
                    raise _error(str(path), "获取保护期间程序发生漂移") from None
                yield path
            finally:
                _unchanged(inputs)
                _verify_source_set(value)
                if _program_state(path, alias, executable["sha256"]) != before:
                    raise _error(str(path), "调用期间程序发生身份或内容漂移") from None
    except _ToolFailure:
        raise
    except (OSError, ValueError):
        raise _error(str(path), "普通 PE 程序保护或收尾身份复核失败") from None


def _exchange(path: Path, request: bytes) -> tuple[int, bytes]:
    """并发排空有界管道；stdin 写入也受进程总超时控制。"""
    output, failed, oversized = bytearray(), threading.Event(), threading.Event()
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    with subprocess.Popen(
        [str(path), "__frontend-origin", "check"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **options,
    ) as process:

        def send():
            try:
                process.stdin.write(request)
                process.stdin.close()
            except (OSError, ValueError):
                failed.set()

        def receive(stream, *, retain: bool, limit: int):
            count = 0
            try:
                while chunk := stream.read(4096):
                    count += len(chunk)
                    if count > limit:
                        oversized.set()
                        process.kill()
                        break
                    if retain:
                        output.extend(chunk)
            except (OSError, ValueError):
                failed.set()

        threads = [
            threading.Thread(target=send, daemon=True),
            threading.Thread(
                target=receive, args=(process.stdout,), kwargs={"retain": True, "limit": RESPONSE_LIMIT}, daemon=True
            ),
            threading.Thread(
                target=receive, args=(process.stderr,), kwargs={"retain": False, "limit": STDERR_LIMIT}, daemon=True
            ),
        ]
        try:
            for thread in threads:
                thread.start()
            try:
                code = process.wait(timeout=TIMEOUT)
            except subprocess.TimeoutExpired:
                raise _error(str(path), "调用超时，已终止工具") from None
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=1)
            for thread in threads:
                if thread.ident is not None:
                    thread.join(timeout=1)
        if oversized.is_set() or failed.is_set() or any(thread.is_alive() for thread in threads):
            raise _error(str(path), "管道读取失败、stdout 超过响应上限或 stderr 超过 64 KiB") from None
    return code, bytes(output)


def _response(code: int, raw: bytes, *, allow_empty: bool, label: str) -> str:
    try:
        if type(code) is not int or len(raw) > RESPONSE_LIMIT:
            raise ValueError
        value = json_object(raw, label)
        if (
            type(value.get("format_version")) is not int
            or value["format_version"] != 1
            or type(value.get("valid")) is not bool
        ):
            raise ValueError
        if code == 0 and value["valid"]:
            exact(value, {"format_version", "valid", "origin"}, label)
            origin = value["origin"]
            if (
                not isinstance(origin, str)
                or not origin
                and not allow_empty
                or origin
                and (
                    not origin.startswith(("http://", "https://"))
                    or not origin.isascii()
                    or not origin.split("://", 1)[1]
                    or any(ord(char) <= 32 or ord(char) == 127 for char in origin)
                    or any(char in origin.split("://", 1)[1] for char in "/?#@\\")
                )
            ):
                raise ValueError
            return origin
        if code == 1 and not value["valid"]:
            exact(value, {"format_version", "valid", "error"}, label)
            category = SAFE_ERRORS.get(value["error"]) if isinstance(value["error"], str) else None
            if category is not None:
                raise _error(label, f"拒绝 URL（{category}），请修正配置中的 HTTP(S) origin") from None
        raise ValueError
    except _ToolFailure:
        raise
    except (ValueError, TypeError, KeyError):
        reason = "工具协议或参数错误（exit 2）" if code == 2 else "响应协议与退出状态不匹配"
        raise _error(label, reason) from None


def normalize_origin(value: str, *, allow_empty: bool) -> str:
    """使用当前绑定的 Rust 工具返回规范 origin；缺失绑定必须失败。"""
    label = str(ROOT / BINDING)
    if not isinstance(value, str) or type(allow_empty) is not bool:
        raise failure(label, "origin 必须为字符串，allow_empty 必须为 bool")
    try:
        request = json.dumps(
            {"format_version": 1, "value": value, "allow_empty": allow_empty}, ensure_ascii=True
        ).encode("utf-8")
        if len(request) > INPUT_LIMIT:
            raise _error(label, "请求超过 64 MiB") from None
        with _verified_tool() as path:
            code, raw = _exchange(path, request)
            return _response(code, raw, allow_empty=allow_empty, label=str(path))
    except _ToolFailure:
        raise
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
        raise _error(label, "调用、文件保护或协议处理失败") from None
