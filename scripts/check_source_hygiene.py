#!/usr/bin/env python3
"""拒绝损坏或意外折叠的文本源码。"""

from __future__ import annotations

import os
import re
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXCLUDED_DIRS = {".git", ".pnpm-store", "target", "ryframe-vue3"}
TEXT_SUFFIXES = {
    ".cjs",
    ".conf",
    ".js",
    ".json",
    ".md",
    ".mjs",
    ".ps1",
    ".py",
    ".rs",
    ".sh",
    ".sql",
    ".toml",
    ".yaml",
    ".yml",
}
TEXT_NAMES = {".editorconfig", ".gitattributes", ".gitignore", "Dockerfile"}
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
    # 该模块的测试必须保留历史字段名，以验证公开接口会明确拒绝旧写法。
    "crates/ryframe-api/src/macros.rs",
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
GOVERNANCE_TEXT_PREFIXES = (".github/", "docs/", "scripts/")
WORKFLOW_PREFIX = ".github/workflows/"
COMMENT_LANGUAGE_SUFFIXES = {
    ".cjs",
    ".conf",
    ".dockerfile",
    ".js",
    ".mjs",
    ".ps1",
    ".py",
    ".rs",
    ".sh",
    ".sql",
    ".toml",
    ".yaml",
    ".yml",
}
COMMENT_LANGUAGE_EXCLUDED_PREFIXES: tuple[str, ...] = ()
HAN_CHARACTER_PATTERN = re.compile(r"[\u4e00-\u9fff]")
COMMENT_DIRECTIVE_PATTERN = re.compile(
    r"^(?:!|/?\s*<reference\b|(?:cargo|clippy|fmt|noqa|pragma|rustfmt|shellcheck|type)"
    r"(?:[-:\s]|$)|(?:syntax|escape)=|SPDX-License-Identifier:|Copyright\b)",
    re.IGNORECASE,
)
COMMENT_SEPARATOR_PATTERN = re.compile(r"^[\s|=*#_\-─—]+$")
TECHNICAL_COMMENT_PATTERN = re.compile(
    r"^(?:`[^`]+`(?:\s*\([A-Z0-9-]+\))?|"
    r"(?:GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)\s+\S+|"
    r"`?/[A-Za-z0-9_./{}?=&:-]+`?[。.]?)$"
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


def comment_language_suffix(path: Path) -> str:
    if path.name == "Dockerfile":
        return ".dockerfile"
    return path.suffix.lower()


def collect_comments(text: str, suffix: str) -> list[tuple[int, str, bool]]:
    """以轻量词法扫描提取注释，避免把字符串中的注释标记误判为注释。"""
    comments: list[tuple[int, str, bool]] = []
    index = 0
    line_number = 1
    length = len(text)
    supports_slash_comments = suffix in {".cjs", ".js", ".mjs", ".rs"}
    supports_hash_comments = suffix in {
        ".conf",
        ".dockerfile",
        ".ps1",
        ".py",
        ".sh",
        ".toml",
        ".yaml",
        ".yml",
    }
    supports_sql_comments = suffix == ".sql"
    supports_single_quote = suffix in {
        ".cjs",
        ".conf",
        ".js",
        ".mjs",
        ".ps1",
        ".py",
        ".sh",
        ".toml",
        ".yaml",
        ".yml",
    }
    supports_backtick = suffix in {".cjs", ".js", ".mjs"}

    def skip_quoted(start: int, quote: str) -> int:
        cursor = start + len(quote)
        while cursor < length:
            if text.startswith(quote, cursor):
                return cursor + len(quote)
            if text[cursor] == "\\":
                cursor += 2
            else:
                cursor += 1
        return cursor

    while index < length:
        current = text[index]
        next_character = text[index + 1] if index + 1 < length else ""

        if (
            suffix == ".rs"
            and current == "r"
            and (index == 0 or not (text[index - 1].isalnum() or text[index - 1] == "_"))
        ):
            raw_start = re.match(r'r(#+)?"', text[index:])
            if raw_start:
                hashes = raw_start.group(1) or ""
                closing = '"' + hashes
                end = text.find(closing, index + len(raw_start.group(0)))
                if end == -1:
                    line_number += text[index:].count("\n")
                    break
                end += len(closing)
                line_number += text[index:end].count("\n")
                index = end
                continue

        if suffix == ".py" and text.startswith(current * 3, index) and current in {"'", '"'}:
            end = skip_quoted(index, current * 3)
            line_number += text[index:end].count("\n")
            index = end
            continue
        if suffix == ".rs" and current == "'":
            # Rust 生命周期不带闭合引号；仅跳过可确认的字符字面量，避免把 b'"'
            # 中的双引号误当成字符串起点。
            character_end = index + (3 if index + 1 < length and text[index + 1] == "\\" else 2)
            if character_end < length and text[character_end] == "'":
                index = character_end + 1
                continue
        if current == '"' or (current == "'" and supports_single_quote) or (
            current == "`" and supports_backtick
        ):
            end = skip_quoted(index, current)
            line_number += text[index:end].count("\n")
            index = end
            continue

        if supports_slash_comments and current == "/" and next_character == "/":
            end = text.find("\n", index + 2)
            if end == -1:
                end = length
            is_doc = text.startswith("///", index) or text.startswith("//!", index)
            comments.append(
                (line_number, text[index + (3 if is_doc else 2) : end], is_doc)
            )
            index = end
            continue
        if supports_slash_comments and current == "/" and next_character == "*":
            end = text.find("*/", index + 2)
            content_end = length if end == -1 else end
            comments.append(
                (
                    line_number,
                    text[index + 2 : content_end],
                    text.startswith("/**", index) or text.startswith("/*!", index),
                )
            )
            index = length if end == -1 else end + 2
            continue
        if supports_hash_comments and current == "#" and (
            index == 0 or text[index - 1].isspace()
        ):
            end = text.find("\n", index + 1)
            if end == -1:
                end = length
            comments.append((line_number, text[index + 1 : end], False))
            index = end
            continue
        if supports_sql_comments and current == "-" and next_character == "-":
            end = text.find("\n", index + 2)
            if end == -1:
                end = length
            comments.append((line_number, text[index + 2 : end], False))
            index = end
            continue

        if current == "\n":
            line_number += 1
        index += 1

    return comments


def comment_language_violations(relative: str, text: str, suffix: str) -> list[str]:
    """返回不含中文说明的项目自有源码注释。"""
    if relative.startswith(COMMENT_LANGUAGE_EXCLUDED_PREFIXES):
        return []

    violations: list[str] = []
    in_doc_code_block = False
    previous_doc_line: int | None = None

    for line_number, comment, is_doc in collect_comments(text, suffix):
        if not is_doc or previous_doc_line is None or line_number > previous_doc_line + 1:
            in_doc_code_block = False
        for offset, raw_line in enumerate(comment.splitlines() or [comment]):
            raw_comment = raw_line.strip().lstrip("*").strip()
            if not raw_comment:
                continue

            if is_doc and raw_comment.startswith("```"):
                in_doc_code_block = not in_doc_code_block
                continue
            if in_doc_code_block:
                continue
            if COMMENT_SEPARATOR_PATTERN.fullmatch(raw_comment):
                continue
            if COMMENT_DIRECTIVE_PATTERN.match(raw_comment):
                continue
            technical_comment = raw_comment.removeprefix("- ").strip("= ")
            if TECHNICAL_COMMENT_PATTERN.fullmatch(technical_comment):
                continue
            if not HAN_CHARACTER_PATTERN.search(raw_comment):
                violations.append(
                    f"{relative}:{line_number + offset}: explanatory comment must contain Chinese text"
                )
        previous_doc_line = line_number if is_doc else None

    return violations


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

        is_governance_text = relative.startswith(GOVERNANCE_TEXT_PREFIXES)
        if is_governance_text and data.startswith(b"\xef\xbb\xbf"):
            errors.append(f"{relative}: must use UTF-8 without a BOM")
        if relative.startswith(WORKFLOW_PREFIX) and b"\r" in data:
            errors.append(f"{relative}: workflow files must use LF line endings")
        if data and not data.endswith(b"\n"):
            errors.append(f"{relative}: text file must end with a newline")
        if "\0" in text:
            errors.append(f"{relative}: contains a NUL byte")
        if any(marker in text for marker in MOJIBAKE_MARKERS):
            errors.append(f"{relative}: contains replacement or mojibake characters")
        if any("\ue000" <= character <= "\uf8ff" for character in text):
            errors.append(f"{relative}: contains a Unicode private-use character")
        if len(data) > 1_000 and text.count("\n") < 2:
            errors.append(f"{relative}: suspiciously collapsed into fewer than three lines")
        suffix = comment_language_suffix(path)
        if is_first_party and suffix in COMMENT_LANGUAGE_SUFFIXES:
            errors.extend(comment_language_violations(relative, text, suffix))
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
