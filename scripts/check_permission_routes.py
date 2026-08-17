#!/usr/bin/env python3
"""当属性路由缺少权限标注时使 CI 失败。"""

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

# 这些文件要么公开挂载，要么位于仅认证路由策略之后，故意不使用逐路由 RBAC 权限码。
NON_RBAC_FILES = {
    "auth_handler.rs",
    "captcha_handler.rs",
    "common_handler.rs",
    "profile_handler.rs",
    # 个人服务委托只允许当前 JWT 主体管理本人委托；Agent 路由使用独立 API Key 授权与审计链。
    "service_delegation_profile_handler.rs",
    "agent_handler.rs",
}

CAPABILITY_REQUIRED_FILES = {
    "service_account_handler.rs": "system.service_accounts",
    "service_delegation_profile_handler.rs": "system.service_accounts",
    "agent_handler.rs": "system.service_accounts",
}

AUTHENTICATED_ONLY_ROUTES = {
    ("menu_handler.rs", "/current"),
    # 消息收件箱是当前认证用户的自有资源，不依赖管理端 RBAC 权限码。
    ("message_handler.rs", "/"),
    ("message_handler.rs", "/unread-count"),
    ("message_handler.rs", "/ack"),
    ("message_handler.rs", "/delete"),
    ("message_handler.rs", "/{id}/read"),
    ("message_handler.rs", "/read-all"),
    # 导出任务仅允许创建者操作自身任务，服务层会复核租户、申请人和资源权限，
    # 因此不要求额外的管理端 RBAC 权限码。
    ("export_handler.rs", "/"),
    ("export_handler.rs", "/notifications/unread-count"),
    ("export_handler.rs", "/notifications/read"),
    ("export_handler.rs", "/{id}"),
    ("export_handler.rs", "/{id}/cancel"),
    ("export_handler.rs", "/{id}/download"),
}

ROUTE_ATTR = re.compile(
    r'^\s*#\[(get|post|put|delete)\(([^\]]+)\)\]'
    r'(?:\s*\n\s*#\[capability\("([^"]+)"\)\])?'
    r'(?:\s*\n\s*#\[perm\("([^"]+)"\)\])?',
    re.MULTILINE,
)


def routes_are_authenticated_only(filename: str, route_paths: list[str]) -> bool:
    return bool(route_paths) and all(
        (filename, route_path) in AUTHENTICATED_ONLY_ROUTES
        for route_path in route_paths
    )


def main() -> int:
    violations: list[str] = []

    protected_files = [
        path
        for path in sorted(HANDLERS.rglob("*.rs"))
        if path.relative_to(HANDLERS).parts[0] not in NON_RBAC_FILES
    ] + EXTRA_PROTECTED_FILES
    for filename in CAPABILITY_REQUIRED_FILES:
        path = HANDLERS / filename
        if path not in protected_files:
            protected_files.append(path)
    for path in protected_files:
        text = path.read_text(encoding="utf-8")
        for match in ROUTE_ATTR.finditer(text):
            route_paths = re.findall(r'"([^"]+)"', match.group(2))
            expected_capability = CAPABILITY_REQUIRED_FILES.get(path.name)
            if expected_capability and match.group(3) != expected_capability:
                route_label = ", ".join(route_paths) or "<unknown>"
                violations.append(
                    f"{path.relative_to(ROOT)} :: {route_label} "
                    f"(missing #[capability(\"{expected_capability}\")])"
                )
                continue
            if path.name in NON_RBAC_FILES:
                continue
            if routes_are_authenticated_only(path.name, route_paths):
                continue
            if match.group(4) is None:
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
