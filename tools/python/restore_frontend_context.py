"""origin authority 仍使用的通用 JSON helper；前端上下文规则由 Rust 核验。"""
from __future__ import annotations

import re

IDENTITY = ".local-tests/build/source.json"


def exact(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{label}字段无效；请使用当前完整字段，删除未知字段")
    return value


def hex_value(value: object, length: int) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{%d}" % length, value) is not None


def validate_frontend_build_context(value: object) -> dict:
    from restore_frontend_native import request

    return request("context", value=value)
