#!/usr/bin/env python3
"""校验工作区 crate 图、源码规模与租户数据静态边界。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable
from datetime import date
from pathlib import Path
from typing import Any

# 该脚本既会被直接执行，也会通过 importlib 从单元测试加载。后一种方式不会
# 自动把 scripts/ 放入模块搜索路径，因此显式固定同目录模块的解析位置。
SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from tenant_data_boundaries import validate_tenant_data_boundaries  # noqa: E402

from rust_function_size import (  # noqa: E402
    FunctionRecord,
    changed_line_ranges,
    parse_policy as parse_function_size_policy,
    parse_rust_sources,
    parse_template_unsafe,
    validate_functions,
    validate_no_unsafe,
)


ROOT = Path(__file__).resolve().parents[1]
POLICY_PATH = ROOT / "architecture" / "crate-boundaries.toml"
LEGACY_PERSISTENCE_API_NAMES = (
    "Persistence" + "Future",
    "Control" + "Transaction",
)
TRAIT_DECLARATION = re.compile(
    r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b[^{}]*\{"
)
TRAIT_FUTURE_METHOD = re.compile(
    r"\b(?P<async>async\s+)?fn\s+(?P<method>[A-Za-z_][A-Za-z0-9_]*)\b"
    r"[^;{}]*?->\s*[^;{}]*(?:\b[A-Za-z_][A-Za-z0-9_]*Future\b|"
    r"\bimpl\s+Future\b|\bPin\s*<)[^;{}]*;",
    re.DOTALL,
)
DOCUMENT_LIMITS = {
    "README.md": 120,
    "docs/api.md": 180,
    "docs/architecture.md": 200,
    "docs/data.md": 200,
    "docs/development.md": 160,
    "docs/operations.md": 240,
}
HISTORICAL_DOCUMENTS = {"CHANGELOG.md"}
DOCUMENTATION_EXCLUDED_DIRECTORIES = frozenset(
    {
        ".cache",
        ".git",
        ".github",
        ".local-tests",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        "__pycache__",
        "node_modules",
        "target",
        "vendor",
    }
)
TEST_FILE_NAME = re.compile(r"(?:^tests?\.rs$|_tests?\.rs$)", re.IGNORECASE)
TEST_ATTRIBUTE = re.compile(
    r"#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|(?:[A-Za-z_][A-Za-z0-9_]*::)?test)\s*\]"
)
SYSTEM_DOMAIN_MODULES = {"content", "identity", "operations", "platform"}


def read(relative: str) -> str:
    return (ROOT / relative).read_text(encoding="utf-8")


def validate_system_domain_surface(root: Path, errors: list[str]) -> None:
    """确保 system 仅通过四个业务域暴露公开 API。"""
    module_path = root / "crates/ryframe-application/src/system/mod.rs"
    if not module_path.is_file():
        errors.append("缺少 ryframe-application system 模块入口")
        return
    source = module_path.read_text(encoding="utf-8")
    public_modules = set(
        re.findall(r"(?m)^\s*pub\s+mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;", source)
    )
    if public_modules != SYSTEM_DOMAIN_MODULES:
        errors.append(
            "system 公开模块必须且只能为四个业务域: "
            f"expected={sorted(SYSTEM_DOMAIN_MODULES)}, actual={sorted(public_modules)}"
        )
    if re.search(r"(?m)^\s*pub\s+use\s+", source):
        errors.append("system 根模块不得保留兼容 re-export")


def documentation_paths(root: Path, errors: list[str]) -> set[str]:
    """列出项目人工文档，并在进入工具缓存前剪枝。"""

    actual: set[str] = set()

    def record_walk_error(error: OSError) -> None:
        errors.append(f"无法枚举文档目录: {error}")

    for directory, directory_names, file_names in os.walk(
        root,
        topdown=True,
        onerror=record_walk_error,
    ):
        directory_names[:] = [
            name
            for name in directory_names
            if name.casefold() not in DOCUMENTATION_EXCLUDED_DIRECTORIES
        ]
        directory_path = Path(directory)
        actual.update(
            (directory_path / name).relative_to(root).as_posix()
            for name in file_names
            if name.casefold().endswith(".md")
        )
    return actual


def validate_documentation(root: Path, errors: list[str]) -> None:
    actual = documentation_paths(root, errors)
    expected = set(DOCUMENT_LIMITS) | HISTORICAL_DOCUMENTS
    if actual != expected:
        missing = expected - actual
        unexpected = actual - expected
        if missing:
            errors.append(f"后端缺少约定文档: {', '.join(sorted(missing))}")
        if unexpected:
            errors.append(f"后端存在额外人工文档: {', '.join(sorted(unexpected))}")
    for relative, limit in DOCUMENT_LIMITS.items():
        path = root / relative
        if not path.is_file():
            continue
        lines = len(path.read_text(encoding="utf-8").splitlines())
        if lines > limit:
            errors.append(f"{relative} 共 {lines} 行，超过 {limit} 行上限")


def parse_edge(value: str, label: str, errors: list[str]) -> tuple[str, str] | None:
    parts = [part.strip() for part in value.split("->")]
    if len(parts) != 2 or not all(parts):
        errors.append(f"{label} 包含无效依赖边: {value!r}")
        return None
    return parts[0], parts[1]


def parse_string_set(
    value: Any,
    label: str,
    errors: list[str],
) -> set[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        errors.append(f"{label} 必须是字符串数组")
        return set()
    result = set(value)
    if len(result) != len(value):
        errors.append(f"{label} 不得包含重复项")
    return result


def parse_edges(value: Any, label: str, errors: list[str]) -> set[tuple[str, str]]:
    raw_edges = parse_string_set(value, label, errors)
    result: set[tuple[str, str]] = set()
    for raw_edge in raw_edges:
        edge = parse_edge(raw_edge, label, errors)
        if edge is not None:
            result.add(edge)
    return result


def parse_temporary_edges(
    value: Any,
    label: str,
    errors: list[str],
) -> set[tuple[str, str]]:
    if not isinstance(value, list):
        errors.append(f"{label} 必须是 table 数组")
        return set()
    result: set[tuple[str, str]] = set()
    for index, item in enumerate(value):
        item_label = f"{label}[{index}]"
        if not isinstance(item, dict):
            errors.append(f"{item_label} 必须是 table")
            continue
        unknown = set(item) - {"edge", "reason", "expires"}
        missing = {"edge", "reason", "expires"} - set(item)
        if unknown:
            errors.append(f"{item_label} 包含未知字段: {', '.join(sorted(unknown))}")
        if missing:
            errors.append(f"{item_label} 缺少字段: {', '.join(sorted(missing))}")
            continue
        edge_value = item["edge"]
        reason = item["reason"]
        expiry = item["expires"]
        if not isinstance(edge_value, str):
            errors.append(f"{item_label}.edge 必须是字符串")
            continue
        edge = parse_edge(edge_value, f"{item_label}.edge", errors)
        if not isinstance(reason, str) or not reason.strip():
            errors.append(f"{item_label}.reason 必须是非空字符串")
        try:
            if type(expiry) is date:
                expiry_date = expiry
            elif isinstance(expiry, str):
                expiry_date = date.fromisoformat(expiry)
            else:
                raise TypeError
        except (TypeError, ValueError):
            errors.append(f"{item_label}.expires 必须是 YYYY-MM-DD 日期")
            expiry_date = None
        if expiry_date is not None and expiry_date < date.today():
            errors.append(f"{item_label} 已于 {expiry_date.isoformat()} 过期")
        if edge is not None and edge in result:
            errors.append(f"{label} 不得包含重复边: {edge_value}")
        elif edge is not None:
            result.add(edge)
    return result


def strongly_connected_components(
    nodes: Iterable[str],
    edges: Iterable[tuple[str, str]],
) -> list[list[str]]:
    graph = {node: [] for node in nodes}
    for source, target in edges:
        graph.setdefault(source, []).append(target)
        graph.setdefault(target, [])

    index = 0
    indices: dict[str, int] = {}
    low_links: dict[str, int] = {}
    stack: list[str] = []
    on_stack: set[str] = set()
    components: list[list[str]] = []

    def visit(node: str) -> None:
        nonlocal index
        indices[node] = index
        low_links[node] = index
        index += 1
        stack.append(node)
        on_stack.add(node)

        for target in graph[node]:
            if target not in indices:
                visit(target)
                low_links[node] = min(low_links[node], low_links[target])
            elif target in on_stack:
                low_links[node] = min(low_links[node], indices[target])

        if low_links[node] != indices[node]:
            return
        component: list[str] = []
        while stack:
            member = stack.pop()
            on_stack.remove(member)
            component.append(member)
            if member == node:
                break
        components.append(sorted(component))

    for node in sorted(graph):
        if node not in indices:
            visit(node)
    return components


def cyclic_components(
    nodes: set[str],
    edges: set[tuple[str, str]],
) -> list[list[str]]:
    self_loops = {source for source, target in edges if source == target}
    return [
        component
        for component in strongly_connected_components(nodes, edges)
        if len(component) > 1 or component[0] in self_loops
    ]


def validate_profile(name: str, profile: Any, errors: list[str]) -> dict[str, Any]:
    label = f"profiles.{name}"
    if not isinstance(profile, dict):
        errors.append(f"{label} 必须是 TOML table")
        return {}

    packages = parse_string_set(profile.get("packages"), f"{label}.packages", errors)
    products = parse_string_set(
        profile.get("product_packages"), f"{label}.product_packages", errors
    )
    tools = parse_string_set(
        profile.get("tool_packages"), f"{label}.tool_packages", errors
    )
    allowed_edges = parse_edges(
        profile.get("allowed_internal_edges"),
        f"{label}.allowed_internal_edges",
        errors,
    )
    temporary_edges = parse_temporary_edges(
        profile.get("temporary_product_tool_edges", []),
        f"{label}.temporary_product_tool_edges",
        errors,
    )
    expected_count = profile.get("expected_package_count")
    if not isinstance(expected_count, int) or expected_count < 1:
        errors.append(f"{label}.expected_package_count 必须是正整数")
    elif expected_count != len(packages):
        errors.append(
            f"{label} 声明 {expected_count} 个包，但 packages 实际包含 {len(packages)} 个"
        )
    if products & tools:
        errors.append(f"{label} 的产品包与工具包不得重叠")
    if products | tools != packages:
        missing = packages - products - tools
        unknown = (products | tools) - packages
        if missing:
            errors.append(f"{label} 未分类包: {', '.join(sorted(missing))}")
        if unknown:
            errors.append(f"{label} 分类了未声明包: {', '.join(sorted(unknown))}")

    for source, target in sorted(allowed_edges):
        if source not in packages or target not in packages:
            errors.append(f"{label} 的允许边引用未声明包: {source} -> {target}")

    product_tool_edges = {
        edge
        for edge in allowed_edges
        if edge[0] in products and edge[1] in tools
    }
    unknown_exceptions = temporary_edges - product_tool_edges
    if unknown_exceptions:
        errors.append(
            f"{label} 的产品依赖工具豁免不是允许边: "
            + ", ".join(f"{a} -> {b}" for a, b in sorted(unknown_exceptions))
        )
    unapproved_product_tool_edges = product_tool_edges - temporary_edges
    if unapproved_product_tool_edges:
        errors.append(
            f"{label} 存在未豁免的产品 -> 工具边: "
            + ", ".join(
                f"{a} -> {b}" for a, b in sorted(unapproved_product_tool_edges)
            )
        )

    cycles = cyclic_components(packages, allowed_edges)
    for component in cycles:
        errors.append(f"{label} 的允许依赖图存在 SCC: {' -> '.join(component)}")

    return {
        "packages": packages,
        "products": products,
        "tools": tools,
        "allowed_edges": allowed_edges,
        "temporary_edges": temporary_edges,
        "expected_count": expected_count,
    }


def load_policy(
    errors: list[str],
) -> tuple[
    str,
    dict[str, dict[str, Any]],
    dict[str, Any],
    dict[str, Any],
    dict[str, Any],
]:
    try:
        policy = tomllib.loads(POLICY_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"无法读取架构策略 {POLICY_PATH.relative_to(ROOT)}: {error}")
        return "", {}, {}, {}, {}

    if policy.get("schema_version") != 2:
        errors.append("架构策略 schema_version 必须为 2")
    active_profile = policy.get("active_profile")
    if not isinstance(active_profile, str) or not active_profile:
        errors.append("架构策略 active_profile 必须是非空字符串")
        active_profile = ""

    raw_profiles = policy.get("profiles")
    profiles: dict[str, dict[str, Any]] = {}
    if not isinstance(raw_profiles, dict) or not raw_profiles:
        errors.append("架构策略必须声明至少一个 profiles.*")
    else:
        for name, profile in raw_profiles.items():
            profiles[name] = validate_profile(name, profile, errors)
    if active_profile not in profiles:
        errors.append(f"active_profile 未声明: {active_profile!r}")

    source_size = policy.get("source_size")
    if not isinstance(source_size, dict):
        errors.append("架构策略必须声明 source_size")
        source_size = {}
    test_layout = policy.get("test_layout")
    if not isinstance(test_layout, dict):
        errors.append("架构策略必须声明 test_layout")
        test_layout = {}
    function_size = policy.get("function_size")
    if not isinstance(function_size, dict):
        errors.append("架构策略必须声明 function_size")
        function_size = {}
    return active_profile, profiles, source_size, test_layout, function_size


def cargo_metadata(errors: list[str]) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
    except OSError as error:
        errors.append(f"无法执行 cargo metadata: {error}")
        return {}
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        errors.append(f"cargo metadata 执行失败: {detail}")
        return {}
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        errors.append(f"cargo metadata 输出不是有效 JSON: {error}")
        return {}


def workspace_packages(metadata: dict[str, Any]) -> dict[str, dict[str, Any]]:
    workspace_members = set(metadata.get("workspace_members", []))
    return {
        package["name"]: package
        for package in metadata.get("packages", [])
        if package.get("id") in workspace_members
    }


def validate_unsafe_lint_policy(
    root: Path, packages: dict[str, dict[str, Any]], errors: list[str]
) -> int:
    """固定 Workspace lint，并确保每个自维护 crate 都继承该策略。"""

    try:
        workspace_manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"无法读取 Workspace unsafe lint：{error}")
        return 0
    workspace_lints = workspace_manifest.get("workspace", {}).get("lints", {})
    rust_lints = workspace_lints.get("rust", {}) if isinstance(workspace_lints, dict) else {}
    if not isinstance(rust_lints, dict) or rust_lints.get("unsafe_code") != "forbid":
        errors.append("Workspace 必须设置 [workspace.lints.rust] unsafe_code = \"forbid\"")

    checked = 0
    for package_name, package in sorted(packages.items()):
        manifest_path = Path(package["manifest_path"]).resolve()
        try:
            manifest_path.relative_to(root)
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        except ValueError:
            errors.append(f"工作区 crate 位于仓库之外：{package_name}")
            continue
        except (OSError, tomllib.TOMLDecodeError) as error:
            errors.append(f"无法读取 crate lint 配置：{package_name}（{error}）")
            continue
        checked += 1
        lints = manifest.get("lints")
        if not isinstance(lints, dict) or lints.get("workspace") is not True:
            errors.append(f"工作区 crate 必须继承 Workspace lint：{package_name}")
    return checked


def workspace_edges(packages: dict[str, dict[str, Any]]) -> set[tuple[str, str]]:
    package_roots = {
        Path(package["manifest_path"]).resolve().parent: package_name
        for package_name, package in packages.items()
    }
    return {
        (package_name, package_roots[Path(dependency["path"]).resolve()])
        for package_name, package in packages.items()
        for dependency in package.get("dependencies", [])
        if dependency.get("path") is not None
        and Path(dependency["path"]).resolve() in package_roots
    }


def validate_active_workspace(
    active_profile: str,
    profile: dict[str, Any],
    packages: dict[str, dict[str, Any]],
    errors: list[str],
) -> set[tuple[str, str]]:
    actual_packages = set(packages)
    expected_packages = profile.get("packages", set())
    if len(actual_packages) != profile.get("expected_count"):
        errors.append(
            f"工作区包数漂移：profile={active_profile} 期望 "
            f"{profile.get('expected_count')}，实际 {len(actual_packages)}"
        )
    missing = expected_packages - actual_packages
    unexpected = actual_packages - expected_packages
    if missing:
        errors.append(f"工作区缺少 profile 包: {', '.join(sorted(missing))}")
    if unexpected:
        errors.append(f"工作区出现未批准包: {', '.join(sorted(unexpected))}")

    actual_edges = workspace_edges(packages)
    forbidden_edges = actual_edges - profile.get("allowed_edges", set())
    if forbidden_edges:
        errors.append(
            "工作区出现未批准内部依赖边: "
            + ", ".join(f"{a} -> {b}" for a, b in sorted(forbidden_edges))
        )
    stale_declared_edges = profile.get("allowed_edges", set()) - actual_edges
    if stale_declared_edges:
        errors.append(
            "架构事实源包含已不存在的内部依赖边: "
            + ", ".join(f"{a} -> {b}" for a, b in sorted(stale_declared_edges))
        )

    actual_product_tool_edges = {
        edge
        for edge in actual_edges
        if edge[0] in profile.get("products", set())
        and edge[1] in profile.get("tools", set())
    }
    temporary_edges = profile.get("temporary_edges", set())
    unapproved_tool_edges = actual_product_tool_edges - temporary_edges
    if unapproved_tool_edges:
        errors.append(
            "产品 crate 不得依赖工具 crate: "
            + ", ".join(f"{a} -> {b}" for a, b in sorted(unapproved_tool_edges))
        )
    stale_exceptions = temporary_edges - actual_product_tool_edges
    if stale_exceptions:
        errors.append(
            "产品 -> 工具临时豁免已失效，应删除: "
            + ", ".join(f"{a} -> {b}" for a, b in sorted(stale_exceptions))
        )

    for component in cyclic_components(actual_packages, actual_edges):
        errors.append(f"工作区内部依赖存在 SCC: {' -> '.join(component)}")
    return actual_edges


def count_lines(path: Path) -> int:
    content = path.read_text(encoding="utf-8")
    if not content:
        return 0
    return len(content.rstrip("\r\n").splitlines())


def frozen_migration_sources(
    root: Path,
    lock_relative: str,
    errors: list[str],
) -> set[str]:
    """返回锁定清单中路径和摘要均匹配的冻结迁移源码。"""

    lock_path = (root / lock_relative).resolve()
    try:
        lock_path.relative_to(root)
    except ValueError:
        errors.append(f"冻结迁移清单路径越出仓库: {lock_relative}")
        return set()
    try:
        document = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"无法读取冻结迁移清单 {lock_relative}: {error}")
        return set()
    if document.get("format_version") != 1:
        errors.append(f"冻结迁移清单 format_version 必须为 1: {lock_relative}")
    entries = document.get("files")
    if not isinstance(entries, list) or not entries:
        errors.append(f"冻结迁移清单 files 必须是非空 table 数组: {lock_relative}")
        return set()

    frozen: set[str] = set()
    for index, entry in enumerate(entries):
        label = f"冻结迁移清单 files[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} 必须是 TOML table")
            continue
        raw_path = entry.get("path")
        expected_hash = entry.get("sha256")
        if not isinstance(raw_path, str) or not raw_path:
            errors.append(f"{label}.path 必须是非空字符串")
            continue
        if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
            errors.append(f"{label}.sha256 必须是 64 位小写十六进制摘要")
            continue
        source_path = (root / raw_path).resolve()
        try:
            relative = source_path.relative_to(root).as_posix()
        except ValueError:
            errors.append(f"{label}.path 越出仓库: {raw_path}")
            continue
        if not (
            relative.startswith("crates/")
            and "/migration/" in relative
            and relative.endswith(".rs")
        ):
            errors.append(f"{label}.path 必须是 crates 下的迁移 Rust 源码: {relative}")
            continue
        if relative in frozen:
            errors.append(f"冻结迁移清单路径重复: {relative}")
            continue
        if not source_path.is_file():
            errors.append(f"冻结迁移源码不存在: {relative}")
            continue
        actual_hash = hashlib.sha256(source_path.read_bytes()).hexdigest()
        if actual_hash != expected_hash:
            errors.append(
                f"冻结迁移源码哈希不匹配: {relative}（清单 {expected_hash}，实际 {actual_hash}）"
            )
            continue
        frozen.add(relative)
    return frozen


def source_has_generated_marker(path: Path, markers: tuple[str, ...]) -> bool:
    header = "\n".join(path.read_text(encoding="utf-8").splitlines()[:5])
    return any(marker in header for marker in markers)


def source_size_level(lines: int, limit: int) -> str | None:
    if lines >= limit:
        return "fail"
    if lines * 10 >= limit * 9:
        return "strong"
    if lines * 5 >= limit * 4:
        return "hint"
    return None


def validate_source_size(
    source_size: dict[str, Any],
    profile: dict[str, Any],
    packages: dict[str, dict[str, Any]],
    errors: list[str],
    warnings: list[str],
) -> tuple[int, dict[str, int], set[str]]:
    max_lines = source_size.get("max_lines")
    if not isinstance(max_lines, int) or max_lines < 1:
        errors.append("source_size.max_lines 必须是正整数")
        return 0, {}, set()

    generated_max_lines = source_size.get("generated_max_lines")
    if not isinstance(generated_max_lines, int) or generated_max_lines < 1:
        errors.append("source_size.generated_max_lines 必须是正整数")
        return 0, {}, set()
    if generated_max_lines > max_lines:
        errors.append("source_size.generated_max_lines 不得大于 source_size.max_lines")

    raw_generated_markers = source_size.get("generated_markers")
    if not isinstance(raw_generated_markers, list) or not raw_generated_markers:
        errors.append("source_size.generated_markers 必须是非空字符串数组")
        generated_markers: tuple[str, ...] = ()
    else:
        generated_markers = tuple(
            marker
            for marker in raw_generated_markers
            if isinstance(marker, str) and marker.strip()
        )
        if len(generated_markers) != len(raw_generated_markers):
            errors.append("source_size.generated_markers 只能包含非空字符串")

    lock_relative = source_size.get("frozen_migration_lock")
    if not isinstance(lock_relative, str) or not lock_relative:
        errors.append("source_size.frozen_migration_lock 必须是非空字符串")
        frozen_migrations: set[str] = set()
    else:
        frozen_migrations = frozen_migration_sources(ROOT, lock_relative, errors)
    if "legacy_exceptions" in source_size:
        errors.append("source_size 不再支持 legacy_exceptions")

    scanned_paths: set[str] = set()
    scanned_by_package: dict[str, int] = {}
    for package_name in sorted(
        profile.get("products", set()) | profile.get("tools", set())
    ):
        package = packages.get(package_name)
        if package is None:
            continue
        package_root = Path(package["manifest_path"]).resolve().parent
        try:
            package_root.relative_to(ROOT)
        except ValueError:
            errors.append(f"工作区 crate 位于仓库之外: {package_name}（{package_root}）")
            continue
        source_root = package_root / "src"
        candidates = list(source_root.rglob("*.rs")) if source_root.is_dir() else []
        build_script = package_root / "build.rs"
        if build_script.is_file():
            candidates.append(build_script)

        package_count = 0
        for path in sorted(set(candidates)):
            relative = path.relative_to(ROOT).as_posix()
            scanned_paths.add(relative)
            package_count += 1
            lines = count_lines(path)
            limit = (
                generated_max_lines
                if source_has_generated_marker(path, generated_markers)
                else max_lines
            )
            if relative in frozen_migrations:
                continue
            level = source_size_level(lines, limit)
            if level == "fail":
                errors.append(f"源码文件达到 {limit} 行硬上限: {relative}（{lines} 行）")
            elif level == "strong":
                warnings.append(f"强警告：{relative} 已达硬上限 90%（{lines}/{limit} 行）")
            elif level == "hint":
                warnings.append(f"提示：{relative} 已达硬上限 80%（{lines}/{limit} 行）")
        if package_count == 0:
            errors.append(f"工作区 crate 没有纳入任何 Rust 源文件: {package_name}")
        scanned_by_package[package_name] = package_count

    stale_frozen_paths = frozen_migrations - scanned_paths
    for path in sorted(stale_frozen_paths):
        errors.append(f"冻结迁移清单未命中工作区源码: {path}")
    return len(scanned_paths), scanned_by_package, frozen_migrations


def mutable_function_records(
    records: list[FunctionRecord],
    frozen_migrations: set[str],
    retained_exceptions: set[tuple[str, str]],
) -> list[FunctionRecord]:
    """冻结迁移按摘要锁定；过渡期仍保留策略中已登记的旧例外。"""

    return [
        record
        for record in records
        if record.path not in frozen_migrations
        or (record.path, record.symbol) in retained_exceptions
    ]


def validate_test_layout(
    test_layout: dict[str, Any],
    packages: dict[str, dict[str, Any]],
    errors: list[str],
) -> tuple[int, int]:
    """允许私有逻辑就地单测，并保持公开契约测试属于当前 crate。"""

    directory = test_layout.get("directory")
    if directory != "tests":
        errors.append("test_layout.directory 必须固定为 tests")
        directory = "tests"
    if test_layout.get("allow_colocated_unit_tests") is not True:
        errors.append("test_layout.allow_colocated_unit_tests 必须为 true")
    if test_layout.get("forbid_source_test_files") is not True:
        errors.append("test_layout.forbid_source_test_files 必须为 true")
    max_integration_test_lines = test_layout.get("max_integration_test_lines")
    if (
        not isinstance(max_integration_test_lines, int)
        or max_integration_test_lines < 1
    ):
        errors.append("test_layout.max_integration_test_lines 必须是正整数")
        max_integration_test_lines = 1
    disabled_bin_test_targets = parse_string_set(
        test_layout.get("disabled_bin_test_targets"),
        "test_layout.disabled_bin_test_targets",
        errors,
    )

    actual_bin_targets = {
        f"{package_name}:{target.get('name')}"
        for package_name, package in packages.items()
        for target in package.get("targets", [])
        if "bin" in target.get("kind", [])
    }
    stale_disabled_bins = disabled_bin_test_targets - actual_bin_targets
    if stale_disabled_bins:
        errors.append(
            "空测试 harness 策略引用不存在的二进制目标: "
            + ", ".join(sorted(stale_disabled_bins))
        )

    checked_sources = 0
    integration_targets = 0
    for package_name, package in sorted(packages.items()):
        package_root = Path(package["manifest_path"]).resolve().parent
        try:
            package_root.relative_to(ROOT)
        except ValueError:
            errors.append(f"crate 位于仓库之外，无法检查测试位置: {package_name}")
            continue

        test_root = (package_root / directory).resolve()
        if test_root.is_dir():
            for path in sorted(test_root.rglob("*.rs")):
                lines = count_lines(path)
                if lines > max_integration_test_lines:
                    errors.append(
                        f"集成测试文件超过 {max_integration_test_lines} 行: "
                        f"{path.relative_to(ROOT).as_posix()}（{lines} 行）"
                    )
        for target in package.get("targets", []):
            if "bin" in target.get("kind", []):
                target_key = f"{package_name}:{target.get('name')}"
                test_disabled = target.get("test") is False
                if test_disabled and target_key not in disabled_bin_test_targets:
                    errors.append(f"关闭测试 harness 的二进制目标必须登记: {target_key}")
                if target_key in disabled_bin_test_targets:
                    if not test_disabled:
                        errors.append(f"二进制目标必须设置 test = false: {target_key}")
                    target_path = Path(target["src_path"]).resolve()
                    target_sources = {target_path}
                    companion_root = target_path.with_suffix("")
                    if companion_root.is_dir():
                        target_sources.update(companion_root.rglob("*.rs"))
                    package_targets = package.get("targets", [])
                    if not any("lib" in item.get("kind", []) for item in package_targets):
                        target_sources.update((package_root / "src").rglob("*.rs"))
                    for source_path in sorted(target_sources):
                        source = source_path.read_text(encoding="utf-8")
                        if TEST_ATTRIBUTE.search(source):
                            errors.append(
                                "已关闭测试 harness 的二进制源码不得包含单测: "
                                f"{source_path.relative_to(ROOT).as_posix()}"
                            )
            if "test" not in target.get("kind", []):
                continue
            integration_targets += 1
            target_path = Path(target["src_path"]).resolve()
            try:
                target_path.relative_to(test_root)
            except ValueError:
                errors.append(
                    f"{package_name} 的测试目标必须位于本 crate/tests: "
                    f"{target_path.relative_to(ROOT).as_posix()}"
                )

        source_root = package_root / "src"
        if not source_root.is_dir():
            continue
        for path in sorted(source_root.rglob("*.rs")):
            checked_sources += 1
            relative = path.relative_to(ROOT).as_posix()
            if TEST_FILE_NAME.search(path.name):
                errors.append(
                    f"源码内单测必须与私有实现同文件，独立测试文件移至所属 crate/tests: {relative}"
                )
    return checked_sources, integration_targets


def workspace_rust_sources(
    root: Path,
    profile: dict[str, Any],
    packages: dict[str, dict[str, Any]],
) -> list[Path]:
    sources: set[Path] = set()
    for package_name in sorted(profile.get("products", set()) | profile.get("tools", set())):
        package = packages.get(package_name)
        if package is None:
            continue
        package_root = Path(package["manifest_path"]).resolve().parent
        try:
            package_root.relative_to(root)
        except ValueError:
            continue
        source_root = package_root / "src"
        if source_root.is_dir():
            sources.update(source_root.rglob("*.rs"))
        test_root = package_root / "tests"
        if test_root.is_dir():
            sources.update(test_root.rglob("*.rs"))
        build_script = package_root / "build.rs"
        if build_script.is_file():
            sources.add(build_script)
    return sorted(sources)


def workspace_rust_templates(
    root: Path,
    profile: dict[str, Any],
    packages: dict[str, dict[str, Any]],
) -> list[Path]:
    templates: set[Path] = set()
    for package_name in sorted(profile.get("products", set()) | profile.get("tools", set())):
        package = packages.get(package_name)
        if package is None:
            continue
        package_root = Path(package["manifest_path"]).resolve().parent
        try:
            package_root.relative_to(root)
        except ValueError:
            continue
        templates.update(package_root.rglob("*.rs.tpl"))
    return sorted(path for path in templates if path.is_file())


def validate_legacy_persistence_apis(
    root: Path,
    sources: Iterable[Path],
    errors: list[str],
) -> tuple[int, int]:
    """旧持久化异步接口一律禁止重新进入产品、工具和测试源码。"""

    source_paths = sorted({path.resolve() for path in sources if path.is_file()})
    violations = 0
    for path in source_paths:
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(root).as_posix()
        for name in LEGACY_PERSISTENCE_API_NAMES:
            if re.search(rf"\b{re.escape(name)}\b", source):
                errors.append(f"工作区源码不得使用已删除的持久化接口 {name}: {relative}")
                violations += 1
    return len(source_paths), violations


def trait_body(source: str, opening_brace: int) -> str | None:
    """返回 trait 最外层花括号内容；不完整源码交由 Rust 编译器继续报告。"""

    depth = 1
    for index, character in enumerate(source[opening_brace + 1 :], opening_brace + 1):
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
            if depth == 0:
                return source[opening_brace + 1 : index]
    return None


def validate_async_port_traits(
    root: Path,
    sources: Iterable[Path],
    errors: list[str],
) -> tuple[int, int]:
    """禁止端口 trait 通过手写 Future 暴露异步操作。"""

    source_paths = sorted({path.resolve() for path in sources if path.is_file()})
    violations = 0
    for path in source_paths:
        source = path.read_text(encoding="utf-8")
        if "Future" not in source and "Pin<" not in source:
            continue
        relative = path.relative_to(root).as_posix()
        for declaration in TRAIT_DECLARATION.finditer(source):
            body = trait_body(source, declaration.end() - 1)
            if body is None:
                continue
            for method in TRAIT_FUTURE_METHOD.finditer(body):
                line = source.count("\n", 0, declaration.end() + method.start()) + 1
                trait_name = declaration.group("name")
                method_name = method.group("method")
                if method.group("async") is None:
                    errors.append(
                        "异步端口 trait 必须使用 async fn，而不得返回手写 Future: "
                        f"{relative}:{line} {trait_name}::{method_name}"
                    )
                else:
                    errors.append(
                        "异步端口 trait 不得从 async fn 返回手写 Future: "
                        f"{relative}:{line} {trait_name}::{method_name}"
                    )
                violations += 1
    return len(source_paths), violations


def main() -> int:
    errors: list[str] = []
    warnings: list[str] = []
    validate_documentation(ROOT, errors)
    validate_system_domain_surface(ROOT, errors)
    active_profile, profiles, source_size, test_layout, function_size = load_policy(errors)
    metadata = cargo_metadata(errors)
    packages = workspace_packages(metadata) if metadata else {}
    unsafe_lint_packages = validate_unsafe_lint_policy(ROOT, packages, errors) if packages else 0
    active = profiles.get(active_profile, {})
    actual_edges: set[tuple[str, str]] = set()
    scanned = 0
    scanned_by_package: dict[str, int] = {}
    frozen_migrations: set[str] = set()
    checked_test_sources = 0
    integration_targets = 0
    persistence_sources = 0
    legacy_persistence_violations = 0
    async_port_sources = 0
    async_port_violations = 0
    checked_unsafe_sources = 0
    unsafe_syntax_violations = 0
    checked_functions = 0
    function_size_violations = 0
    excluded_frozen_functions = 0
    if active and packages:
        actual_edges = validate_active_workspace(
            active_profile, active, packages, errors
        )
        scanned, scanned_by_package, frozen_migrations = validate_source_size(
            source_size, active, packages, errors, warnings
        )
        checked_test_sources, integration_targets = validate_test_layout(
            test_layout, packages, errors
        )
        persistence_sources, legacy_persistence_violations = validate_legacy_persistence_apis(
            ROOT,
            workspace_rust_sources(ROOT, active, packages),
            errors,
        )
        async_port_sources, async_port_violations = validate_async_port_traits(
            ROOT,
            workspace_rust_sources(ROOT, active, packages),
            errors,
        )
        rust_sources = workspace_rust_sources(ROOT, active, packages)
        functions, unsafe_syntax = parse_rust_sources(ROOT, rust_sources, errors)
        rust_templates = workspace_rust_templates(ROOT, active, packages)
        unsafe_syntax.extend(parse_template_unsafe(ROOT, rust_templates, errors))
        checked_unsafe_sources = len(rust_sources) + len(rust_templates)
        unsafe_syntax_violations = validate_no_unsafe(unsafe_syntax, errors)
        function_policy = parse_function_size_policy(function_size, errors)
        if function_policy is not None:
            retained_exceptions = {
                (exception.path, exception.symbol)
                for exception in function_policy.exceptions
            }
            mutable_functions = mutable_function_records(
                functions,
                frozen_migrations,
                retained_exceptions,
            )
            excluded_frozen_functions = len(functions) - len(mutable_functions)
            changed = (
                changed_line_ranges(ROOT, errors)
                if function_policy.mode == "changed"
                else None
            )
            checked_functions, function_size_violations = validate_functions(
                function_policy,
                mutable_functions,
                changed,
                errors,
            )

    validate_tenant_data_boundaries(ROOT, errors)

    if warnings:
        print("Architecture size notices:", file=sys.stderr)
        for warning in warnings:
            print(f"  - {warning}", file=sys.stderr)
    if errors:
        print("Architecture check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print(
        "Architecture check passed "
        f"(profile={active_profile}, packages={len(packages)}, "
        f"internal_edges={len(actual_edges)}, scc=0)."
    )
    profile_summary = ", ".join(
        f"{name}={len(profile.get('packages', set()))}"
        for name, profile in sorted(profiles.items())
    )
    print(f"Declared architecture profiles are valid ({profile_summary}).")
    print(
        "Workspace source-size coverage passed "
        f"(crates={len(scanned_by_package)}, files={scanned}, "
        f"max_lines={source_size.get('max_lines')})."
    )
    print(
        "Crate test-layout coverage passed "
        f"(source_files={checked_test_sources}, integration_targets={integration_targets})."
    )
    print(
        "Legacy persistence API gate passed "
        f"(source_files={persistence_sources}, violations={legacy_persistence_violations})."
    )
    print(
        "Async port interface gate passed "
        f"(source_files={async_port_sources}, violations={async_port_violations})."
    )
    print(
        "Rust function-size AST gate passed "
        f"(mode={function_size.get('mode')}, functions={checked_functions}, "
        f"frozen_functions={excluded_frozen_functions}, violations={function_size_violations})."
    )
    print(
        "Rust unsafe AST gate passed "
        f"(source_files={checked_unsafe_sources}, violations={unsafe_syntax_violations})."
    )
    print(f"Workspace unsafe lint inheritance passed (packages={unsafe_lint_packages}).")
    print("Tenant-data architecture boundaries are valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
