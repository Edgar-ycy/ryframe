#!/usr/bin/env python3
"""为后端消费契约检查选择前端提交。"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


MARKER_PATTERN = re.compile(
    r"^[ \t]*Frontend-Commit:[ \t]*([0-9a-fA-F]{40})[ \t]*$",
    re.MULTILINE,
)
MARKER_MENTION_PATTERN = re.compile(r"Frontend-Commit", re.IGNORECASE)


def select_frontend_ref(body: str, contract_changed: bool) -> str:
    """契约未变化时使用 main，变化时要求唯一且严格的完整提交 SHA。"""

    if not contract_changed:
        return "main"

    matches = MARKER_PATTERN.findall(body)
    mentions = MARKER_MENTION_PATTERN.findall(body)
    if len(matches) != 1 or len(mentions) != 1:
        raise ValueError(
            "OpenAPI 已变化；PR 正文必须且只能包含一行 "
            "Frontend-Commit: <40 位 SHA>"
        )
    return matches[0].lower()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", type=Path, required=True)
    parser.add_argument(
        "--contract-changed",
        choices=("true", "false"),
        required=True,
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    event = json.loads(args.event.read_text(encoding="utf-8"))
    pull_request = event.get("pull_request")
    if not isinstance(pull_request, dict):
        raise ValueError("消费契约选择器只能处理 pull_request 事件")
    body = pull_request.get("body")
    if body is None:
        body = ""
    if not isinstance(body, str):
        raise ValueError("pull_request.body 必须是字符串或 null")
    print(select_frontend_ref(body, args.contract_changed == "true"))


if __name__ == "__main__":
    main()
