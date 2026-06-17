#!/usr/bin/env python3
"""Fail CI when a protected API route is missing permission binding.

The check is intentionally simple and conservative:
- It scans handler source files for axum `.route(...)` declarations.
- Routes in explicitly public handler files are ignored.
- For other routes, the route snippet must contain `perm_route(...)`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HANDLERS = ROOT / "crates" / "ryframe-api" / "src" / "handlers"

PUBLIC_FILES = {
    "auth_handler.rs",
    "captcha_handler.rs",
    "common_handler.rs",
    "profile_handler.rs",
}

PUBLIC_PATHS = {
    "/user-tree",
}


def extract_route_calls(text: str) -> list[str]:
    calls: list[str] = []
    idx = 0
    while True:
        start = text.find(".route(", idx)
        if start == -1:
            break
        depth = 0
        i = start
        while i < len(text):
            ch = text[i]
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0:
                    calls.append(text[start : i + 1])
                    idx = i + 1
                    break
            i += 1
        else:
            break
    return calls


def first_string_literal(snippet: str) -> str | None:
    match = re.search(r'"([^"]+)"', snippet)
    return match.group(1) if match else None


def main() -> int:
    violations: list[str] = []

    for path in sorted(HANDLERS.glob("*.rs")):
        if path.name in PUBLIC_FILES:
            continue

        text = path.read_text(encoding="utf-8")
        for call in extract_route_calls(text):
            route_path = first_string_literal(call)
            if route_path in PUBLIC_PATHS:
                continue
            if "perm_route(" not in call:
                violations.append(f"{path.relative_to(ROOT)} :: {route_path or '<unknown>'}")

    if violations:
        print("Missing permission binding in protected routes:")
        for item in violations:
            print(f"  - {item}")
        print()
        print("Wrap the route with `perm_route(...)` or add it to the explicit public allowlist if it is intended to be public.")
        return 1

    print("Permission route check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
