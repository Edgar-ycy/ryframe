#!/usr/bin/env python3
"""使用 tree-sitter Rust AST 校验函数长度。"""

from __future__ import annotations

import datetime as dt
import importlib.metadata
import json
import os
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


SHA_PATTERN = re.compile(r"[0-9a-fA-F]{40}")
HUNK_PATTERN = re.compile(r"^@@ -\d+(?:,\d+)? \+(?P<start>\d+)(?:,(?P<count>\d+))? @@")
PARSER_DISTRIBUTIONS = {
    "tree-sitter": "0.25.2",
    "tree-sitter-rust": "0.24.2",
}
NATIVE_PROBE_ROUNDS = 64


@dataclass(frozen=True)
class FunctionRecord:
    path: str
    symbol: str
    start_line: int
    end_line: int
    lines: int
    private: bool


@dataclass(frozen=True)
class UnsafeSyntaxRecord:
    path: str
    line: int
    context: str


@dataclass(frozen=True)
class FunctionRule:
    path: str
    symbol: str


@dataclass(frozen=True)
class FunctionException(FunctionRule):
    current_lines: int
    reason: str
    expires: dt.date


@dataclass(frozen=True)
class FunctionSizePolicy:
    mode: str
    public_max_lines: int
    private_max_lines: int
    orchestration_max_lines: int
    max_exception_days: int
    orchestrations: frozenset[FunctionRule]
    exceptions: tuple[FunctionException, ...]


@dataclass(frozen=True)
class RustAstRuntime:
    """共同持有原生 Language 与 Parser，避免两者生命周期被拆开。"""

    language: Any
    parser: Any

    def parse_source_records(
        self, path: str, source: bytes, errors: list[str]
    ) -> tuple[list[FunctionRecord], list[UnsafeSyntaxRecord]]:
        tree = self.parser.parse(source)
        if tree.root_node.has_error:
            errors.append(f"tree-sitter 无法完整解析 Rust 源码：{path}")
            return [], []
        # Tree 必须覆盖完整的 Node 遍历周期；不要只把临时 root_node 传给调用方。
        return (
            functions_in_tree(path, source, tree.root_node),
            unsafe_syntax_in_tree(path, tree.root_node),
        )

    def parse_records(
        self, path: str, source: bytes, errors: list[str]
    ) -> list[FunctionRecord]:
        functions, _ = self.parse_source_records(path, source, errors)
        return functions

    def parse_template_unsafe(
        self, path: str, source: bytes
    ) -> list[UnsafeSyntaxRecord]:
        """模板允许占位符导致恢复性语法错误，但仍按 Rust token 识别 unsafe。"""

        tree = self.parser.parse(source)
        return unsafe_syntax_in_tree(path, tree.root_node)


def parse_policy(raw: Any, errors: list[str]) -> FunctionSizePolicy | None:
    if not isinstance(raw, dict):
        errors.append("架构策略必须声明 function_size")
        return None
    mode = raw.get("mode")
    if mode not in {"changed", "all"}:
        errors.append("function_size.mode 只允许 changed 或 all")
        mode = "changed"
    limits: dict[str, int] = {}
    for key in (
        "public_max_lines",
        "private_max_lines",
        "orchestration_max_lines",
        "max_exception_days",
    ):
        value = raw.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value < 1:
            errors.append(f"function_size.{key} 必须是正整数")
            value = 1
        limits[key] = value
    if not (
        limits["orchestration_max_lines"]
        <= limits["private_max_lines"]
        <= limits["public_max_lines"]
    ):
        errors.append("函数长度上限必须满足 orchestration <= private <= public")

    orchestrations = parse_rules(raw.get("orchestrations", []), "orchestrations", errors)
    exceptions = parse_exceptions(
        raw.get("exceptions", []), limits["max_exception_days"], errors
    )
    duplicate_exceptions = duplicate_keys(exceptions)
    if duplicate_exceptions:
        errors.append(
            "function_size.exceptions 不得重复："
            + ", ".join(f"{path}:{symbol}" for path, symbol in duplicate_exceptions)
        )
    if mode == "all" and exceptions:
        errors.append("function_size.mode 只能在 exceptions 清零后切换为 all")
    return FunctionSizePolicy(
        mode=mode,
        public_max_lines=limits["public_max_lines"],
        private_max_lines=limits["private_max_lines"],
        orchestration_max_lines=limits["orchestration_max_lines"],
        max_exception_days=limits["max_exception_days"],
        orchestrations=frozenset(orchestrations),
        exceptions=tuple(exceptions),
    )


