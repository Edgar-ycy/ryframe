#!/usr/bin/env python3
"""Fail CI when an attribute route is missing a permission annotation."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HANDLERS = ROOT / "crates" / "ryframe-api" / "src" / "handlers"
EXTRA_PROTECTED_FILES = [
    ROOT / "crates" / "ryframe-api" / "src" / "router.rs",
    ROOT / "crates" / "ryframe-monitor" / "src" / "lib.rs",
]

PUBLIC_FILES = {
    "auth_handler.rs",
    "captcha_handler.rs",
    "common_handler.rs",
    "profile_handler.rs",
}

PUBLIC_PATHS = {"/get-menus"}

ROUTE_ATTR = re.compile(
    r'^\s*#\[(get|post|put|delete)\(([^\]]+)\)\]'
    r'(?:\s*\n\s*#\[perm\("([^"]+)"\)\])?',
    re.MULTILINE,
)


def main() -> int:
    violations: list[str] = []

    protected_files = [
        path for path in sorted(HANDLERS.glob("*.rs")) if path.name not in PUBLIC_FILES
    ] + EXTRA_PROTECTED_FILES
    for path in protected_files:
        text = path.read_text(encoding="utf-8")
        for match in ROUTE_ATTR.finditer(text):
            route_paths = re.findall(r'"([^"]+)"', match.group(2))
            if any(route_path in PUBLIC_PATHS for route_path in route_paths):
                continue
            if match.group(3) is None:
                route_label = ", ".join(route_paths) or "<unknown>"
                violations.append(f"{path.relative_to(ROOT)} :: {route_label}")

    if violations:
        print("Missing permission binding in protected routes:")
        for item in violations:
            print(f"  - {item}")
        print()
        print("Add `#[perm(\"permission:code\")]` below the route attribute, or explicitly allowlist a public path.")
        return 1

    print("Permission route check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
