#!/usr/bin/env python3
"""Reject corrupted or accidentally collapsed text sources."""

from __future__ import annotations

import os
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXCLUDED_DIRS = {".git", ".pnpm-store", "target", "ryframe-vue3"}
TEXT_SUFFIXES = {
    ".json",
    ".md",
    ".ps1",
    ".py",
    ".rs",
    ".sh",
    ".sql",
    ".toml",
    ".yaml",
    ".yml",
}
TEXT_NAMES = {".editorconfig", ".gitattributes", ".gitignore"}
MOJIBAKE_MARKERS = ("\ufffd", "\u951b", "\u9286", "\u922b")
ALLOWED_IGNORED_TESTS = {
    ("crates/ryframe-storage/tests/object_storage_test.rs", "test_s3_integration_put_get_delete"),
    ("crates/ryframe-db/tests/named_datasource_mysql_test.rs", "mysql_named_source_is_distinct_and_explicit"),
}
IGNORED_TEST_PATTERN = re.compile(
    r'#\[ignore(?:\s*=\s*"[^"]*")?\]\s*'
    r'(?:#\[[^\]]+\]\s*)*'
    r'(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)'
)
LEGACY_API_ALIAS_PATTERN = re.compile(
    r'\balias\s*=\s*"(?:pageNum|pageSize|size)"'
)
LINT_ALLOW_PATTERN = re.compile(r'#\s*!?\[\s*allow\s*\(')
IGNORED_DOCTEST_PATTERN = re.compile(
    r"^\s*//[!/]\s*```\s*(?:rust\s*,\s*)?ignore\b", re.MULTILINE
)
LEGACY_API_TERMS = ("pageSize", "pageNum", "searchValue", "requestId")
LEGACY_ACTION_PATHS = (
    "assign-perm",
    "assign-dept",
    "update-data-scope",
    "assign-role",
)
LEGACY_API_TERM_ALLOWLIST = {
    "crates/ryframe-core/src/repository.rs",
    "scripts/check_source_hygiene.py",
}


def source_files() -> list[Path]:
    files: list[Path] = []
    for directory, directories, names in os.walk(ROOT):
        directories[:] = [name for name in directories if name not in EXCLUDED_DIRS]
        base = Path(directory)
        files.extend(
            path
            for name in names
            if (path := base / name).suffix.lower() in TEXT_SUFFIXES or name in TEXT_NAMES
        )
    return sorted(files)


def main() -> int:
    errors: list[str] = []
    checked = 0

    if (ROOT / ".pnpm-store").exists():
        errors.append(
            ".pnpm-store: frontend pnpm commands must run from ryframe-vue3"
        )

    for path in source_files():
        relative = path.relative_to(ROOT).as_posix()
        data = path.read_bytes()
        checked += 1

        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError as error:
            errors.append(f"{relative}: invalid UTF-8 ({error})")
            continue

        if "\0" in text:
            errors.append(f"{relative}: contains a NUL byte")
        if any(marker in text for marker in MOJIBAKE_MARKERS):
            errors.append(f"{relative}: contains replacement or mojibake characters")
        if any("\ue000" <= character <= "\uf8ff" for character in text):
            errors.append(f"{relative}: contains a Unicode private-use character")
        if len(data) > 1_000 and text.count("\n") < 2:
            errors.append(f"{relative}: suspiciously collapsed into fewer than three lines")
        if path.suffix == ".rs" and LEGACY_API_ALIAS_PATTERN.search(text):
            errors.append(f"{relative}: contains a legacy pagination alias")
        if path.suffix == ".rs" and LINT_ALLOW_PATTERN.search(text):
            errors.append(f"{relative}: suppresses a compiler or Clippy lint with allow")
        if path.suffix == ".rs" and IGNORED_DOCTEST_PATTERN.search(text):
            errors.append(f"{relative}: contains an ignored Rust documentation test")
        if relative not in LEGACY_API_TERM_ALLOWLIST:
            for term in LEGACY_API_TERMS:
                if term in text:
                    errors.append(f"{relative}: contains legacy API term {term}")
        if path.suffix == ".rs" and relative.startswith("crates/") and "/src/" in relative:
            for route in LEGACY_ACTION_PATHS:
                if route in text:
                    errors.append(f"{relative}: contains legacy action path {route}")
        if "tests" in path.parts:
            for test_name in IGNORED_TEST_PATTERN.findall(text):
                if (relative, test_name) not in ALLOWED_IGNORED_TESTS:
                    errors.append(f"{relative}: ignored test is not allowlisted ({test_name})")

    if errors:
        print("Source hygiene check failed:")
        for error in errors:
            print(f"  - {error}")
        return 1

    print(f"Source hygiene check passed ({checked} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