def parse_rules(raw: Any, label: str, errors: list[str]) -> list[FunctionRule]:
    if not isinstance(raw, list):
        errors.append(f"function_size.{label} 必须是 table 数组")
        return []
    rules: list[FunctionRule] = []
    for index, entry in enumerate(raw):
        item = parse_rule(entry, f"function_size.{label}[{index}]", errors)
        if item is not None:
            rules.append(item)
    duplicates = duplicate_keys(rules)
    if duplicates:
        errors.append(
            f"function_size.{label} 不得重复："
            + ", ".join(f"{path}:{symbol}" for path, symbol in duplicates)
        )
    return rules


def parse_rule(raw: Any, label: str, errors: list[str]) -> FunctionRule | None:
    if not isinstance(raw, dict):
        errors.append(f"{label} 必须是 TOML table")
        return None
    path = raw.get("path")
    symbol = raw.get("symbol")
    if not isinstance(path, str) or not valid_relative_rust_path(path):
        errors.append(f"{label}.path 必须是规范 Rust 相对路径")
        return None
    if not isinstance(symbol, str) or not symbol.strip():
        errors.append(f"{label}.symbol 必须是非空字符串")
        return None
    return FunctionRule(path=path, symbol=symbol)


def parse_exceptions(
    raw: Any, max_exception_days: int, errors: list[str]
) -> list[FunctionException]:
    if not isinstance(raw, list):
        errors.append("function_size.exceptions 必须是 table 数组")
        return []
    today = dt.date.today()
    result: list[FunctionException] = []
    for index, entry in enumerate(raw):
        label = f"function_size.exceptions[{index}]"
        rule = parse_rule(entry, label, errors)
        if rule is None or not isinstance(entry, dict):
            continue
        current_lines = entry.get("current_lines")
        reason = entry.get("reason")
        expires = entry.get("expires")
        if not isinstance(current_lines, int) or isinstance(current_lines, bool) or current_lines < 1:
            errors.append(f"{label}.current_lines 必须是正整数")
            continue
        if not isinstance(reason, str) or not reason.strip():
            errors.append(f"{label}.reason 必须是非空字符串")
            continue
        if not isinstance(expires, dt.date) or isinstance(expires, dt.datetime):
            errors.append(f"{label}.expires 必须是 TOML 日期")
            continue
        if expires < today:
            errors.append(f"{label} 已过期：{expires.isoformat()}")
        if expires > today + dt.timedelta(days=max_exception_days):
            errors.append(
                f"{label}.expires 不得超过 {max_exception_days} 天：{expires.isoformat()}"
            )
        result.append(
            FunctionException(
                path=rule.path,
                symbol=rule.symbol,
                current_lines=current_lines,
                reason=reason,
                expires=expires,
            )
        )
    return result


def duplicate_keys(rules: list[FunctionRule]) -> list[tuple[str, str]]:
    seen: set[tuple[str, str]] = set()
    duplicates: set[tuple[str, str]] = set()
    for rule in rules:
        key = (rule.path, rule.symbol)
        if key in seen:
            duplicates.add(key)
        seen.add(key)
    return sorted(duplicates)


def valid_relative_rust_path(value: str) -> bool:
    path = Path(value)
    return (
        value == value.replace("\\", "/")
        and value.endswith(".rs")
        and not path.is_absolute()
        and ".." not in path.parts
    )


