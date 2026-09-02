#!/usr/bin/env python3
"""校验访问目录编译门禁与已移除的在线生成器边界。"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


DEFAULT_ROOT = Path(__file__).resolve().parents[1]

FORBIDDEN_ONLINE_GENERATOR_PATTERNS = {
    "在线生成器 handler": re.compile(r"\bgenerator_handler\b"),
    "在线生成器 API 路径": re.compile(r"/api/v1/tools/gen(?:/|[\"'])"),
    "在线生成器路由段": re.compile(r"[\"']/gen[\"']"),
    "在线生成器权限": re.compile(r"\btools:gen(?::[a-z]+)?\b"),
    "在线生成器菜单路由": re.compile(r"\btools\.gen\b"),
}

def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="校验访问目录与路由 policy")
    parser.add_argument("--backend-root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--skip-cargo-check", action="store_true")
    arguments = parser.parse_args()
    root = arguments.backend_root.resolve()
    if not root.joinpath("Cargo.toml").is_file() or not root.joinpath("crates").is_dir():
        parser.error(f"后端根目录无效：{root}")
    product_source_roots = [
        path / "src"
        for path in sorted(root.joinpath("crates").iterdir())
        if path.is_dir()
        and path.name != "ryframe-generator"
        and path.joinpath("src").is_dir()
    ]
    violations: list[str] = []

    for source_root in product_source_roots:
        for path in sorted(source_root.rglob("*.rs")):
            text = path.read_text(encoding="utf-8")
            for label, pattern in FORBIDDEN_ONLINE_GENERATOR_PATTERNS.items():
                if pattern.search(text):
                    violations.append(f"{path.relative_to(root)} :: {label}")

    if violations:
        print("访问边界违规：")
        for item in violations:
            print(f"  - {item}")
        return 1

    # 路由及 utoipa 属性通过 syn AST 解析；build.rs 同时校验 TOML 唯一性、
    # 引用闭合和全部编译路由的显式访问策略，避免用正则猜测 HTTP 方法。
    if not arguments.skip_cargo_check:
        result = subprocess.run(
            ["cargo", "check", "--locked", "-p", "ryframe-api"],
            cwd=root,
            check=False,
        )
        if result.returncode != 0:
            return result.returncode

    print("访问目录与路由 policy 检查通过。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
