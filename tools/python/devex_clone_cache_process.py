"""缓存原生协议的 Python 调用适配与证据文件读写。"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path, PureWindowsPath
import re
import uuid


def canonical(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def read(path):
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("缓存进程证据必须是对象")
    return value


def write(path, value):
    path = Path(path)
    pending = path.with_name("." + path.name + "." + uuid.uuid4().hex + ".pending")
    with pending.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    # 两端只能看到完整收据；link 独占发布，不能覆盖已经登记的证据。
    os.link(pending, path)
    pending.unlink()


def bound(path):
    path = Path(path)
    with path.open("rb") as stream:
        value = hashlib.file_digest(stream, "sha256").hexdigest()
        size = os.fstat(stream.fileno()).st_size
    return {"path": str(path), "bytes": size, "sha256": value}


def plain(path):
    path = Path(path)
    if (not path.is_absolute() or not path.exists() or any(item.is_symlink() or
            getattr(item.lstat(), "st_file_attributes", 0) & 0x400 for item in (path, *path.parents))):
        raise ValueError("缓存控制只接受明确无链接的已有绝对路径")
    return path


def linux_path(value):
    path = PureWindowsPath(value)
    if (not path.is_absolute() or not re.fullmatch(r"[A-Za-z]:", path.drive)
            or ".." in path.parts or any(":" in part for part in path.parts[1:])):
        raise ValueError("WSL 仅映射已登记 Windows 盘符绝对路径")
    return "/mnt/" + path.drive[0].lower() + "/" + "/".join(path.parts[1:])


def validate(request, *, files=True):
    from devex_clone_cache_native import invoke
    invoke("validate", request=request, files=files)
    if files:
        directory = plain(request["directory"]["path"])
        identity = request["directory"]
        if (not directory.is_dir() or type(identity["device"]) is not int
                or type(identity["inode"]) is not int
                or (directory.stat().st_dev, directory.stat().st_ino) != (identity["device"], identity["inode"])):
            raise ValueError("Redis 原 Windows 数据目录身份变化")


def inspect_start(request, output):
    from devex_clone_cache_native import invoke
    return invoke("inspect", request=request, output=str(output))


def start(backend, request, environment, output, guard):
    from devex_clone_cache_native import exchange
    def port(operation, arguments):
        if operation != "checkpoint" or arguments is not None:
            raise ValueError("缓存启动拒绝未知回调")
        return guard()
    return exchange({"format_version": 1, "kind": "ryframe-cache-process", "operation": "start-session",
                     "arguments": {"request": request, "private": environment, "output": str(output)}}, port)


def observe(backend, request, environment, runtime, output=None):
    from devex_clone_cache_native import invoke
    result = invoke("observe", request=request, runtime=runtime)
    if output is not None:
        write(Path(output) / "observed.json", result)
    return result


def stop(backend, request, environment, runtime, output):
    from devex_clone_cache_native import invoke
    return invoke("stop", request=request, runtime=runtime, output=str(output))
