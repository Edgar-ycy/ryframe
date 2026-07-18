#!/usr/bin/env python3
"""Fail CI when workspace or source-level architecture boundaries drift."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

EXPECTED_DEPENDENCIES = {
    "ryframe": {
        "ryframe-api",
        "ryframe-auth",
        "ryframe-common",
        "ryframe-config",
        "ryframe-core",
        "ryframe-db",
        "ryframe-db-migration",
        "ryframe-middleware",
        "ryframe-monitor",
        "ryframe-service",
        "ryframe-storage",
    },
    "ryframe-api": {
        "ryframe-auth",
        "ryframe-common",
        "ryframe-config",
        "ryframe-core",
        "ryframe-macro",
        "ryframe-middleware",
        "ryframe-monitor",
        "ryframe-service",
    },
    "ryframe-auth": {"ryframe-common", "ryframe-config", "ryframe-core"},
    "ryframe-common": set(),
    "ryframe-config": {"ryframe-common"},
    "ryframe-core": {"ryframe-common", "ryframe-config"},
    "ryframe-db": {"ryframe-common", "ryframe-config", "ryframe-core", "ryframe-macro"},
    "ryframe-db-migration": {"ryframe-common"},
    "ryframe-generator": {"ryframe-common"},
    "ryframe-macro": {"ryframe-core"},
    "ryframe-middleware": {
        "ryframe-auth",
        "ryframe-common",
        "ryframe-config",
        "ryframe-core",
    },
    "ryframe-monitor": {
        "ryframe-auth",
        "ryframe-common",
        "ryframe-core",
        "ryframe-macro",
        "ryframe-middleware",
    },
    "ryframe-service": {
        "ryframe-auth",
        "ryframe-common",
        "ryframe-config",
        "ryframe-core",
        "ryframe-db",
        "ryframe-generator",
        "ryframe-storage",
    },
    "ryframe-storage": set(),
}


def workspace_dependencies() -> dict[str, set[str]]:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    metadata = json.loads(result.stdout)
    workspace_names = {package["name"] for package in metadata["packages"]}
    return {
        package["name"]: {
            dependency["name"]
            for dependency in package["dependencies"]
            if dependency["name"] in workspace_names and dependency["kind"] != "dev"
        }
        for package in metadata["packages"]
    }


def check_dependency_graph(errors: list[str]) -> None:
    actual = workspace_dependencies()
    if actual.keys() != EXPECTED_DEPENDENCIES.keys():
        errors.append(
            "workspace crate set changed; update scripts/check_architecture.py intentionally"
        )
        return

    for crate in sorted(actual):
        added = actual[crate] - EXPECTED_DEPENDENCIES[crate]
        removed = EXPECTED_DEPENDENCIES[crate] - actual[crate]
        if added:
            errors.append(f"{crate} added internal dependencies: {', '.join(sorted(added))}")
        if removed:
            errors.append(
                f"{crate} removed internal dependencies; update the baseline: "
                f"{', '.join(sorted(removed))}"
            )


def rust_sources(relative_dir: str) -> list[Path]:
    return sorted((ROOT / relative_dir).rglob("*.rs"))


def production_rust_sources() -> list[Path]:
    return sorted(ROOT.glob("crates/*/src/**/*.rs"))


def attributed_functions(path: Path) -> list[tuple[str, list[str]]]:
    functions: list[tuple[str, list[str]]] = []
    attributes: list[str] = []
    current_attribute: list[str] | None = None
    attribute_depth = 0

    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if current_attribute is not None:
            current_attribute.append(stripped)
            attribute_depth += stripped.count("[") - stripped.count("]")
            if attribute_depth <= 0:
                attributes.append(" ".join(current_attribute))
                current_attribute = None
            continue

        if stripped.startswith("#["):
            current_attribute = [stripped]
            attribute_depth = stripped.count("[") - stripped.count("]")
            if attribute_depth <= 0:
                attributes.append(stripped)
                current_attribute = None
            continue

        function = re.search(r"\b(?:pub\s+)?async\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)", stripped)
        if function:
            functions.append((function.group(1), attributes))
            attributes = []
            continue

        if stripped and not stripped.startswith(("///", "//")):
            attributes = []

    return functions


def function_signature(source: str, function: str) -> str:
    match = re.search(
        rf"\b(?:pub\s+)?async\s+fn\s+{re.escape(function)}\s*\((.*?)\)\s*(?:->|\{{)",
        source,
        re.DOTALL,
    )
    return match.group(1) if match else ""


def check_openapi_registration(errors: list[str]) -> None:
    openapi_source = (ROOT / "crates/ryframe-api/src/openapi.rs").read_text(encoding="utf-8")
    handlers_root = ROOT / "crates/ryframe-api/src/handlers"
    route_prefixes = ("#[get(", "#[post(", "#[put(", "#[delete(")

    for path in rust_sources("crates/ryframe-api/src/handlers"):
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(handlers_root)
        module = relative.parts[0].removesuffix(".rs")
        for function, attributes in attributed_functions(path):
            route_attributes = [
                attribute for attribute in attributes if attribute.startswith(route_prefixes)
            ]
            has_openapi = any(attribute.startswith("#[utoipa::path") for attribute in attributes)

            if route_attributes and not has_openapi:
                errors.append(
                    f"route handler is missing #[utoipa::path]: "
                    f"{path.relative_to(ROOT)}::{function}"
                )
            for route_attribute in route_attributes:
                if re.search(r'"[^"\\]*(?:\\.[^"\\]*)*"\s*,\s*"', route_attribute):
                    errors.append(
                        f"route handler declares compatibility aliases: "
                        f"{path.relative_to(ROOT)}::{function}"
                    )

            if has_openapi:
                registration = f"crate::handlers::{module}::{function}"
                if registration not in openapi_source:
                    errors.append(f"OpenAPI path is not registered: {registration}")

                openapi_attributes = " ".join(
                    attribute
                    for attribute in attributes
                    if attribute.startswith("#[utoipa::path")
                )
                documented_query = re.search(
                    r"\bparams\s*\(\s*([A-Za-z_][A-Za-z0-9_:]*)\s*\)",
                    openapi_attributes,
                )
                signature = function_signature(source, function)
                extracted_query = re.search(
                    r"\bQuery\s*\([^)]*\)\s*:\s*Query\s*<\s*"
                    r"([A-Za-z_][A-Za-z0-9_:]*)\s*>",
                    signature,
                )
                handler_name = f"{path.relative_to(ROOT)}::{function}"

                if documented_query and not extracted_query:
                    errors.append(
                        f"OpenAPI documents query parameters but the handler does not "
                        f"extract Query: {handler_name}"
                    )
                if extracted_query and not documented_query:
                    errors.append(
                        f"handler extracts Query but OpenAPI does not document its "
                        f"parameters: {handler_name}"
                    )
                if documented_query and extracted_query:
                    documented_type = documented_query.group(1).split("::")[-1]
                    extracted_type = extracted_query.group(1).split("::")[-1]
                    if documented_type != extracted_type:
                        errors.append(
                            f"OpenAPI query type {documented_type} does not match handler "
                            f"extractor {extracted_type}: {handler_name}"
                        )


def check_compiled_permission_catalog(errors: list[str]) -> None:
    service_source = (
        ROOT / "crates/ryframe-service/src/system/permission_service.rs"
    ).read_text(encoding="utf-8")
    forbidden_runtime_scanner = re.compile(
        r"\b(?:CARGO_MANIFEST_DIR|scan_permission_codes|read_to_string|read_dir)\b"
    )
    if forbidden_runtime_scanner.search(service_source):
        errors.append("permission service scans source files at runtime")

    required_fragments = {
        "crates/ryframe-api/build.rs": (
            "syn::parse_file",
            "crates/ryframe-monitor/src",
            "permission_catalog.rs",
        ),
        "crates/ryframe-api/src/permission_catalog.rs": (
            'include!(concat!(env!("OUT_DIR"), "/permission_catalog.rs"))',
            "route_permission_codes",
        ),
        "crates/ryframe-api/src/handlers/permission_handler.rs": (
            "permission_catalog::route_permission_codes()",
            "sync_route_permissions",
        ),
    }
    for relative_path, fragments in required_fragments.items():
        path = ROOT / relative_path
        if not path.is_file():
            errors.append(f"compiled permission catalog file is missing: {relative_path}")
            continue
        source = path.read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"compiled permission catalog contract is missing in {relative_path}: "
                    f"{fragment}"
                )


def menu_route_contract(
    document: dict[str, object], errors: list[str]
) -> set[tuple[str, str]]:
    extension = document.get("x-ryframe-menu-routes")
    if not isinstance(extension, dict):
        errors.append("OpenAPI is missing x-ryframe-menu-routes")
        return set()
    if extension.get("version") != 1:
        errors.append("x-ryframe-menu-routes uses an unsupported version")

    routes = extension.get("routes")
    if not isinstance(routes, list):
        errors.append("x-ryframe-menu-routes.routes must be an array")
        return set()

    contract: set[tuple[str, str]] = set()
    route_keys: set[str] = set()
    for index, route in enumerate(routes):
        if not isinstance(route, dict):
            errors.append(f"menu route contract entry {index} must be an object")
            continue
        route_key = route.get("route_key")
        menu_type = route.get("menu_type")
        if not isinstance(route_key, str) or not re.fullmatch(
            r"[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*", route_key
        ):
            errors.append(f"menu route contract entry {index} has an invalid route_key")
            continue
        if menu_type not in {"M", "C"}:
            errors.append(f"menu route contract entry {route_key} has an invalid menu_type")
            continue
        if route_key in route_keys:
            errors.append(f"menu route contract contains duplicate route_key {route_key}")
            continue
        route_keys.add(route_key)
        contract.add((route_key, menu_type))

    if len(contract) < 21:
        errors.append(
            f"menu route contract unexpectedly shrank: found {len(contract)} entries"
        )
    return contract


def check_menu_route_sources(
    contract: set[tuple[str, str]], errors: list[str]
) -> None:
    if not contract:
        return

    sql_source = (ROOT / "sql/ryframe_config.sql").read_text(encoding="utf-8")
    if "INSERT IGNORE INTO" in sql_source:
        errors.append(
            "generated MySQL snapshot must not suppress seed errors with INSERT IGNORE"
        )
    insert = re.search(
        r"INSERT INTO `sys_menu`\s*\([^;]+?\)\s*VALUES(?P<rows>.*?);",
        sql_source,
        re.DOTALL,
    )
    if insert is None:
        errors.append("default sys_menu seed statement is missing")
        return

    row_pattern = re.compile(
        r"\(\s*\d+\s*,\s*'(?:''|[^'])*'\s*,\s*(?:NULL|\d+)\s*,\s*"
        r"'([MCF])'\s*,\s*(?:NULL|\d+)\s*,\s*(NULL|'((?:''|[^'])*)')\s*,",
        re.DOTALL,
    )
    sql_routes: set[tuple[str, str]] = set()
    matched_rows = 0
    for match in row_pattern.finditer(insert.group("rows")):
        matched_rows += 1
        menu_type = match.group(1)
        route_key = match.group(3)
        if menu_type in {"M", "C"}:
            if route_key is None:
                errors.append(f"default {menu_type} menu is missing route_key")
            else:
                sql_routes.add((route_key, menu_type))
        elif route_key is not None:
            errors.append(f"default button menu unexpectedly declares route_key {route_key}")
    if matched_rows == 0:
        errors.append("default sys_menu seed rows could not be parsed")
        return
    if sql_routes != contract:
        missing = sorted(contract - sql_routes)
        extra = sorted(sql_routes - contract)
        if missing:
            errors.append(f"SQL menu seed is missing route contracts: {missing}")
        if extra:
            errors.append(f"SQL menu seed has undeclared route contracts: {extra}")

    migration_source = (
        ROOT
        / "crates/ryframe-db-migration/src/m20260701_000002_menu_permission_binding.rs"
    ).read_text(encoding="utf-8")
    route_backfill = re.search(
        r"async fn backfill_route_keys\b.*?(?=async fn backfill_permission_ids\b)",
        migration_source,
        re.DOTALL,
    )
    if route_backfill is None:
        errors.append("route-key migration backfill function is missing")
        return
    migration_keys = set(
        re.findall(
            r"WHEN\s+'(?:''|[^'])*'\s+THEN\s+'((?:''|[^'])*)'",
            route_backfill.group(),
        )
    )
    contract_keys = {route_key for route_key, _ in contract}
    if migration_keys != contract_keys:
        missing = sorted(contract_keys - migration_keys)
        extra = sorted(migration_keys - contract_keys)
        if missing:
            errors.append(f"route-key migration is missing keys: {missing}")
        if extra:
            errors.append(f"route-key migration has undeclared keys: {extra}")


def check_password_policy(document: dict[str, object], errors: list[str]) -> None:
    expected = {
        "version": 1,
        "min_length": 8,
        "max_length": 72,
        "pattern": r"^(?=.*[A-Z])(?=.*[a-z])(?=.*[0-9])(?=.*[^A-Za-z0-9])[!-~]{8,72}$",
        "allowed_characters": "ascii_graphic",
        "required_classes": ["uppercase", "lowercase", "digit", "special"],
    }
    if document.get("x-ryframe-password-policy") != expected:
        errors.append("OpenAPI password policy does not match the canonical strong policy")

    schemas = document.get("components", {})
    if not isinstance(schemas, dict):
        return
    schemas = schemas.get("schemas", {})
    if not isinstance(schemas, dict):
        return
    for schema_name, field_name in (
        ("ChangePasswordRequest", "new_password"),
        ("CompletePasswordResetRequest", "new_password"),
        ("CreateTenantDto", "admin_password"),
    ):
        schema = schemas.get(schema_name, {})
        property_schema = (
            schema.get("properties", {}).get(field_name, {})
            if isinstance(schema, dict)
            else {}
        )
        if not isinstance(property_schema, dict) or any(
            property_schema.get(key) != value
            for key, value in (
                ("minLength", expected["min_length"]),
                ("maxLength", expected["max_length"]),
                ("pattern", expected["pattern"]),
            )
        ):
            errors.append(f"{schema_name}.{field_name} does not expose password policy")


def check_openapi_contract_pipeline(errors: list[str]) -> None:
    required_fragments = {
        "crates/ryframe-api/src/bin/export_openapi.rs": (
            "ApiDoc::openapi()",
            "render_openapi_json",
        ),
        "crates/ryframe-api/src/openapi.rs": (
            "pub fn render_openapi_json",
            "serde_json::to_value(document)",
            "checked_in_contract_snapshot_is_current",
            "x-ryframe-menu-routes",
            "x-ryframe-password-policy",
            "query_operation_count >= 34",
            "must document its success response schema",
            "must document its request body",
            "full-record operation must not document pagination parameters",
        ),
        "crates/ryframe-api/src/macros.rs": (
            "utoipa::IntoParams",
            "parameter_in = Query",
        ),
        ".github/workflows/ci.yml": (
            "cargo run --locked -p ryframe-api --bin export_openapi",
            "git diff --exit-code -- openapi/openapi.json",
            "name: ryframe-openapi",
            "runtime-smoke:",
            "node deploy/tests/smoke-test.js",
        ),
        "deploy/tests/smoke-test.js": (
            'process.env.TENANT_ID || "system"',
            'return { Authorization: `Bearer ${token}`, "X-Tenant-Id": TENANT_ID };',
            "/api/v1/system/perms/tree",
            'json?.["x-ryframe-password-policy"]',
        ),
        "crates/ryframe/src/app.rs": (
            '"/livez"',
            '"/readyz"',
            ".merge(probes)",
            'never pass through authentication',
        ),
    }
    for relative_path, fragments in required_fragments.items():
        path = ROOT / relative_path
        if not path.is_file():
            errors.append(f"OpenAPI contract pipeline file is missing: {relative_path}")
            continue
        source = path.read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"OpenAPI contract pipeline is missing in {relative_path}: {fragment}"
                )

    snapshot = ROOT / "openapi/openapi.json"
    if not snapshot.is_file():
        errors.append("canonical OpenAPI snapshot is missing: openapi/openapi.json")
        return
    document = json.loads(snapshot.read_text(encoding="utf-8"))
    if not str(document.get("openapi", "")).startswith("3."):
        errors.append("canonical OpenAPI snapshot has an unsupported version")
    if len(document.get("paths", {})) < 89:
        errors.append("canonical OpenAPI path coverage unexpectedly shrank")
    if len(document.get("components", {}).get("schemas", {})) < 153:
        errors.append("canonical OpenAPI schema coverage unexpectedly shrank")
    check_menu_route_sources(menu_route_contract(document, errors), errors)
    check_password_policy(document, errors)


def check_source_boundaries(errors: list[str]) -> None:
    implicit_tenant_access = re.compile(
        r"\b(?:current_tenant_id|set_debug_tenant_fallback)\b"
    )
    for path in production_rust_sources():
        source = path.read_text(encoding="utf-8")
        if implicit_tenant_access.search(source):
            errors.append(
                f"production code exposes implicit tenant access: {path.relative_to(ROOT)}"
            )
        if re.search(r"\b(?:enable_password_complexity|enforce_complexity)\b", source):
            errors.append(
                f"production code makes the canonical password policy optional: "
                f"{path.relative_to(ROOT)}"
            )

    task_local_tenant_access = re.compile(r"\bwith_tenant_context\b")
    for relative_dir in ("crates/ryframe-db/src", "crates/ryframe-service/src"):
        for path in rust_sources(relative_dir):
            if task_local_tenant_access.search(path.read_text(encoding="utf-8")):
                errors.append(
                    "data or service layer depends on task-local tenant context: "
                    f"{path.relative_to(ROOT)}"
                )

    forbidden_handler_dependency = re.compile(r"\b(?:ryframe_db|sea_orm)::")
    handler_database_access = re.compile(r"\bstate\.db\b")
    handler_redis_access = re.compile(r"\bstate\.redis\b")
    handler_collection_pagination = re.compile(
        r"\.skip\s*\([^)]*\)\s*\.take\s*\(", re.DOTALL
    )
    for path in rust_sources("crates/ryframe-api/src"):
        source = path.read_text(encoding="utf-8")
        if forbidden_handler_dependency.search(source):
            errors.append(
                f"API production code imports database implementation: {path.relative_to(ROOT)}"
            )

    for path in rust_sources("crates/ryframe-api/src/handlers"):
        source = path.read_text(encoding="utf-8")
        if handler_database_access.search(source):
            errors.append(f"HTTP handler accesses AppState.db: {path.relative_to(ROOT)}")
        if handler_redis_access.search(source):
            errors.append(f"HTTP handler accesses AppState.redis: {path.relative_to(ROOT)}")
        if handler_collection_pagination.search(source):
            errors.append(
                f"HTTP handler paginates an in-memory collection: {path.relative_to(ROOT)}"
            )
        if ".route(" in source:
            errors.append(
                f"HTTP handler bypasses the project route macros: {path.relative_to(ROOT)}"
            )

    forbidden_cross_cutting_database = re.compile(
        r"\b(?:ryframe_db|sea_orm|DatabaseConnection)\b"
    )
    for relative_dir in ("crates/ryframe-auth/src", "crates/ryframe-monitor/src"):
        for path in rust_sources(relative_dir):
            if forbidden_cross_cutting_database.search(
                path.read_text(encoding="utf-8")
            ):
                errors.append(
                    "cross-cutting crate imports a database implementation: "
                    f"{path.relative_to(ROOT)}"
                )

    forbidden_service_dependency = re.compile(r"\b(?:axum|ryframe_api)::")
    public_repository_field = re.compile(
        r"\bpub(?:\([^)]*\))?\s+[A-Za-z_][A-Za-z0-9_]*repo[A-Za-z0-9_]*\s*:"
    )
    public_database_parameter = re.compile(
        r"\bpub\s+async\s+fn\s+[A-Za-z_][A-Za-z0-9_]*[^{}]*"
        r"\bdb\s*:\s*&DatabaseConnection",
        re.DOTALL,
    )
    for path in rust_sources("crates/ryframe-service/src"):
        source = path.read_text(encoding="utf-8")
        if forbidden_service_dependency.search(source):
            errors.append(f"service imports HTTP layer: {path.relative_to(ROOT)}")
        if public_repository_field.search(source):
            errors.append(f"service exposes a repository field: {path.relative_to(ROOT)}")
        if public_database_parameter.search(source):
            errors.append(
                f"public service method exposes DatabaseConnection: {path.relative_to(ROOT)}"
            )

    database_storage_dependency = re.compile(
        r"\b(?:ryframe_storage|ObjectStorage|LocalObjectStorage|S3ObjectStorage)\b"
        r"|\.public_url\s*\("
    )
    for path in rust_sources("crates/ryframe-db/src"):
        if database_storage_dependency.search(path.read_text(encoding="utf-8")):
            errors.append(
                "database layer owns object storage or URL presentation logic: "
                f"{path.relative_to(ROOT)}"
            )

    blocking_redis_keys = re.compile(
        r"redis::cmd\s*\(\s*\"KEYS\"|pub\s+async\s+fn\s+keys\s*\("
    )
    for path in production_rust_sources():
        if blocking_redis_keys.search(path.read_text(encoding="utf-8")):
            errors.append(
                f"production code exposes blocking Redis KEYS: {path.relative_to(ROOT)}"
            )

    redis_client_source = (ROOT / "crates/ryframe-core/src/redis_client.rs").read_text(
        encoding="utf-8"
    )
    for fragment in ("GET_AND_DEL_SCRIPT", 'redis::cmd("EVAL")', "scan_keys"):
        if fragment not in redis_client_source:
            errors.append(f"Redis safety contract is missing: {fragment}")

    detached_cache_invalidation = re.compile(r"\btokio::spawn\b")
    for relative_path in (
        "crates/ryframe-service/src/system/dept_service.rs",
        "crates/ryframe-service/src/system/menu_service.rs",
    ):
        path = ROOT / relative_path
        if detached_cache_invalidation.search(path.read_text(encoding="utf-8")):
            errors.append(f"cache invalidation is detached: {relative_path}")


def check_database_and_storage_topology(errors: list[str]) -> None:
    required_fragments = {
        "crates/ryframe-config/src/db_config.rs": (
            "pub primary: DbConnection",
            "pub replicas: Vec<DatabaseReplicaConfig>",
            "pub sources: Vec<DatabaseSourceConfig>",
            "pub name: String",
        ),
        "crates/ryframe-db/src/cluster.rs": (
            "AtomicUsize",
            "pub fn write(&self) -> &DatabaseConnection",
            "pub fn read(&self) -> &DatabaseConnection",
            "pub fn source(&self, name: &str) -> Option<&DatabaseConnection>",
            "fetch_add(1, Ordering::Relaxed)",
        ),
        "crates/ryframe/src/boot/datasource.rs": (
            "config.database.primary",
            "config.database.replicas",
            "config.database.sources",
            "DatabaseCluster::with_sources(primary, replicas, sources)",
            "verify_schema",
        ),
        "crates/ryframe-config/src/object_storage_config.rs": (
            "Rustfs",
            'Self::Rustfs => "rustfs"',
        ),
        "crates/ryframe/src/boot/storage.rs": (
            "StorageBackend::Rustfs",
            "storage.ensure_bucket(bucket).await",
        ),
        ".github/workflows/ci.yml": (
            "APP_DATABASE_REPLICAS",
            "APP_DATABASE_SOURCES",
            "Prepare and test named data source",
            "Test RustFS adapter",
        ),
        "docker-compose.test.yml": (
            "mysql:8.4",
            "redis:7-alpine",
            "rustfs/rustfs:1.0.0-beta.8",
        ),
        "deploy/tests/smoke-test.js": (
            'test("runtime topology"',
            'test("ryframe_device generator source"',
            'test("RustFS upload and download"',
        ),
    }
    for relative_path, fragments in required_fragments.items():
        source = (ROOT / relative_path).read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"database/storage topology contract is missing in "
                    f"{relative_path}: {fragment}"
                )

    for path in rust_sources("crates/ryframe-service/src"):
        source = path.read_text(encoding="utf-8")
        if re.search(r"\bdb\s*:\s*DatabaseConnection\b", source):
            errors.append(
                f"service stores a raw database connection: {path.relative_to(ROOT)}"
            )
        if "&self.db" in source or re.search(r"\bself\.db\.begin\s*\(", source):
            errors.append(
                f"service bypasses explicit read/write routing: {path.relative_to(ROOT)}"
            )


def main() -> int:
    errors: list[str] = []
    check_dependency_graph(errors)
    check_source_boundaries(errors)
    check_database_and_storage_topology(errors)
    check_openapi_registration(errors)
    check_openapi_contract_pipeline(errors)
    check_compiled_permission_catalog(errors)
    if errors:
        print("Architecture check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print("Architecture boundaries are valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