def load_ast_runtime(errors: list[str]) -> RustAstRuntime | None:
    if not validate_parser_versions(errors):
        return None
    try:
        from tree_sitter import Language, Parser
        import tree_sitter_rust
    except ImportError as error:
        errors.append(
            "Rust 函数长度门禁缺少固定 tree-sitter 依赖；"
            f"请安装 scripts/requirements-ci.txt：{error}"
        )
        return None
    language = Language(tree_sitter_rust.language())
    parser = Parser(language)
    return RustAstRuntime(language=language, parser=parser)


def parse_rust_sources(
    root: Path, paths: list[Path], errors: list[str]
) -> tuple[list[FunctionRecord], list[UnsafeSyntaxRecord]]:
    runtime = load_ast_runtime(errors)
    if runtime is None:
        return [], []
    functions: list[FunctionRecord] = []
    unsafe_syntax: list[UnsafeSyntaxRecord] = []
    for path in paths:
        source = path.read_bytes()
        relative = path.relative_to(root).as_posix()
        source_functions, source_unsafe = runtime.parse_source_records(
            relative, source, errors
        )
        functions.extend(source_functions)
        unsafe_syntax.extend(source_unsafe)
    return functions, unsafe_syntax


def parse_functions(
    root: Path, paths: list[Path], errors: list[str]
) -> list[FunctionRecord]:
    functions, _ = parse_rust_sources(root, paths, errors)
    return functions


def parse_template_unsafe(
    root: Path, paths: list[Path], errors: list[str]
) -> list[UnsafeSyntaxRecord]:
    runtime = load_ast_runtime(errors)
    if runtime is None:
        return []
    records: list[UnsafeSyntaxRecord] = []
    for path in paths:
        relative = path.relative_to(root).as_posix()
        records.extend(runtime.parse_template_unsafe(relative, path.read_bytes()))
    return records


def validate_parser_versions(errors: list[str]) -> bool:
    valid = True
    for distribution, expected in PARSER_DISTRIBUTIONS.items():
        try:
            actual = importlib.metadata.version(distribution)
        except importlib.metadata.PackageNotFoundError:
            errors.append(f"Rust AST 门禁缺少固定依赖：{distribution}=={expected}")
            valid = False
            continue
        if actual != expected:
            errors.append(
                f"Rust AST 门禁拒绝未验证的原生依赖：{distribution} "
                f"必须为 {expected}，实际为 {actual}"
            )
            valid = False
    return valid


def functions_in_tree(path: str, source: bytes, root_node: Any) -> list[FunctionRecord]:
    result: list[FunctionRecord] = []
    stack: list[tuple[Any, tuple[str, ...]]] = [(root_node, ())]
    while stack:
        node, scopes = stack.pop()
        child_scopes = scopes
        if node.type in {"mod_item", "trait_item"}:
            scope_name = node.child_by_field_name("name")
            if scope_name is not None:
                child_scopes = (*scopes, source_text(scope_name, source))
        elif node.type == "impl_item":
            implemented_type = node.child_by_field_name("type")
            implemented_trait = node.child_by_field_name("trait")
            if implemented_type is not None:
                label = source_text(implemented_type, source)
                if implemented_trait is not None:
                    label = f"{source_text(implemented_trait, source)} for {label}"
                child_scopes = (*scopes, label)
        elif node.type == "function_item" and node.child_by_field_name("body") is not None:
            name = node.child_by_field_name("name")
            if name is not None:
                function_name = source_text(name, source)
                start_line = node.start_point.row + 1
                end_line = node.end_point.row + 1
                result.append(
                    FunctionRecord(
                        path=path,
                        symbol="::".join((*scopes, function_name)),
                        start_line=start_line,
                        end_line=end_line,
                        lines=end_line - start_line + 1,
                        private=not any(
                            child.type == "visibility_modifier"
                            for child in node.named_children
                        ),
                    )
                )
                child_scopes = (*scopes, function_name)
        stack.extend((child, child_scopes) for child in reversed(node.named_children))
    return result


