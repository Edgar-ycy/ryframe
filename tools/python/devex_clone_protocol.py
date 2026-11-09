"""严格解码 xtask 与开发复制私有阶段之间的版本化请求。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import stat
from types import SimpleNamespace


PROTOCOL_KIND = "ryframe-xtask-recovery-clone"
COMMANDS = {
    "plan", "verify", "init", "status", "stage", "runtime", "recover",
    "post-copy", "seed-runtime",
}


class CloneProtocolError(ValueError):
    """表示 clone 私有请求格式或命令合同无效。"""


def _strict_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise CloneProtocolError(f"clone 私有协议字段重复：{key}")
        value[key] = item
    return value


def _exact(value, fields, label):
    if not isinstance(value, dict) or set(value) != set(fields):
        raise CloneProtocolError(f"clone 私有协议 {label} 字段不匹配")


def _text(value, label):
    if (not isinstance(value, str) or not value.strip()
            or any(character in value for character in ("\r", "\n", "\0"))):
        raise CloneProtocolError(f"clone 私有协议 {label} 必须是非空字符串")
    return value


def _path(value, label, *, optional=False):
    if optional and value is None:
        return None
    path = Path(_text(value, label))
    if not path.is_absolute():
        raise CloneProtocolError(f"clone 私有协议 {label} 必须是绝对路径")
    if any(part in {".", ".."} for part in value.replace("\\", "/").split("/")):
        raise CloneProtocolError(f"clone 私有协议 {label} 不得包含路径跳转")
    return path


def _link_like(path, metadata):
    attributes = getattr(metadata, "st_file_attributes", 0)
    reparse = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    is_junction = getattr(os.path, "isjunction", lambda _path: False)
    return stat.S_ISLNK(metadata.st_mode) or bool(attributes & reparse) or is_junction(path)


def _existing_metadata(path, label):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise CloneProtocolError(f"clone 私有协议无法核验 {label}") from error
    if _link_like(path, metadata):
        raise CloneProtocolError(f"clone 私有协议 {label} 不得经过链接或 junction")
    return metadata


def _owned_path(backend, value, kind, label):
    boundary = backend / ".local-tests"
    boundary_metadata = _existing_metadata(boundary, ".local-tests")
    if boundary_metadata is None or not stat.S_ISDIR(boundary_metadata.st_mode):
        raise CloneProtocolError("clone 私有协议 .local-tests 必须是现有普通目录")
    try:
        relative = value.relative_to(boundary)
    except ValueError as error:
        raise CloneProtocolError(f"clone 私有协议 {label} 必须位于 .local-tests") from error
    if not relative.parts:
        raise CloneProtocolError(f"clone 私有协议 {label} 不能是 .local-tests 根目录")
    canonical_boundary = boundary.resolve(strict=True)
    current = boundary
    missing = False
    for index, part in enumerate(relative.parts):
        current /= part
        if missing:
            continue
        metadata = _existing_metadata(current, label)
        if metadata is None:
            missing = True
            continue
        if index + 1 < len(relative.parts) and not stat.S_ISDIR(metadata.st_mode):
            raise CloneProtocolError(f"clone 私有协议 {label} 的中间部分必须是目录")
        try:
            resolved = current.resolve(strict=True)
            resolved.relative_to(canonical_boundary)
        except (OSError, ValueError) as error:
            raise CloneProtocolError(f"clone 私有协议 {label} 解析后越界") from error
    metadata = _existing_metadata(value, label)
    if kind == "existing-file":
        valid = metadata is not None and stat.S_ISREG(metadata.st_mode)
    elif kind == "existing-directory":
        valid = metadata is not None and stat.S_ISDIR(metadata.st_mode)
    elif kind == "existing":
        valid = metadata is not None and (stat.S_ISREG(metadata.st_mode)
                                          or stat.S_ISDIR(metadata.st_mode))
    elif kind in {"output-file", "new-directory"}:
        valid = metadata is None
        parent = _existing_metadata(value.parent, f"{label} 父目录")
        valid = valid and parent is not None and stat.S_ISDIR(parent.st_mode)
    else:
        raise CloneProtocolError("clone 私有协议包含未知路径类型")
    if not valid:
        raise CloneProtocolError(f"clone 私有协议 {label} 的存在性或类型不匹配")


def _validate_paths(backend, command, options):
    mappings = {
        "plan": (("input", "existing-file"), ("output", "output-file")),
        "verify": (("plan", "existing-file"),),
        "init": (("manifest", "existing-file"), ("run_dir", "new-directory")),
        "status": (("run_dir", "existing-directory"),),
        "stage": (("run_dir", "existing-directory"),),
        "runtime": (("run_dir", "existing-directory"),),
        "recover": (("run_dir", "existing-directory"), ("owner_binding", "existing-file")),
        "post-copy": (("run_dir", "existing-directory"), ("request", "existing-file"),
                      ("producer_binding", "existing-file")),
        "seed-runtime": (("run_dir", "existing-directory"), ("request", "existing-file"),
                         ("producer_binding", "existing-file")),
    }
    for name, kind in mappings[command]:
        value = options.get(name)
        if value is not None:
            _owned_path(backend, value, kind, name)


def _backend(value):
    backend = _path(value, "backend_dir")
    expected = Path(__file__).resolve().parents[2]
    if os.path.normcase(os.path.abspath(backend)) != os.path.normcase(os.path.abspath(expected)):
        raise CloneProtocolError("clone 私有协议 backend_dir 不是当前后端")
    metadata = _existing_metadata(backend, "backend_dir")
    if metadata is None or not stat.S_ISDIR(metadata.st_mode):
        raise CloneProtocolError("clone 私有协议 backend_dir 必须是现有普通目录")
    return backend


def _enum(value, allowed, label):
    value = _text(value, label)
    if value not in allowed:
        raise CloneProtocolError(f"clone 私有协议 {label} 取值无效")
    return value


def effect(command, options):
    if command in {"verify", "status"}:
        return "read-only"
    if command in {"plan", "init", "recover"}:
        return "evidence-write"
    if command == "stage":
        return ("business-write" if options["stage"] == "copy"
                and options["mode"] != "reconcile" else "evidence-write")
    if command == "runtime":
        if options["operation"] == "status":
            return "read-only"
        return "evidence-write" if options["operation"] == "recover" else "business-write"
    if command == "post-copy":
        return ("evidence-write" if options["operation"] in {
            "register", "amend", "prepare", "reconcile", "verify",
        } else "business-write")
    if command == "seed-runtime":
        if options["operation"] in {
            "register", "quotas-plan", "quotas-reconcile", "departments-plan",
            "departments-reconcile", "departments-verify", "identities-verify",
            "prepare", "arm-input",
        }:
            return "evidence-write"
        return "business-write"
    raise CloneProtocolError("clone 私有协议 command 缺少副作用分类")


def _base(command, options):
    if command == "plan":
        _exact(options, {"input", "output"}, "plan options")
        return {"input": _path(options["input"], "input"),
                "output": _path(options["output"], "output")}
    if command == "verify":
        _exact(options, {"plan"}, "verify options")
        return {"plan": _path(options["plan"], "plan")}
    if command == "init":
        _exact(options, {"manifest", "run_dir"}, "init options")
        return {"manifest": _path(options["manifest"], "manifest"),
                "run_dir": _path(options["run_dir"], "run_dir")}
    if command == "status":
        _exact(options, {"run_dir"}, "status options")
        return {"run_dir": _path(options["run_dir"], "run_dir")}
    if command == "stage":
        _exact(options, {"run_dir", "stage", "mode"}, "stage options")
        stage = _enum(options["stage"], {"copy"}, "stage")
        mode = _enum(options["mode"], {"run", "reconcile", "resume"}, "mode")
        return {
            "run_dir": _path(options["run_dir"], "run_dir"),
            "stage": stage,
            "mode": mode,
        }
    if command == "recover":
        _exact(options, {"run_dir", "owner_binding"}, f"{command} options")
        return {"run_dir": _path(options["run_dir"], "run_dir"),
                "owner_binding": _path(options["owner_binding"], "owner_binding")}
    return None


def _runtime(options):
    _exact(options, {"run_dir", "side", "operation", "roles"}, "runtime options")
    roles = options["roles"]
    if (not isinstance(roles, list) or not roles or len(roles) > 2
            or any(not isinstance(role, str) or role not in {"api", "worker"} for role in roles)
            or len(set(roles)) != len(roles)):
        raise CloneProtocolError("clone 私有协议 roles 必须是无重复的 api/worker 列表")
    return {
        "run_dir": _path(options["run_dir"], "run_dir"),
        "side": _enum(options["side"], {"source", "target"}, "side"),
        "operation": _enum(options["operation"], {"start", "stop", "status", "recover"}, "operation"),
        "roles": roles,
    }


def _post_copy(options):
    _exact(options, {"run_dir", "operation", "request", "producer_binding"}, "post-copy options")
    operation = _enum(options["operation"], {
        "schedules", "reconcile", "verify", "recover-session",
    }, "operation")
    request = _path(options["request"], "request", optional=True)
    producer = _path(options["producer_binding"], "producer_binding", optional=True)
    if (request is not None
            or (producer is not None) != (operation == "recover-session")):
        raise CloneProtocolError("clone 私有协议 post-copy 路径与操作不匹配")
    return {"run_dir": _path(options["run_dir"], "run_dir"), "operation": operation,
            "request": request, "producer_binding": producer}


def _seed_runtime(options):
    _exact(options, {"run_dir", "operation", "request", "producer_binding"}, "seed-runtime options")
    operations = {
        "register", "quotas-plan", "quotas-apply", "quotas-reconcile",
        "departments-plan", "departments-apply", "departments-reconcile", "departments-verify",
        "identities-apply", "identities-verify", "prepare", "close", "arm-input",
        "recover-session",
    }
    operation = _enum(options["operation"], operations, "operation")
    request = _path(options["request"], "request", optional=True)
    producer = _path(options["producer_binding"], "producer_binding", optional=True)
    if ((request is not None) != (operation in {"register", "arm-input"})
            or (producer is not None) != (operation == "recover-session")):
        raise CloneProtocolError("clone 私有协议 seed-runtime 路径与操作不匹配")
    return {"run_dir": _path(options["run_dir"], "run_dir"), "operation": operation,
            "request": request, "producer_binding": producer}


def read_private_stdin(stream):
    """有界读取唯一 UTF-8 请求；按既有 kind 选择解码器前先拒绝重复字段。"""
    raw = stream.read(64 * 1024 + 1)
    if not isinstance(raw, bytes) or not raw or len(raw) > 64 * 1024:
        raise CloneProtocolError("复制私有 stdin 为空或超过 64 KiB")
    try:
        source = raw.decode("utf-8", errors="strict")
        value = json.loads(source, object_pairs_hook=_strict_object)
    except (UnicodeError, ValueError, TypeError, RecursionError) as error:
        raise CloneProtocolError("复制私有 stdin 不是严格 UTF-8 JSON") from error
    if (not isinstance(value, dict) or type(value.get("format_version")) is not int
            or value["format_version"] != 1 or not isinstance(value.get("kind"), str)):
        raise CloneProtocolError("复制私有 stdin 版本或类型无效")
    if value["kind"] == "ryframe-xtask-recovery-clone-stage" and len(raw) > 32 * 1024:
        raise CloneProtocolError("阶段私有 stdin 超过 32 KiB")
    return value["kind"], source


def decode(source: str):
    """解码唯一版本的 clone 请求，并构造既有阶段函数所需的命名参数。"""
    if (not isinstance(source, str) or not source or len(source.encode("utf-8")) > 64 * 1024
            or any(character in source for character in ("\r", "\n", "\0"))):
        raise CloneProtocolError("clone 私有协议为空、过长或包含换行/NUL")
    try:
        value = json.loads(source, object_pairs_hook=_strict_object)
    except (json.JSONDecodeError, TypeError, ValueError, RecursionError) as error:
        raise CloneProtocolError("clone 私有协议不是有效 JSON") from error
    _exact(value, {"format_version", "kind", "native_tool", "request"}, "根")
    if (type(value["format_version"]) is not int or value["format_version"] != 1
            or value["kind"] != PROTOCOL_KIND):
        raise CloneProtocolError("clone 私有协议版本或类型不受支持")
    from rate_limit_tool import parse_binding

    native_tool = parse_binding(json.dumps(value["native_tool"], ensure_ascii=False))
    request = value["request"]
    _exact(request, {"backend_dir", "command", "effect", "options", "write"}, "request")
    command = _enum(request["command"], COMMANDS, "command")
    if not isinstance(request["write"], bool):
        raise CloneProtocolError("clone 私有协议 write 必须是布尔值")
    options = request["options"]
    decoded = _base(command, options)
    if decoded is None:
        decoded = {
            "runtime": _runtime,
            "post-copy": _post_copy,
            "seed-runtime": _seed_runtime,
        }[command](options)
    expected_effect = effect(command, options)
    if request["effect"] != expected_effect:
        raise CloneProtocolError("clone 私有协议 effect 与操作不匹配")
    if request["write"] != (expected_effect != "read-only"):
        raise CloneProtocolError("clone 私有协议 write 与副作用分类不匹配")
    backend = _backend(request["backend_dir"])
    _validate_paths(backend, command, decoded)
    return backend, SimpleNamespace(
        command=command, write=request["write"], native_tool=native_tool, **decoded)
