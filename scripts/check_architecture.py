#!/usr/bin/env python3
"""校验工作区 crate 图、源码规模与租户数据静态边界。"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
POLICY_PATH = ROOT / "architecture" / "crate-boundaries.toml"


def read(relative: str) -> str:
    return (ROOT / relative).read_text(encoding="utf-8")


def business_sources() -> list[Path]:
    result: list[Path] = []
    for path in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
        relative = path.relative_to(ROOT)
        lowered_parts = {part.lower() for part in relative.parts}
        source = path.read_text(encoding="utf-8")
        if (
            lowered_parts.intersection({"biz", "business", "tenant_business"})
            or path.name.lower().startswith("biz_")
            or "tenant-data-boundary: business" in source
        ):
            result.append(path)
    return result


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
    temporary_edges = parse_edges(
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
    run_legacy_checks = profile.get("run_tenant_data_legacy_checks", False)
    if not isinstance(run_legacy_checks, bool):
        errors.append(f"{label}.run_tenant_data_legacy_checks 必须是布尔值")
        run_legacy_checks = False

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
        "run_legacy_checks": run_legacy_checks,
    }


def load_policy(
    errors: list[str],
) -> tuple[str, dict[str, dict[str, Any]], dict[str, Any]]:
    try:
        policy = tomllib.loads(POLICY_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        errors.append(f"无法读取架构策略 {POLICY_PATH.relative_to(ROOT)}: {error}")
        return "", {}, {}

    if policy.get("schema_version") != 1:
        errors.append("架构策略 schema_version 必须为 1")
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
    return active_profile, profiles, source_size


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


def validate_source_size(
    source_size: dict[str, Any],
    profile: dict[str, Any],
    packages: dict[str, dict[str, Any]],
    errors: list[str],
) -> tuple[int, dict[str, int]]:
    max_lines = source_size.get("max_lines")
    if not isinstance(max_lines, int) or max_lines < 1:
        errors.append("source_size.max_lines 必须是正整数")
        return 0, {}

    raw_generated_exclusions = source_size.get("generated_exclusions", [])
    generated_exclusions: set[str] = set()
    if not isinstance(raw_generated_exclusions, list):
        errors.append("source_size.generated_exclusions 必须是 table 数组")
        raw_generated_exclusions = []
    for index, exclusion in enumerate(raw_generated_exclusions):
        label = f"source_size.generated_exclusions[{index}]"
        if not isinstance(exclusion, dict):
            errors.append(f"{label} 必须是 TOML table")
            continue
        path = exclusion.get("path")
        marker = exclusion.get("required_marker")
        reason = exclusion.get("reason")
        if not isinstance(path, str) or not path:
            errors.append(f"{label}.path 必须是非空字符串")
            continue
        if path in generated_exclusions:
            errors.append(f"生成源码排除路径重复: {path}")
        if not isinstance(marker, str) or not marker:
            errors.append(f"{label}.required_marker 必须是非空字符串")
            continue
        if not isinstance(reason, str) or not reason.strip():
            errors.append(f"{label}.reason 必须说明生成来源")
        generated_path = (ROOT / path).resolve()
        try:
            generated_path.relative_to(ROOT)
        except ValueError:
            errors.append(f"生成源码排除路径越出仓库: {path}")
            continue
        if not generated_path.is_file():
            errors.append(f"生成源码排除路径不存在: {path}")
        else:
            header = "\n".join(
                generated_path.read_text(encoding="utf-8").splitlines()[:5]
            )
            if marker not in header:
                errors.append(f"生成源码排除缺少规定标记: {path}")
        generated_exclusions.add(path)

    raw_exceptions = source_size.get("legacy_exceptions", [])
    exceptions: dict[str, int] = {}
    if not isinstance(raw_exceptions, list):
        errors.append("source_size.legacy_exceptions 必须是 table 数组")
        raw_exceptions = []
    for index, exception in enumerate(raw_exceptions):
        label = f"source_size.legacy_exceptions[{index}]"
        if not isinstance(exception, dict):
            errors.append(f"{label} 必须是 TOML table")
            continue
        path = exception.get("path")
        limit = exception.get("max_lines")
        reason = exception.get("reason")
        if not isinstance(path, str) or not path:
            errors.append(f"{label}.path 必须是非空字符串")
            continue
        if path in exceptions:
            errors.append(f"源码规模豁免路径重复: {path}")
        if not isinstance(limit, int) or limit <= max_lines:
            errors.append(f"{label}.max_lines 必须大于全局上限 {max_lines}")
            continue
        if not isinstance(reason, str) or not reason.strip():
            errors.append(f"{label}.reason 必须说明拆分债务")
        exceptions[path] = limit

    scanned_paths: set[str] = set()
    matched_generated_exclusions: set[str] = set()
    scanned_by_package: dict[str, int] = {}
    for package_name in sorted(profile.get("products", set())):
        package = packages.get(package_name)
        if package is None:
            continue
        package_root = Path(package["manifest_path"]).resolve().parent
        try:
            package_root.relative_to(ROOT)
        except ValueError:
            errors.append(f"生产 crate 位于仓库之外: {package_name}（{package_root}）")
            continue
        source_root = package_root / "src"
        candidates = list(source_root.rglob("*.rs")) if source_root.is_dir() else []
        build_script = package_root / "build.rs"
        if build_script.is_file():
            candidates.append(build_script)

        package_count = 0
        for path in sorted(set(candidates)):
            relative = path.relative_to(ROOT).as_posix()
            if relative in generated_exclusions:
                matched_generated_exclusions.add(relative)
                continue
            scanned_paths.add(relative)
            package_count += 1
            lines = count_lines(path)
            limit = exceptions.get(relative, max_lines)
            if lines > limit:
                errors.append(f"源码文件超过 {limit} 行: {relative}（{lines} 行）")
            if relative in exceptions and lines <= max_lines:
                errors.append(f"源码文件已降至全局上限内，应删除豁免: {relative}")
        if package_count == 0:
            errors.append(f"生产 crate 没有纳入任何 Rust 源文件: {package_name}")
        scanned_by_package[package_name] = package_count

    stale_exception_paths = set(exceptions) - scanned_paths
    for path in sorted(stale_exception_paths):
        errors.append(f"源码规模豁免未命中生产源文件，应删除: {path}")
    stale_generated_paths = generated_exclusions - matched_generated_exclusions
    for path in sorted(stale_generated_paths):
        errors.append(f"生成源码排除未命中生产源文件，应删除: {path}")
    return len(scanned_paths), scanned_by_package


def validate_tenant_data_boundaries(errors: list[str]) -> None:
    """保留现有租户数据专项门禁，后续随 crate 搬迁更新定位。"""

    tenant_manifest = read("crates/ryframe-tenant-db/Cargo.toml")
    for forbidden in ("ryframe-application", "ryframe-api"):
        if re.search(rf"(?m)^\s*{re.escape(forbidden)}\s*=", tenant_manifest):
            errors.append(f"ryframe-tenant-db must not depend on {forbidden}")

    use_case_template = read("crates/ryframe-generator/src/template/use_case.rs")
    repository_template = read("crates/ryframe-generator/src/template/repository.rs")
    tenant_data_repository = read(
        "crates/ryframe-db/src/repositories/tenant_data_repo.rs"
    )
    catalog_template = read("crates/ryframe-generator/src/template/catalog.rs")
    generator_engine = read("crates/ryframe-generator/src/engine.rs")
    for fragment in (
        "DataSource",
        "RepositoryPort",
        ".begin(tenant_id)",
        ".commit(transaction)",
        ".rollback(transaction)",
        ".insert(&transaction",
    ):
        if fragment not in use_case_template:
            errors.append(f"generator use-case template misses application boundary: {fragment}")
    for forbidden in (
        "ryframe_db",
        "ryframe_tenant_db",
        "ryframe_adapters",
        "ryframe_http",
        "sea_orm",
        "axum",
    ):
        if forbidden in use_case_template:
            errors.append(f"generator use-case template crosses application boundary: {forbidden}")
    for fragment in (
        "connection: &DatabaseConnection",
        "transaction: &DatabaseTransaction",
        "find_by_id",
        "insert",
        "update",
        "delete",
        ".reset_all()",
    ):
        if fragment not in repository_template:
            errors.append(f"generator repository template misses SQL boundary: {fragment}")
    for forbidden in (".begin(", ".commit(", ".rollback(", "TransactionTrait"):
        if forbidden in repository_template:
            errors.append(f"generator repository template owns transaction boundary: {forbidden}")
    if tenant_data_repository.count(".reset_all()") < 3:
        errors.append(
            "tenant-data repository saves must mark mutated model fields for UPDATE"
        )
    for fragment in ('starts_with("biz_")', 'column.name == "tenant_id"'):
        if fragment not in generator_engine:
            errors.append(f"generator business-table validation misses: {fragment}")
    for fragment in (
        "TenantDataTableDescriptor",
        "primary_key_cursor_columns",
        "checksum_columns",
        "foreign_key_dependencies",
        "GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT",
    ):
        if fragment not in catalog_template:
            errors.append(f"generator catalog template misses: {fragment}")
    for forbidden in ("ControlDatabaseCluster", ".write(", ".source("):
        if forbidden in use_case_template or forbidden in repository_template:
            errors.append(f"generator business template reaches control data source: {forbidden}")

    for path in business_sources():
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(ROOT).as_posix()
        is_generated_sql_boundary = relative.startswith(
            "crates/ryframe-db/src/repositories/business/"
        )
        forbidden_type_pattern = (
            r"\b(?:ControlDatabaseCluster|TenantDataTargetHandle|"
            r"TenantDatabaseTargetRegistry)\b"
            if is_generated_sql_boundary
            else r"\b(?:ControlDatabaseCluster|DatabaseConnection|"
            r"TenantDataTargetHandle|TenantDatabaseTargetRegistry)\b"
        )
        forbidden_types = re.search(forbidden_type_pattern, source)
        forbidden_methods = re.search(
            r"\.(?:write|source|open_target(?:_for_catalog)?|verify_target_now(?:_for_catalog)?|"
            r"target_occupancy(?:_for_catalog)?|tenant_is_empty_on_target(?:_for_catalog)?|"
            r"prepare_migration_target(?:_for_catalog)?|freeze_fence(?:_for_catalog)?|"
            r"activate_fence(?:_for_catalog)?|clear_prepared_target(?:_for_catalog)?|"
            r"cleanup_ownership_for_catalog|delete_tenant_rows_batch(?:_for_catalog)?|"
            r"finish_tenant_cleanup_for_catalog|finalize_retained_source(?:_for_catalog)?|"
            r"runtime_snapshot|verify_current_targets|placement_metrics_snapshot|"
            r"prepare_provisioning|provision_tenant_fence|provision_pending_fence)\(",
            source,
        )
        if forbidden_types or forbidden_methods:
            errors.append(
                "tenant business module bypasses TenantDataSession: "
                f"{relative}"
            )

    generated_catalog = read(
        "crates/ryframe-tenant-db/src/migration/generated_catalog.rs"
    )
    migration_module = read("crates/ryframe-tenant-db/src/migration/mod.rs")
    migration_catalog = read("crates/ryframe-tenant-db/src/migration/catalog.rs")
    if "mod generated_catalog;" not in migration_module:
        errors.append("tenant-data generated catalog is not compiled into the migration module")
    for fragment in (
        "GENERATED_TENANT_DATA_TABLES",
        "GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT",
    ):
        if fragment not in generated_catalog or fragment not in migration_catalog:
            errors.append(f"tenant-data compiled catalog misses: {fragment}")

    adapters_multi_tenant = ROOT / "crates/ryframe-adapters/src/multi_tenant.rs"
    if adapters_multi_tenant.is_file():
        source = adapters_multi_tenant.read_text(encoding="utf-8")
        for removed in ("IsolationStrategy", "TenantFilter"):
            if re.search(rf"\b{removed}\b", source):
                errors.append(f"removed multi-tenant shell remains: {removed}")


def main() -> int:
    errors: list[str] = []
    active_profile, profiles, source_size = load_policy(errors)
    metadata = cargo_metadata(errors)
    packages = workspace_packages(metadata) if metadata else {}
    active = profiles.get(active_profile, {})
    actual_edges: set[tuple[str, str]] = set()
    scanned = 0
    scanned_by_package: dict[str, int] = {}
    if active and packages:
        actual_edges = validate_active_workspace(
            active_profile, active, packages, errors
        )
        scanned, scanned_by_package = validate_source_size(
            source_size, active, packages, errors
        )

    ran_tenant_checks = bool(active.get("run_legacy_checks"))
    if ran_tenant_checks:
        validate_tenant_data_boundaries(errors)

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
        "Production source-size coverage passed "
        f"(crates={len(scanned_by_package)}, files={scanned}, "
        f"max_lines={source_size.get('max_lines')})."
    )
    if ran_tenant_checks:
        print("Tenant-data architecture boundaries are valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