def unsafe_syntax_in_tree(path: str, root_node: Any) -> list[UnsafeSyntaxRecord]:
    """只匹配语法树中的 unsafe 关键字，不误报注释、字符串或原始标识符。"""

    result: list[UnsafeSyntaxRecord] = []
    stack = [root_node]
    while stack:
        node = stack.pop()
        if node.type == "unsafe":
            parent = node.parent
            result.append(
                UnsafeSyntaxRecord(
                    path=path,
                    line=node.start_point.row + 1,
                    context=parent.type if parent is not None else "unknown",
                )
            )
        stack.extend(reversed(node.children))
    return result


def validate_no_unsafe(records: list[UnsafeSyntaxRecord], errors: list[str]) -> int:
    for record in records:
        errors.append(
            "自维护 Rust 源码禁止 unsafe 语法："
            f"{record.path}:{record.line}（{record.context}）"
        )
    return len(records)


def source_text(node: Any, source: bytes) -> str:
    return source[node.start_byte : node.end_byte].decode("utf-8")


def changed_line_ranges(root: Path, errors: list[str]) -> dict[str, list[tuple[int, int]]] | None:
    configured_base = os.environ.get("RYFRAME_CI_BASE_SHA") or os.environ.get("GITHUB_BASE_SHA")
    if configured_base is not None and not SHA_PATTERN.fullmatch(configured_base):
        errors.append("函数长度 changed 模式收到无效 base SHA，已回退检查全部函数")
        return None
    base = configured_base or "HEAD"
    completed = subprocess.run(
        ["git", "diff", "--unified=0", "--no-ext-diff", base, "--", "*.rs"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if completed.returncode != 0:
        errors.append("函数长度 changed 模式无法读取 Git diff，已回退检查全部函数")
        return None
    changed = parse_changed_ranges(completed.stdout)
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "-z", "--", "*.rs"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if untracked.returncode != 0:
        errors.append("函数长度 changed 模式无法读取未跟踪 Rust 文件，已回退检查全部函数")
        return None
    for path in parse_untracked_rust_paths(untracked.stdout):
        changed[path] = [(1, 2**31 - 1)]
    return changed


def parse_untracked_rust_paths(output: str) -> list[str]:
    return sorted(
        {
            path.replace("\\", "/")
            for path in output.split("\0")
            if path and path.replace("\\", "/").endswith(".rs")
        }
    )


def parse_changed_ranges(diff: str) -> dict[str, list[tuple[int, int]]]:
    result: dict[str, list[tuple[int, int]]] = {}
    current: str | None = None
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            current = line.removeprefix("+++ b/")
            result.setdefault(current, [])
            continue
        match = HUNK_PATTERN.match(line)
        if current is None or match is None:
            continue
        start = int(match.group("start"))
        count = int(match.group("count") or "1")
        if count > 0:
            result[current].append((start, start + count - 1))
    return result


def validate_functions(
    policy: FunctionSizePolicy,
    records: list[FunctionRecord],
    changed: dict[str, list[tuple[int, int]]] | None,
    errors: list[str],
) -> tuple[int, int]:
    by_key = {(record.path, record.symbol): record for record in records}
    orchestration_keys = {(rule.path, rule.symbol) for rule in policy.orchestrations}
    exception_by_key = {
        (exception.path, exception.symbol): exception for exception in policy.exceptions
    }
    for path, symbol in sorted(orchestration_keys - set(by_key)):
        errors.append(f"编排函数登记已失效：{path}:{symbol}")
    for path, symbol in sorted(set(exception_by_key) - set(by_key)):
        errors.append(f"函数长度例外已失效：{path}:{symbol}")

    checked = 0
    violations = 0
    for record in records:
        key = (record.path, record.symbol)
        exception = exception_by_key.get(key)
        if exception is not None:
            if record.lines > exception.current_lines:
                errors.append(
                    f"函数长度例外只能缩短：{record.path}:{record.start_line} "
                    f"{record.symbol}（登记 {exception.current_lines}，实际 {record.lines}）"
                )
                violations += 1
            continue
        if policy.mode == "changed" and changed is not None and not function_changed(record, changed):
            continue
        checked += 1
        if key in orchestration_keys:
            limit = policy.orchestration_max_lines
            category = "编排函数"
        elif record.private:
            limit = policy.private_max_lines
            category = "private helper"
        else:
            limit = policy.public_max_lines
            category = "普通函数"
        if record.lines > limit:
            errors.append(
                f"{category}超过 {limit} 行：{record.path}:{record.start_line} "
                f"{record.symbol}（{record.lines} 行）"
            )
            violations += 1
    return checked, violations


def function_changed(
    record: FunctionRecord, changed: dict[str, list[tuple[int, int]]]
) -> bool:
    return any(
        start <= record.end_line and end >= record.start_line
        for start, end in changed.get(record.path, [])
    )


def ast_json_request() -> int:
    """仅供 Rust 架构门禁调用的 AST 入口，按相对路径返回语法记录。"""

    try:
        request = json.loads(sys.stdin.buffer.read().decode("utf-8"))
        root = Path(request["root"]).resolve()
        sources = request["sources"]
        templates = request["templates"]
        if (
            not isinstance(sources, list)
            or not isinstance(templates, list)
            or not all(isinstance(item, str) for item in sources + templates)
        ):
            raise ValueError("sources 和 templates 必须是字符串数组")
    except (KeyError, TypeError, ValueError, OSError, json.JSONDecodeError) as error:
        print(f"Rust AST helper 请求无效：{error}", file=sys.stderr)
        return 2

    def resolve(relative: str, error_label: str, errors: list[str]) -> Path | None:
        path = (root / relative).resolve()
        try:
            path.relative_to(root)
        except ValueError:
            errors.append(f"{error_label}路径越出仓库：{relative}")
            return None
        if not path.is_file():
            errors.append(f"{error_label}源码不存在：{relative}")
            return None
        return path

    errors: list[str] = []
    source_paths = [
        path
        for relative in sources
        if (path := resolve(relative, "Rust ", errors)) is not None
    ]
    template_paths = [
        path
        for relative in templates
        if (path := resolve(relative, "Rust 模板", errors)) is not None
    ]
    functions, unsafe_syntax = parse_rust_sources(root, source_paths, errors)
    unsafe_syntax.extend(parse_template_unsafe(root, template_paths, errors))
    payload = {
        "functions": [asdict(record) for record in functions],
        "unsafe_syntax": [asdict(record) for record in unsafe_syntax],
        "errors": errors,
    }
    sys.stdout.buffer.write(
        json.dumps(payload, ensure_ascii=False).encode("utf-8")
    )
    return 0


def native_probe() -> int:
    """在隔离子进程中重复验证原生 AST runtime，阻止 CI 对话框阻塞。"""

    if sys.platform == "win32":
        import ctypes

        # 访问冲突等原生崩溃应转换为退出码；禁止弹出等待人工关闭的系统错误对话框。
        ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002)
    errors: list[str] = []
    if sys.version_info < (3, 11):
        errors.append(
            f"Python 版本过低：至少需要 3.11，实际为 "
            f"{sys.version_info.major}.{sys.version_info.minor}"
        )
    runtime = load_ast_runtime(errors)
    if runtime is not None:
        source = b"pub fn probe() { let value = 1; }\n"
        for iteration in range(NATIVE_PROBE_ROUNDS):
            records = runtime.parse_records("native-probe.rs", source, errors)
            if len(records) != 1 or records[0].symbol != "probe":
                errors.append(f"Rust AST 原生探针第 {iteration + 1} 轮返回异常结果")
                break
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print(f"Rust AST native probe passed (rounds={NATIVE_PROBE_ROUNDS})")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--ast-json"]:
        return ast_json_request()
    if sys.argv[1:] == ["--native-probe"]:
        return native_probe()
    print(
        "用法：python tools/python/rust_function_size.py <--ast-json|--native-probe>",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
