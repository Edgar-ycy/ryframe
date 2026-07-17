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

# These files are mounted either publicly or behind authentication-only router
# policies and intentionally do not use per-route RBAC permission codes.
NON_RBAC_FILES = {
    "auth_handler.rs",
    "captcha_handler.rs",
    "common_handler.rs",
    "profile_handler.rs",
}

AUTHENTICATED_ONLY_ROUTES = {("menu_handler.rs", "/current")}

ROUTE_ATTR = re.compile(
    r'^\s*#\[(get|post|put|delete)\(([^\]]+)\)\]'
    r'(?:\s*\n\s*#\[perm\("([^"]+)"\)\])?',
    re.MULTILINE,
)


def main() -> int:
    violations: list[str] = []

    protected_files = [
        path
        for path in sorted(HANDLERS.rglob("*.rs"))
        if path.relative_to(HANDLERS).parts[0] not in NON_RBAC_FILES
    ] + EXTRA_PROTECTED_FILES
    for path in protected_files:
        text = path.read_text(encoding="utf-8")
        for match in ROUTE_ATTR.finditer(text):
            route_paths = re.findall(r'"([^"]+)"', match.group(2))
            if any(
                (path.name, route_path) in AUTHENTICATED_ONLY_ROUTES
                for route_path in route_paths
            ):
                continue
            if match.group(3) is None:
                route_label = ", ".join(route_paths) or "<unknown>"
                violations.append(f"{path.relative_to(ROOT)} :: {route_label}")

    if violations:
        print("Missing permission binding in protected routes:")
        for item in violations:
            print(f"  - {item}")
        print()
        print(
            "Add `#[perm(\"permission:code\")]` below the route attribute, "
            "or explicitly allowlist an authentication-only path."
        )
        return 1

    print("Permission route check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
