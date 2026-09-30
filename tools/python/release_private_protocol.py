"""Rust xtask 与发布核验 Python 实现之间的私有协议。"""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from typing import Mapping, Sequence


PREFIX = "RYFRAME_RELEASE_"
VERSION_KEY = f"{PREFIX}PROTOCOL_VERSION"
MODE_KEY = f"{PREFIX}MODE"
PROTOCOL_VERSION = "1"

SOURCE_MODE = "source"
EVIDENCE_MODE = "ci-evidence"
RECORD_PAIR_MODE = "ci-record-pair"
VERIFY_PAIR_MODE = "ci-verify-pair"


@dataclass(frozen=True)
class ProtocolSchema:
    required: frozenset[str]
    optional: frozenset[str] = frozenset()


@dataclass(frozen=True)
class PrivateProtocol:
    mode: str
    values: Mapping[str, str]

    def value(self, name: str) -> str:
        return self.values[f"{PREFIX}{name}"]

    def optional(self, name: str) -> str | None:
        return self.values.get(f"{PREFIX}{name}")


SCHEMAS = {
    SOURCE_MODE: ProtocolSchema(
        frozenset(
            {
                "TAG",
                "FRONTEND_DIR",
                "BACKEND_REPOSITORY",
                "BACKEND_COMMIT",
                "FRONTEND_REPOSITORY",
                "FRONTEND_COMMIT",
                "MANIFEST_PATH",
            }
        )
    ),
    EVIDENCE_MODE: ProtocolSchema(
        frozenset(
            {
                "BACKEND_REPOSITORY",
                "FRONTEND_REPOSITORY",
                "BACKEND_SHA",
                "FRONTEND_SHA",
                "TAG",
                "TIMEOUT_SECONDS",
                "OUTPUT_PATH",
            }
        ),
        frozenset({"BACKEND_TAG_OID", "FRONTEND_TAG_OID"}),
    ),
    RECORD_PAIR_MODE: ProtocolSchema(
        frozenset({"BACKEND_DIR", "FRONTEND_DIR", "OUTPUT_PATH"})
    ),
    VERIFY_PAIR_MODE: ProtocolSchema(
        frozenset({"BACKEND_DIR", "FRONTEND_DIR", "INPUT_PATH"})
    ),
}


class ProtocolError(ValueError):
    """私有协议不完整、含歧义或不是由统一入口生成。"""


def load_protocol(
    expected_modes: set[str] | frozenset[str],
    *,
    arguments: Sequence[str] | None = None,
    environment: Mapping[str, str] | None = None,
) -> PrivateProtocol:
    """读取严格环境协议；发布脚本不再接受公开命令行选项。"""
    actual_arguments = sys.argv[1:] if arguments is None else arguments
    if actual_arguments:
        raise ProtocolError("发布核验脚本是私有实现，不接受命令行参数")

    source = os.environ if environment is None else environment
    values = {key: value for key, value in source.items() if key.startswith(PREFIX)}
    if values.get(VERSION_KEY) != PROTOCOL_VERSION:
        raise ProtocolError(f"{VERSION_KEY} 必须是 {PROTOCOL_VERSION}")

    mode = values.get(MODE_KEY)
    if mode not in expected_modes or mode not in SCHEMAS:
        raise ProtocolError(f"未知或不适用的发布私有协议模式：{mode!r}")

    schema = SCHEMAS[mode]
    required = {f"{PREFIX}{name}" for name in schema.required}
    optional = {f"{PREFIX}{name}" for name in schema.optional}
    allowed = required | optional | {VERSION_KEY, MODE_KEY}
    unknown = sorted(set(values) - allowed)
    if unknown:
        raise ProtocolError(f"发布私有协议含未知字段：{', '.join(unknown)}")
    missing = sorted(key for key in required if not values.get(key))
    if missing:
        raise ProtocolError(f"发布私有协议缺少字段：{', '.join(missing)}")
    empty_optional = sorted(key for key in optional if key in values and not values[key])
    if empty_optional:
        raise ProtocolError(f"发布私有协议字段不能为空：{', '.join(empty_optional)}")
    invalid_text = sorted(
        key
        for key, value in values.items()
        if "\n" in value or "\r" in value or "\0" in value
    )
    if invalid_text:
        raise ProtocolError(
            f"发布私有协议字段不能包含换行符或 NUL：{', '.join(invalid_text)}"
        )

    return PrivateProtocol(mode=mode, values=values)
