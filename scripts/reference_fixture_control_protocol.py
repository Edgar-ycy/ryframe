"""参考夹具控制脚本的版本化私有 JSON 协议。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys
from typing import Callable


PROTOCOL_KEY = "RYFRAME_REFERENCE_FIXTURE_CONTROL_PROTOCOL"
PROTOCOL_PREFIX = "RYFRAME_REFERENCE_FIXTURE_CONTROL_"
PROTOCOL_KIND = "ryframe-reference-fixture-control"
BASE_FIELDS = {"backend_dir", "domain", "format_version", "kind", "operation", "write"}
PATH_FIELDS = {
    "backend_dir", "frontend_dir", "output_dir", "review", "fixture", "maintenance_build", "output", "secrets_dir",
    "source_fixture", "source_secrets", "template", "future_root", "environment",
    "service_run", "source_result", "predecessor_review", "predecessor_request",
    "successor_review", "seed_request", "base_request", "candidate_request", "successor",
    "source_backend", "backend_build", "source_environment", "product_backend",
    "source_export_result", "workspace", "copy_directory", "owner_binding",
}
INTEGER_FIELDS = {
    "api_port", "worker_port", "frontend_port", "rustfs_api_port", "rustfs_console_port",
    "redis_port",
}


class FixtureControlProtocolError(ValueError):
    """私有请求不完整、歧义或来自错误入口。"""


def _object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise FixtureControlProtocolError(f"私有协议字段重复：{key}")
        result[key] = value
    return result


def _text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value or any(mark in value for mark in ("\n", "\r", "\0")):
        raise FixtureControlProtocolError(f"{label}必须是无换行或 NUL 的非空字符串")
    return value


def _path(value: object, label: str) -> str:
    text = _text(value, label)
    if not Path(text).is_absolute() or any(part in (".", "..") for part in Path(text).parts):
        raise FixtureControlProtocolError(f"{label}必须是无跳转的绝对路径")
    return text


def _write_matches(policy: bool | str, protocol: dict) -> bool:
    expected = policy if isinstance(policy, bool) else policy in protocol
    return type(protocol.get("write")) is bool and protocol["write"] is expected


def private_arguments(
    domain: str,
    schemas: dict[str, tuple[tuple[str, ...], tuple[str, ...], bool | str]],
    *,
    positional_operation: bool,
    argv: list[str] | None = None,
    environment: dict[str, str] | None = None,
) -> list[str]:
    """只从单个私有环境字段重建经过精确约束的内部 argv。"""
    arguments = sys.argv[1:] if argv is None else argv
    if arguments:
        raise FixtureControlProtocolError("参考夹具控制脚本是私有实现，不接受命令行参数")
    values = os.environ if environment is None else environment
    unknown = sorted(
        name for name in values if name.startswith(PROTOCOL_PREFIX) and name != PROTOCOL_KEY
    )
    if unknown:
        raise FixtureControlProtocolError("参考夹具控制私有环境含未知字段")
    raw = values.get(PROTOCOL_KEY)
    if not isinstance(raw, str) or not raw or len(raw) > 65536:
        raise FixtureControlProtocolError("参考夹具控制私有协议缺失、为空或过长")
    if any(mark in raw for mark in ("\n", "\r", "\0")):
        raise FixtureControlProtocolError("参考夹具控制私有协议不能包含换行符或 NUL")
    try:
        protocol = json.loads(raw, object_pairs_hook=_object)
    except FixtureControlProtocolError:
        raise
    except (TypeError, ValueError) as error:
        raise FixtureControlProtocolError("参考夹具控制私有协议不是有效 JSON") from error
    if not isinstance(protocol, dict):
        raise FixtureControlProtocolError("参考夹具控制私有协议必须是对象")
    if (
        type(protocol.get("format_version")) is not int
        or protocol["format_version"] != 1
        or protocol.get("kind") != PROTOCOL_KIND
        or protocol.get("domain") != domain
    ):
        raise FixtureControlProtocolError("参考夹具控制私有协议版本、类型或子域无效")
    operation = protocol.get("operation")
    if operation not in schemas:
        raise FixtureControlProtocolError("参考夹具控制私有协议操作无效")
    required, optional, write_policy = schemas[operation]
    expected = BASE_FIELDS | set(required)
    actual = set(protocol)
    if not expected <= actual or not actual <= expected | set(optional):
        raise FixtureControlProtocolError("参考夹具控制私有协议字段不完整或含未知字段")
    if not _write_matches(write_policy, protocol):
        raise FixtureControlProtocolError("参考夹具控制私有协议写入授权与操作不符")
    result = [operation] if positional_operation else []
    result.extend(("--backend-dir", _path(protocol["backend_dir"], "后端目录")))
    for name in (*required, *optional):
        if name not in protocol:
            continue
        value = protocol[name]
        if name in PATH_FIELDS:
            value = _path(value, name)
        elif name in INTEGER_FIELDS:
            if type(value) is not int:
                raise FixtureControlProtocolError(f"{name} 必须是整数")
            value = str(value)
        else:
            value = _text(value, name)
        result.extend(("--" + name.replace("_", "-"), value))
    if protocol["write"]:
        result.append("--write")
    return result


def run_private(
    domain: str,
    schemas: dict[str, tuple[tuple[str, ...], tuple[str, ...], bool | str]],
    main: Callable[[list[str]], None],
    *,
    positional_operation: bool,
) -> int:
    """运行固定实现，并保持参数错误 2、任务错误 1 与中断语义。"""
    try:
        arguments = private_arguments(
            domain, schemas, positional_operation=positional_operation
        )
    except FixtureControlProtocolError:
        print("reference_fixture_control_protocol_error：私有控制请求无效。", file=sys.stderr)
        return 2
    inherited_protocol = os.environ.pop(PROTOCOL_KEY, None)
    try:
        try:
            main(arguments)
        except Exception:
            print(
                f"reference_fixture_{domain}_failed：夹具控制阶段失败，请核对原账本和失败证据；不自动重放。",
                file=sys.stderr,
            )
            return 1
    finally:
        if inherited_protocol is not None:
            os.environ[PROTOCOL_KEY] = inherited_protocol
    return 0
