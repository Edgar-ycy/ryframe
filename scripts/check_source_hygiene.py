#!/usr/bin/env python3
"""Reject corrupted or accidentally collapsed text sources."""

from __future__ import annotations

import os
import re
import tomllib
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
    (
        "crates/ryframe-core/tests/refresh_session_redis_test.rs",
        "redis_refresh_rotation_cas_semantics",
    ),
    (
        "crates/ryframe-core/tests/refresh_session_redis_test.rs",
        "redis_refresh_rotation_recovers_after_transient_response_loss",
    ),
    (
        "crates/ryframe-api/tests/integration_test.rs",
        "force_logout_uses_authoritative_family_and_recovers_after_redis_failure",
    ),
    (
        "crates/ryframe-api/tests/integration_test.rs",
        "auth_middleware_fails_closed_when_redis_is_unavailable",
    ),
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
VENDORED_SOURCE_PREFIX = "vendor/"
CURRENT_DOC_NAMES = {"README.md", "CONTRIBUTING.md"}
LEGACY_DATABASE_DOC_PATTERN = re.compile(r"\b(?:PostgreSQL|SQLite)\b", re.IGNORECASE)
LEGACY_DATABASE_DRIVER_PATTERN = re.compile(r"^\s*driver\s*=", re.MULTILINE)
REMOVED_RELOAD_PATTERN = re.compile(
    r"\b(?:HotConfig|reload_hot|config_watcher)\b|配置热更新|hot[- ]reload",
    re.IGNORECASE,
)
REMOVED_HEALTH_PATTERN = re.compile(r"(?:/api/v1/monitor)?/health\b")
DATABASE_DRIVER_ENV_PATTERN = re.compile(r"\bAPP_DATABASE_DRIVER\b")
REMOVED_SQLX_PACKAGES = {"libsqlite3-sys", "sqlx-postgres", "sqlx-sqlite"}
HEALTH_CONTRACT_PREFIXES = (
    "crates/ryframe/src/",
    "crates/ryframe-api/src/",
    "crates/ryframe-monitor/src/",
    "openapi/",
)


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
        is_first_party = not relative.startswith(VENDORED_SOURCE_PREFIX)
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
        if (
            is_first_party
            and path.suffix == ".rs"
            and LEGACY_API_ALIAS_PATTERN.search(text)
        ):
            errors.append(f"{relative}: contains a legacy pagination alias")
        if (
            is_first_party
            and path.suffix == ".rs"
            and LINT_ALLOW_PATTERN.search(text)
        ):
            errors.append(f"{relative}: suppresses a compiler or Clippy lint with allow")
        if (
            is_first_party
            and path.suffix == ".rs"
            and IGNORED_DOCTEST_PATTERN.search(text)
        ):
            errors.append(f"{relative}: contains an ignored Rust documentation test")
        if is_first_party and relative not in LEGACY_API_TERM_ALLOWLIST:
            for term in LEGACY_API_TERMS:
                if term in text:
                    errors.append(f"{relative}: contains legacy API term {term}")
        if (
            path.suffix == ".rs"
            and relative.startswith("crates/")
            and "/src/" in relative
        ):
            for route in LEGACY_ACTION_PATHS:
                if route in text:
                    errors.append(f"{relative}: contains legacy action path {route}")
        if is_first_party and "tests" in path.parts:
            for test_name in IGNORED_TEST_PATTERN.findall(text):
                if (relative, test_name) not in ALLOWED_IGNORED_TESTS:
                    errors.append(f"{relative}: ignored test is not allowlisted ({test_name})")

        is_current_doc = relative in CURRENT_DOC_NAMES or relative.startswith("docs/")
        if is_current_doc and LEGACY_DATABASE_DOC_PATTERN.search(text):
            errors.append(f"{relative}: current documentation is not MySQL-only")
        if (
            (is_current_doc or relative.startswith("config/"))
            and LEGACY_DATABASE_DRIVER_PATTERN.search(text)
        ):
            errors.append(f"{relative}: contains removed database driver configuration")
        if (
            relative != "scripts/check_source_hygiene.py"
            and (is_current_doc or relative.startswith("config/") or "/src/" in relative)
            and REMOVED_RELOAD_PATTERN.search(text)
        ):
            errors.append(f"{relative}: contains removed runtime configuration reload API")
        if (
            relative != "scripts/check_source_hygiene.py"
            and (is_current_doc or relative.startswith("config/") or "/src/" in relative)
            and DATABASE_DRIVER_ENV_PATTERN.search(text)
        ):
            errors.append(f"{relative}: contains removed database driver environment variable")
        if (
            (is_current_doc or relative.startswith(HEALTH_CONTRACT_PREFIXES))
            and REMOVED_HEALTH_PATTERN.search(text)
        ):
            errors.append(f"{relative}: contains removed /health contract")
        if is_first_party and path.name == "Cargo.toml":
            for feature in ("sqlx-postgres", "sqlx-sqlite"):
                if feature in text:
                    errors.append(f"{relative}: contains removed Cargo feature {feature}")

    lock_path = ROOT / "Cargo.lock"
    try:
        lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
        locked_packages = {package["name"] for package in lock.get("package", [])}
        removed = sorted(locked_packages & REMOVED_SQLX_PACKAGES)
        if removed:
            errors.append(
                "Cargo.lock: contains removed database driver packages "
                + ", ".join(removed)
            )
    except (OSError, tomllib.TOMLDecodeError, KeyError) as error:
        errors.append(f"Cargo.lock: cannot validate dependency hygiene ({error})")

    if errors:
        print("Source hygiene check failed:")
        for error in errors:
            print(f"  - {error}")
        return 1

    print(f"Source hygiene check passed ({checked} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
