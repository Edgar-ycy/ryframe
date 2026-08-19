#!/usr/bin/env python3
"""检查后端手写业务源码的文件规模。"""

from __future__ import annotations

from pathlib import Path


MAX_LINES = 1000
ROOT = Path(__file__).resolve().parents[1]
TARGETS = (
    ROOT / "crates" / "ryframe-application" / "src",
    ROOT / "crates" / "ryframe-adapters" / "src",
)
EXCLUDED_PARTS = {"generated", "schema", "migration", "migrations", "snapshots"}


def count_lines(path: Path) -> int:
    content = path.read_text(encoding="utf-8")
    if not content:
        return 0
    return len(content.rstrip("\r\n").splitlines())


def is_handwritten_business_source(path: Path) -> bool:
    relative_parts = path.relative_to(ROOT).parts
    if any(part in EXCLUDED_PARTS for part in relative_parts):
        return False
    return path.name != "schema.rs"


def main() -> int:
    violations: list[tuple[int, Path]] = []
    scanned = 0
    for target in TARGETS:
        for path in sorted(target.rglob("*.rs")):
            if not is_handwritten_business_source(path):
                continue
            scanned += 1
            lines = count_lines(path)
            if lines > MAX_LINES:
                violations.append((lines, path.relative_to(ROOT)))

    if violations:
        print(f"源码规模检查失败：手写 Rust 文件不得超过 {MAX_LINES} 行。")
        for lines, path in sorted(violations, reverse=True):
            print(f"  {lines:>4} 行  {path.as_posix()}")
        return 1

    print(f"源码规模检查通过（扫描 {scanned} 个手写 Rust 文件，单文件上限 {MAX_LINES} 行）。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
