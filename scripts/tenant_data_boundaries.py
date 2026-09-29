"""核对当前资源清单、生成复制目录和租户业务持久化边界。"""

from __future__ import annotations

import re
import tomllib
from pathlib import Path


def validate_tenant_data_boundaries(root: Path, errors: list[str]) -> None:
    expected = []
    for path in sorted((root / "catalog/resources").glob("*.toml")):
        resource = tomllib.loads(path.read_text(encoding="utf-8"))
        if resource.get("storage", {}).get("kind") == "tenant_data":
            expected.append(resource["database"]["table"])
    catalog = read_required(root, "crates/ryframe-tenant-db/src/generated/catalog.rs", errors)
    tables = re.findall(r'\btable:\s*"([a-zA-Z0-9_]+)"', catalog)
    if sorted(tables) != sorted(expected):
        errors.append(f"租户数据复制目录与资源清单不一致：expected={sorted(expected)}, actual={sorted(tables)}")
    required = {
        "crates/ryframe-tenant-db/src/generated/mod.rs": ["pub mod catalog;"],
        "crates/ryframe-tenant-db/src/migration/catalog.rs": [
            "tables: crate::generated::catalog::GENERATED_TENANT_DATA_TABLES",
            "tenant_data_schema_fingerprint()", "self.validate_structure()?",
            "computed != tenant_data_schema_fingerprint()",
        ],
        "crates/ryframe-generator/src/resource/render/tenant_catalog.rs": [
            "TenantDataTableDescriptor", "primary_key_cursor_columns", "checksum_columns",
            "column_types", "schema_canonical", "schema::column_type(field)",
        ],
        "crates/ryframe-generator/src/resource/render/slice/migration.rs": ["super::super::schema::column_type(field)"],
    }
    for relative, fragments in required.items():
        source = read_required(root, relative, errors)
        for fragment in fragments:
            if fragment not in source:
                errors.append(f"租户数据生成或校验链路缺少 {fragment}：{relative}")
    validate_business_repositories(root, errors)


def read_required(root: Path, relative: str, errors: list[str]) -> str:
    try:
        return (root / relative).read_text(encoding="utf-8")
    except OSError:
        errors.append(f"租户数据边界所需源码缺失：{relative}")
        return ""


def validate_business_repositories(root: Path, errors: list[str]) -> None:
    bypass = re.compile(
        r"\b(?:ControlDatabaseCluster|TenantDataTargetHandle|TenantDatabaseTargetRegistry)\b|"
        r"\.(?:write|source|open_target(?:_for_catalog)?|verify_target_now(?:_for_catalog)?|"
        r"target_occupancy(?:_for_catalog)?|prepare_migration_target(?:_for_catalog)?|"
        r"freeze_fence(?:_for_catalog)?|activate_fence(?:_for_catalog)?|"
        r"delete_tenant_rows_batch(?:_for_catalog)?|runtime_snapshot|prepare_provisioning)\("
    )
    for path in sorted((root / "crates/ryframe-tenant-db/src/generated").glob("*/repository.rs")):
        source = path.read_text(encoding="utf-8")
        if bypass.search(source):
            errors.append(f"租户业务 Repository 绕过受控会话：{path.relative_to(root).as_posix()}")
