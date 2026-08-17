#!/usr/bin/env python3
"""校验租户业务数据与控制面的静态依赖边界。"""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


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


def main() -> int:
    errors: list[str] = []

    tenant_manifest = read("crates/ryframe-tenant-db/Cargo.toml")
    for forbidden in ("ryframe-kernel", "ryframe-service", "ryframe-api"):
        if re.search(rf"(?m)^\s*{re.escape(forbidden)}\s*=", tenant_manifest):
            errors.append(f"ryframe-tenant-db must not depend on {forbidden}")

    migration_manifest = read("crates/ryframe-tenant-db-migration/Cargo.toml")
    if re.search(r"(?m)^\s*ryframe-db-migration\s*=", migration_manifest):
        errors.append("tenant-data migrator must not depend on the control migrator")

    service_template = read("crates/ryframe-generator/src/template/service.rs")
    repository_template = read("crates/ryframe-generator/src/template/repository.rs")
    tenant_data_repository = read(
        "crates/ryframe-db/src/repositories/tenant_data_repo.rs"
    )
    catalog_template = read("crates/ryframe-generator/src/template/catalog.rs")
    generator_engine = read("crates/ryframe-generator/src/engine.rs")
    for fragment in (
        "Arc<TenantDatabaseRouter>",
        ".resolve(tenant_id)",
        ".find_by_page(&session",
        ".insert(&session",
    ):
        if fragment not in service_template:
            errors.append(f"generator service template misses tenant-data chain: {fragment}")
    for fragment in (
        "TenantDataSession",
        ".select_read(ReadConsistency::Eventual)",
        ".begin_write()",
        "find_by_id",
        "insert",
        "update",
        "delete",
        ".reset_all()",
    ):
        if fragment not in repository_template:
            errors.append(f"generator repository template misses transaction support: {fragment}")
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
    for forbidden in ("ControlDatabaseCluster", "DatabaseConnection", ".write(", ".source("):
        if forbidden in service_template or forbidden in repository_template:
            errors.append(f"generator business template reaches control data source: {forbidden}")

    for path in business_sources():
        source = path.read_text(encoding="utf-8")
        forbidden_types = re.search(
            r"\b(?:ControlDatabaseCluster|DatabaseConnection|"
            r"TenantDataTargetHandle|TenantDatabaseTargetRegistry)\b",
            source,
        )
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
                f"{path.relative_to(ROOT)}"
            )

    generated_catalog = read(
        "crates/ryframe-tenant-db-migration/src/generated_catalog.rs"
    )
    migration_lib = read("crates/ryframe-tenant-db-migration/src/lib.rs")
    migration_catalog = read("crates/ryframe-tenant-db-migration/src/catalog.rs")
    if "mod generated_catalog;" not in migration_lib:
        errors.append("tenant-data generated catalog is not compiled into the migrator")
    for fragment in (
        "GENERATED_TENANT_DATA_TABLES",
        "GENERATED_TENANT_DATA_SCHEMA_FINGERPRINT",
    ):
        if fragment not in generated_catalog or fragment not in migration_catalog:
            errors.append(f"tenant-data compiled catalog misses: {fragment}")

    core_multi_tenant = ROOT / "crates/ryframe-core/src/multi_tenant.rs"
    if core_multi_tenant.is_file():
        source = core_multi_tenant.read_text(encoding="utf-8")
        for removed in ("IsolationStrategy", "TenantFilter"):
            if re.search(rf"\b{removed}\b", source):
                errors.append(f"removed multi-tenant shell remains: {removed}")

    if errors:
        print("Architecture check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print("Tenant-data architecture boundaries are valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
