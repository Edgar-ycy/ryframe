"""备份、恢复与运行收据共享的标识符契约。"""

from __future__ import annotations

import re

IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}")
SCOPE_IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9_-]{0,46}[a-z0-9]")


def valid_identifier(value: object) -> bool:
    return isinstance(value, str) and bool(IDENTIFIER.fullmatch(value))


def valid_scope_identifier(value: object) -> bool:
    return isinstance(value, str) and bool(SCOPE_IDENTIFIER.fullmatch(value))
