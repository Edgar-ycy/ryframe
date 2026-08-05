#!/usr/bin/env python3
"""当工作区或源码级架构边界发生偏移时使 CI 失败。"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from functools import cache
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

EXPECTED_DEPENDENCIES = {
    "ryframe": {
        "ryframe-api",
        "ryframe-auth",
        "ryframe-config",
        "ryframe-core",
        "ryframe-db",
        "ryframe-db-migration",
        "ryframe-i18n",
        "ryframe-kernel",
        "ryframe-middleware",
        "ryframe-monitor",
        "ryframe-service",
        "ryframe-storage",
        "ryframe-utils",
    },
    "ryframe-api": {
        "ryframe-auth",
        "ryframe-captcha",
        "ryframe-config",
        "ryframe-core",
        "ryframe-excel",
        "ryframe-http",
        "ryframe-i18n",
        "ryframe-kernel",
        "ryframe-macro",
        "ryframe-middleware",
        "ryframe-monitor",
        "ryframe-service",
        "ryframe-utils",
    },
    "ryframe-auth": {"ryframe-config", "ryframe-core", "ryframe-http", "ryframe-kernel"},
    "ryframe-captcha": {"ryframe-kernel"},
    "ryframe-config": {"ryframe-kernel", "ryframe-utils"},
    "ryframe-core": {"ryframe-config", "ryframe-kernel"},
    "ryframe-db": {
        "ryframe-config",
        "ryframe-core",
        "ryframe-kernel",
        "ryframe-macro",
        "ryframe-utils",
    },
    "ryframe-db-migration": {"ryframe-utils"},
    "ryframe-excel": {"ryframe-kernel"},
    "ryframe-generator": {"ryframe-kernel"},
    "ryframe-http": {"ryframe-kernel"},
    "ryframe-i18n": set(),
    "ryframe-kernel": set(),
    "ryframe-macro": {"ryframe-core"},
    "ryframe-mail": set(),
    "ryframe-middleware": {
        "ryframe-auth",
        "ryframe-config",
        "ryframe-core",
        "ryframe-http",
        "ryframe-i18n",
        "ryframe-kernel",
        "ryframe-utils",
    },
    "ryframe-monitor": {
        "ryframe-auth",
        "ryframe-core",
        "ryframe-http",
        "ryframe-kernel",
        "ryframe-macro",
        "ryframe-middleware",
    },
    "ryframe-service": {
        "ryframe-auth",
        "ryframe-config",
        "ryframe-core",
        "ryframe-db",
        "ryframe-excel",
        "ryframe-generator",
        "ryframe-kernel",
        "ryframe-storage",
        "ryframe-utils",
    },
    "ryframe-storage": set(),
    "ryframe-utils": {"ryframe-kernel"},
    "xtask": set(),
}

KERNEL_FORBIDDEN_DEPENDENCIES = (
    "axum",
    "axum-extra",
    "sea-orm",
    "sea-orm-migration",
    "redis",
    "image",
    "calamine",
    "rust_xlsxwriter",
    "lettre",
    "reqwest",
    "tokio",
    "ryframe-common",
    "ryframe-http",
    "ryframe-utils",
    "ryframe-captcha",
    "ryframe-excel",
    "ryframe-mail",
    "ryframe-api",
    "ryframe-auth",
    "ryframe-config",
    "ryframe-core",
    "ryframe-db",
    "ryframe-db-migration",
    "ryframe-generator",
    "ryframe-middleware",
    "ryframe-monitor",
    "ryframe-service",
    "ryframe-storage",
)


@cache
def workspace_metadata() -> dict[str, object]:
    """一次读取 Cargo 元数据，供依赖和 feature 守卫共同复用。"""
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(result.stdout)


def workspace_dependencies() -> dict[str, set[str]]:
    metadata = workspace_metadata()
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


def feature_registry_violations(
    metadata: dict[str, object], registry: object
) -> list[str]:
    """校验 feature 注册表完整覆盖工作区的最小和最大组合。"""
    violations: list[str] = []
    if not isinstance(registry, dict) or registry.get("version") != 1:
        return ["feature registry version must be 1"]
    entries = registry.get("packages")
    if not isinstance(entries, list):
        return ["feature registry packages must be an array"]

    workspace_members = set(metadata.get("workspace_members", []))
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        return ["Cargo metadata packages must be an array"]
    available_by_package: dict[str, set[str]] = {}
    for package in packages:
        if not isinstance(package, dict) or package.get("id") not in workspace_members:
            continue
        name = package.get("name")
        features = package.get("features")
        if not isinstance(name, str) or not isinstance(features, dict):
            violations.append("Cargo metadata contains an invalid workspace package")
            continue
        available_by_package[name] = set(features)

    feature_packages = {
        name for name, features in available_by_package.items() if features
    }
    registered_packages: set[str] = set()
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            violations.append(f"feature registry packages[{index}] must be an object")
            continue
        package = entry.get("package")
        if not isinstance(package, str) or not package.strip():
            violations.append(f"feature registry packages[{index}] has no package")
            continue
        if package in registered_packages:
            violations.append(f"feature registry duplicates package: {package}")
            continue
        registered_packages.add(package)
        available = available_by_package.get(package)
        if available is None:
            violations.append(f"feature registry contains unknown package: {package}")
            continue
        if not available:
            violations.append(f"feature registry includes featureless package: {package}")
            continue

        combinations: dict[str, set[str]] = {}
        for label in ("minimal", "maximal"):
            values = entry.get(label)
            if not isinstance(values, list) or any(
                not isinstance(value, str) or not value.strip() for value in values
            ):
                violations.append(
                    f"feature registry {package}.{label} must be a string array"
                )
                continue
            selected = set(values)
            if len(selected) != len(values):
                violations.append(
                    f"feature registry {package}.{label} contains duplicates"
                )
            unknown = selected - available
            if unknown:
                violations.append(
                    f"feature registry {package}.{label} contains unknown features: "
                    f"{', '.join(sorted(unknown))}"
                )
            combinations[label] = selected

        minimal = combinations.get("minimal")
        maximal = combinations.get("maximal")
        if minimal is None or maximal is None:
            continue
        if not minimal <= maximal:
            violations.append(
                f"feature registry {package}.minimal is not a subset of maximal"
            )
        if maximal != available:
            missing = available - maximal
            extra = maximal - available
            detail = sorted(missing | extra)
            violations.append(
                f"feature registry {package}.maximal does not exactly cover features: "
                f"{', '.join(detail)}"
            )

    if registered_packages != feature_packages:
        missing = feature_packages - registered_packages
        extra = registered_packages - feature_packages
        violations.append(
            "feature registry package set differs from Cargo metadata; "
            f"missing=[{', '.join(sorted(missing))}], "
            f"extra=[{', '.join(sorted(extra))}]"
        )
    return violations


def check_feature_registry(errors: list[str]) -> None:
    """CI 只校验元数据，不重复编译本地 feature 矩阵。"""
    path = ROOT / "config/feature-matrix.json"
    try:
        registry = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"cannot read feature registry: {error}")
        return
    errors.extend(feature_registry_violations(workspace_metadata(), registry))


def check_removed_common_crate(errors: list[str]) -> None:
    """禁止已删除的 common 包、目录、依赖和 Rust 导入重新出现。"""
    removed_path = ROOT / "crates/ryframe-common"
    if removed_path.exists():
        errors.append("removed crate path exists again: crates/ryframe-common")

    for path in [ROOT / "Cargo.toml", *sorted((ROOT / "crates").glob("*/Cargo.toml"))]:
        source = path.read_text(encoding="utf-8")
        if re.search(r"\bryframe-common\b", source):
            errors.append(
                "manifest references removed ryframe-common: "
                f"{path.relative_to(ROOT)}"
            )

    for path in sorted((ROOT / "crates").glob("**/*.rs")):
        if re.search(r"\bryframe_common\b", path.read_text(encoding="utf-8")):
            errors.append(
                "Rust source imports removed ryframe-common: "
                f"{path.relative_to(ROOT)}"
            )


REMOVED_CONFIG_CRYPTO_SYMBOL = re.compile(
    r"\b(?:config_crypto|ConfigCrypto|CONFIG_MASTER_KEY)\b"
)
PRODUCTION_FILE_SECRET_KEYS = {
    "password",
    "jwt_secret",
    "access_key",
    "secret_key",
    "metrics_bearer_token",
}


def exposes_removed_config_crypto(source: str) -> bool:
    """识别已删除的配置加解密模块、类型和主密钥环境变量。"""
    return REMOVED_CONFIG_CRYPTO_SYMBOL.search(source) is not None


def configured_secret_paths(value: object, path: str = "") -> list[str]:
    """返回生产配置合并输入中包含非占位敏感值的路径。"""
    violations: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = f"{path}.{key}" if path else key
            if (
                key in PRODUCTION_FILE_SECRET_KEYS
                and isinstance(child, str)
                and child != ""
                and not (
                    key == "jwt_secret" and child == "change-me-in-production"
                )
            ):
                violations.append(child_path)
            violations.extend(configured_secret_paths(child, child_path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            violations.extend(configured_secret_paths(child, f"{path}[{index}]"))
    return violations


def check_secret_source_policy(errors: list[str]) -> None:
    """锁定生产 secret 文件注入边界，禁止旧 ENC/AES 兼容实现回流。"""
    removed_module = ROOT / "crates/ryframe-config/src/config_crypto.rs"
    if removed_module.exists():
        errors.append("removed configuration crypto module exists again")

    config_manifest = (
        ROOT / "crates/ryframe-config/Cargo.toml"
    ).read_text(encoding="utf-8")
    for dependency in ("aes-gcm", "base64", "rand"):
        if re.search(rf"(?m)^\s*{re.escape(dependency)}\s*=", config_manifest):
            errors.append(
                f"configuration crate restores removed crypto dependency: {dependency}"
            )

    workspace_manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    if re.search(r"(?m)^\s*aes-gcm\s*=", workspace_manifest):
        errors.append("workspace restores unused AES configuration dependency")

    for path in rust_sources("crates/ryframe-config/src"):
        if exposes_removed_config_crypto(path.read_text(encoding="utf-8")):
            errors.append(
                "configuration source restores removed crypto surface: "
                f"{path.relative_to(ROOT)}"
            )

    app_config = (
        ROOT / "crates/ryframe-config/src/app_config.rs"
    ).read_text(encoding="utf-8")
    file_guard = app_config.find("reject_production_file_secrets(&table)?;")
    environment_override = app_config.find("apply_env_overrides(&mut table)?;")
    removed_encoding_guard = app_config.find("reject_removed_secret_encoding(&table)?;")
    if not 0 <= file_guard < environment_override < removed_encoding_guard:
        errors.append(
            "configuration loading must reject file secrets before environment overrides "
            "and reject removed ENC values afterwards"
        )

    override_spec = (
        ROOT / "crates/ryframe-config/src/app_config/environment_overrides/spec.rs"
    ).read_text(encoding="utf-8")
    override_runtime = (
        ROOT / "crates/ryframe-config/src/app_config/environment_overrides/mod.rs"
    ).read_text(encoding="utf-8")
    file_capable_overrides = {
        "APP_MONITOR_METRICS_BEARER_TOKEN": "secret",
        "APP_DATABASE_PASSWORD": "secret",
        "APP_DATABASE_REPLICAS": "json_file",
        "APP_DATABASE_SOURCES": "json_file",
        "APP_AUTH_JWT_SECRET": "secret",
        "APP_REDIS_PASSWORD": "secret",
        "APP_OBJECT_STORAGE_ACCESS_KEY": "secret",
        "APP_OBJECT_STORAGE_SECRET_KEY": "secret",
    }
    for name, constructor in file_capable_overrides.items():
        if not re.search(
            rf'EnvOverride::{constructor}\(\s*"{re.escape(name)}"',
            override_spec,
        ):
            errors.append(f"sensitive override does not support _FILE input: {name}")
    for fragment in (
        'format!("{}_FILE", spec.name)',
        "direct_value.is_some() && file_path.is_some()",
        "read_override_file(&file_name, &path)?",
        "String::from_utf8(bytes)",
    ):
        if fragment not in override_runtime:
            errors.append(f"secret file override guard is missing: {fragment}")

    production_compose = (ROOT / "deploy/compose.prod.yml").read_text(encoding="utf-8")
    compose_file_overrides = (
        "APP_DATABASE_PASSWORD_FILE",
        "APP_DATABASE_REPLICAS_FILE",
        "APP_DATABASE_SOURCES_FILE",
        "APP_REDIS_PASSWORD_FILE",
        "APP_OBJECT_STORAGE_ACCESS_KEY_FILE",
        "APP_OBJECT_STORAGE_SECRET_KEY_FILE",
        "APP_AUTH_JWT_SECRET_FILE",
        "APP_MONITOR_METRICS_BEARER_TOKEN_FILE",
    )
    for name in compose_file_overrides:
        if not re.search(rf"(?m)^\s+{re.escape(name)}:\s+/run/secrets/", production_compose):
            errors.append(f"production Compose does not use a mounted secret file: {name}")
    for name in (item.removesuffix("_FILE") for item in compose_file_overrides):
        if re.search(rf"(?m)^\s+{re.escape(name)}:\s+", production_compose):
            errors.append(f"production Compose restores direct secret injection: {name}")

    for relative_path in ("config/app.toml", "config/app.prod.toml"):
        path = ROOT / relative_path
        with path.open("rb") as source:
            configured_secrets = configured_secret_paths(tomllib.load(source))
        for secret_path in configured_secrets:
            errors.append(
                f"production configuration input contains a secret: "
                f"{relative_path}:{secret_path}"
            )


def check_kernel_manifest(errors: list[str]) -> None:
    """确保领域核心 crate 不倒灌传输、存储或运行时依赖。"""
    relative_path = "crates/ryframe-kernel/Cargo.toml"
    manifest_path = ROOT / relative_path
    if not manifest_path.is_file():
        errors.append(f"kernel crate manifest is missing: {relative_path}")
        return

    manifest = manifest_path.read_text(encoding="utf-8")
    for dependency in KERNEL_FORBIDDEN_DEPENDENCIES:
        if re.search(rf"(?m)^\s*{re.escape(dependency)}\s*=", manifest):
            errors.append(
                f"kernel crate must not depend on {dependency}: {relative_path}"
            )


TEST_SOURCE_DIRECTORY_NAMES = frozenset({"benches", "tests", "src-tests"})


def rust_sources(relative_dir: str) -> list[Path]:
    return sorted(
        path
        for path in (ROOT / relative_dir).rglob("*.rs")
        if not TEST_SOURCE_DIRECTORY_NAMES.intersection(path.relative_to(ROOT).parts)
    )


def production_rust_sources() -> list[Path]:
    return sorted(ROOT.glob("crates/*/src/**/*.rs"))


UNSIGNED_REPLAY_HEADER = re.compile(r'''(?i)["']x-(?:nonce|timestamp)["']''')


def exposes_unsigned_replay_contract(source: str) -> bool:
    return UNSIGNED_REPLAY_HEADER.search(source) is not None


def check_unsigned_replay_contract(errors: list[str]) -> None:
    """拒绝已废弃且未签名的 X-Nonce/X-Timestamp 伪协议。

    客户端控制的 nonce 和时间戳不是已认证请求组成部分。机器间持有者证明必须采用
    单独评审过的消息签名契约，不得重新启用这些请求头。
    """
    for path in production_rust_sources():
        if exposes_unsigned_replay_contract(path.read_text(encoding="utf-8")):
            errors.append(
                "production code reintroduces the unsigned replay-header contract: "
                f"{path.relative_to(ROOT)}"
            )


def check_removed_compatibility_surfaces(errors: list[str]) -> None:
    """禁止重新引入已经删除的迁移入口和 Serde 字段别名。"""
    migration_source = (
        ROOT / "crates/ryframe-db-migration/src/lib.rs"
    ).read_text(encoding="utf-8")
    if re.search(
        r"\bpub\s+async\s+fn\s+run\s*\(|\bpub\s+use\b[^;]*\bas\s+run\b",
        migration_source,
        re.DOTALL,
    ):
        errors.append(
            "ryframe-db-migration must expose up/status/verify without a run alias"
        )

    for path in production_rust_sources():
        if re.search(r"#\[serde\s*\([^\]]*\balias\s*=", path.read_text(encoding="utf-8")):
            errors.append(
                f"production code declares a removed Serde field alias: {path.relative_to(ROOT)}"
            )

    idempotency_source = (
        ROOT / "crates/ryframe-middleware/src/idempotency.rs"
    ).read_text(encoding="utf-8")
    if 'const KEY_PREFIX: &str = "ryframe:v0.7:idempotency:";' not in idempotency_source:
        errors.append("idempotency key namespace must use only the v0.7 contract")
    if "ryframe:v0.6:idempotency:" in idempotency_source:
        errors.append("idempotency runtime restores the removed v0.6 key namespace")


REMOVED_OPER_LOG_JOB_PATTERNS = {
    "legacy operation-log job constant": re.compile(r"\bOPER_LOG_" r"JOB_TYPE\b"),
    "legacy operation-log job handler": re.compile(r"\bOperLog" r"JobHandler\b"),
    "legacy operation-log enqueue method": re.compile(r"\benqueue_oper" r"_log\s*\("),
    "legacy operation-log job type": re.compile(r"system\.oper_log\." r"record"),
}


def removed_oper_log_job_violations(sources: dict[str, str]) -> list[str]:
    """识别已经由 audit.operation Outbox 取代的旧操作日志任务链路。"""
    violations: list[str] = []
    for path, source in sorted(sources.items()):
        for description, pattern in REMOVED_OPER_LOG_JOB_PATTERNS.items():
            if pattern.search(source):
                violations.append(f"{description} appears again: {path}")
    return violations


def check_removed_oper_log_job(errors: list[str]) -> None:
    """禁止旧 JobQueue 审计任务回流，并锁定新的事务 Outbox 消费边界。"""
    sources = {
        path.relative_to(ROOT).as_posix(): path.read_text(encoding="utf-8")
        for path in sorted(ROOT.glob("crates/**/*.rs"))
    }
    errors.extend(removed_oper_log_job_violations(sources))

    jobs_source = sources["crates/ryframe-service/src/jobs.rs"]
    middleware_source = sources["crates/ryframe-api/src/oper_log_middleware.rs"]
    if "AUDIT_OPERATION_OUTBOX_EVENT_TYPE" not in jobs_source:
        errors.append("Outbox Worker does not consume audit.operation events")
    if "AuditOutbox" not in middleware_source or "scope_audit_request" not in middleware_source:
        errors.append("operation-log middleware does not use the transactional audit Outbox")


def check_removed_repository_wrapper(errors: list[str]) -> None:
    """禁止不产生日志的仓储包装层重新进入生产代码、生成模板或文档。"""
    paths = [
        *production_rust_sources(),
        *sorted((ROOT / "crates/ryframe-generator/src/template").glob("*.rs")),
        *sorted((ROOT / "docs").glob("*.md")),
    ]
    removed_name = "Logged" + "Repo"
    for path in paths:
        if removed_name in path.read_text(encoding="utf-8"):
            errors.append(
                "removed repository wrapper appears again: "
                f"{path.relative_to(ROOT)}"
            )


def check_http_error_boundary(errors: list[str]) -> None:
    """确保 HTTP 边界只保留单向领域错误适配。"""
    http_source = (ROOT / "crates/ryframe-http/src/lib.rs").read_text(encoding="utf-8")
    if re.search(r"\bpub\s+enum\s+AppError\b", http_source):
        errors.append("ryframe-http reintroduces a duplicate AppError enum")
    if re.search(r"\bpub\s+type\s+AppResult\b", http_source):
        errors.append("ryframe-http reintroduces the removed AppResult alias")
    if "pub type HttpResult<T> = Result<T, HttpAppError>;" not in http_source:
        errors.append("ryframe-http does not expose the explicit HttpResult boundary type")

    removed_reference = re.compile(
        r"\bryframe_http::(?:AppError|AppResult)\b"
        r"|\buse\s+ryframe_http::\{[^;]*\b(?:AppError|AppResult)\b[^;]*\};",
        re.DOTALL,
    )
    paths = [
        *production_rust_sources(),
        ROOT / "crates/ryframe-generator/src/template/handler.rs",
    ]
    for path in paths:
        if removed_reference.search(path.read_text(encoding="utf-8")):
            errors.append(
                "removed HTTP error compatibility symbol appears again: "
                f"{path.relative_to(ROOT)}"
            )


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
        "crates/ryframe-api/src/openapi.rs": (
            "permission_catalog_contract",
            "x-ryframe-permission-catalog",
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


def check_openapi_permission_catalog(
    document: dict[str, object], errors: list[str]
) -> None:
    """确保前端使用的权限目录与编译期路由权限保持同源。"""
    extension = document.get("x-ryframe-permission-catalog")
    if not isinstance(extension, dict) or extension.get("version") != 1:
        errors.append("OpenAPI permission catalog is missing or has an unsupported version")
        return
    codes = extension.get("codes")
    if not isinstance(codes, list) or not all(isinstance(code, str) for code in codes):
        errors.append("OpenAPI permission catalog codes must be a string array")
        return
    if codes != sorted(set(codes)):
        errors.append("OpenAPI permission catalog codes must be sorted and unique")

    compiled_codes: set[str] = set()
    permission_pattern = re.compile(r'#\s*\[\s*perm\s*\(\s*"([^"]+)"\s*\)\s*\]')
    for root in ("crates/ryframe-api/src", "crates/ryframe-monitor/src"):
        for path in rust_sources(root):
            compiled_codes.update(permission_pattern.findall(path.read_text(encoding="utf-8")))
    if set(codes) != compiled_codes:
        errors.append("OpenAPI permission catalog does not match compiled route permissions")


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


def openapi_api_prefix_violations(
    document: object, canonical_prefix: str
) -> list[str]:
    """要求 OpenAPI 扩展与后端唯一 API 前缀完全一致。"""
    if not isinstance(document, dict):
        return ["canonical OpenAPI snapshot must contain a JSON object"]
    extension_name = "x-ryframe-api-prefix"
    if extension_name not in document:
        return [f"canonical OpenAPI snapshot is missing {extension_name}"]
    expected = {"version": 1, "value": canonical_prefix}
    actual = document[extension_name]
    if actual != expected:
        return [
            f"canonical OpenAPI {extension_name} is {actual!r}, expected {expected!r}"
        ]
    return []


def openapi_json_success_envelope_violations(
    document: object, canonical_prefix: str
) -> list[str]:
    """要求公开 API 的 JSON 成功响应只引用当前统一信封 Schema。"""
    if not isinstance(document, dict):
        return ["canonical OpenAPI snapshot must contain a JSON object"]
    paths = document.get("paths")
    if not isinstance(paths, dict):
        return ["canonical OpenAPI snapshot must contain a paths object"]

    allowed_exact = "#/components/schemas/ApiEmptyResponse"
    allowed_prefixes = (
        "#/components/schemas/ApiResponse_",
        "#/components/schemas/ApiPageResponse_",
    )
    violations: list[str] = []
    for path, path_item in paths.items():
        if not isinstance(path, str) or not (
            path == canonical_prefix or path.startswith(f"{canonical_prefix}/")
        ):
            continue
        if not isinstance(path_item, dict):
            continue
        for method in ("get", "post", "put", "delete", "patch"):
            operation = path_item.get(method)
            if not isinstance(operation, dict):
                continue
            responses = operation.get("responses")
            if not isinstance(responses, dict):
                continue
            for status, response in responses.items():
                if not str(status).startswith("2") or not isinstance(response, dict):
                    continue
                content = response.get("content")
                if not isinstance(content, dict):
                    continue
                for media_type, media in content.items():
                    normalized_media_type = str(media_type).split(";", maxsplit=1)[0]
                    normalized_media_type = normalized_media_type.strip().lower()
                    if (
                        normalized_media_type != "application/json"
                        and not normalized_media_type.endswith("+json")
                    ):
                        continue
                    schema = media.get("schema") if isinstance(media, dict) else None
                    schema_ref = schema.get("$ref") if isinstance(schema, dict) else None
                    if schema_ref != allowed_exact and not (
                        isinstance(schema_ref, str)
                        and schema_ref.startswith(allowed_prefixes)
                    ):
                        violations.append(
                            f"OpenAPI {method.upper()} {path} {status} "
                            f"{media_type} success response bypasses the unified "
                            f"envelope: {schema_ref!r}"
                        )
    return violations


def check_openapi_contract_pipeline(errors: list[str]) -> None:
    required_fragments = {
        "crates/ryframe-api/src/bin/export_openapi.rs": (
            "ApiDoc::openapi()",
            "render_openapi_json",
        ),
        "crates/ryframe-api/src/openapi.rs": (
            "pub fn render_openapi_json",
            "serde_json::to_value(document)",
            "x-ryframe-menu-routes",
            "x-ryframe-password-policy",
            "x-ryframe-permission-catalog",
        ),
        "crates/ryframe-api/src/macros.rs": (
            "utoipa::IntoParams",
            "parameter_in = Query",
        ),
        "crates/ryframe/src/app.rs": (
            '"/livez"',
            '"/readyz"',
            ".merge(probes)",
            '绝不会经过认证',
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
    http_source = (ROOT / "crates/ryframe-http/src/lib.rs").read_text(encoding="utf-8")
    prefix_matches = re.findall(
        r'pub const API_PREFIX: &str = "([^"]+)";', http_source
    )
    if len(prefix_matches) == 1:
        errors.extend(openapi_api_prefix_violations(document, prefix_matches[0]))
        errors.extend(
            openapi_json_success_envelope_violations(document, prefix_matches[0])
        )
    else:
        errors.append(
            "canonical OpenAPI API prefix cannot be checked because API_PREFIX is ambiguous"
        )
    check_menu_route_sources(menu_route_contract(document, errors), errors)
    check_password_policy(document, errors)
    check_openapi_permission_catalog(document, errors)


def public_dto_conversion_violations(source: str) -> list[str]:
    """检查 API 公共 DTO 是否保持所有权转换和穷尽解构约束。"""
    violations: list[str] = []
    conversion_types = re.findall(
        r"\bimpl\s+From<(Service[A-Za-z0-9_]+)>\s+for\s+[A-Za-z0-9_]+",
        source,
    )
    destructures = re.findall(
        r"\blet\s+(Service[A-Za-z0-9_]+)\s*\{(?P<fields>.*?)\}\s*=\s*value\s*;",
        source,
        re.DOTALL,
    )

    if len(conversion_types) != 32:
        violations.append(
            f"public DTO conversion count changed: expected 32, found {len(conversion_types)}"
        )
    if len(destructures) != 32:
        violations.append(
            f"public DTO destructure count changed: expected 32, found {len(destructures)}"
        )

    destructured_types = [name for name, _ in destructures]
    if sorted(conversion_types) != sorted(destructured_types):
        violations.append("public DTO conversions must destructure their owned source types")
    if any(".." in fields for _, fields in destructures):
        violations.append("public DTO conversions must not use a rest-pattern destructure")
    if re.search(r"\bimpl\s+(?:Try)?From<\s*&Service", source):
        violations.append("public DTO conversions must consume service values by ownership")
    if re.search(r"(?m)^\s*_\s*=>", source):
        violations.append("public DTO enum conversions must not use wildcard match arms")
    if re.search(r"(?m)^\s*pub\s+type\s+", source):
        violations.append("public DTO boundary must not expose compatibility type aliases")
    return violations


def check_public_dto_boundary(errors: list[str]) -> None:
    """锁定 OpenAPI 模型所有权，禁止传输注解回流到业务层。"""
    schema_markers = re.compile(r"\b(?:ToSchema|IntoParams|utoipa::)|#\[schema\b")
    schema_free_sources = [
        *rust_sources("crates/ryframe-service/src"),
        ROOT / "crates/ryframe-generator/src/engine.rs",
        ROOT / "crates/ryframe-generator/src/schema.rs",
        ROOT / "crates/ryframe-utils/src/file_upload.rs",
    ]
    for path in schema_free_sources:
        if schema_markers.search(path.read_text(encoding="utf-8")):
            errors.append(
                "transport schema annotation leaked outside API: "
                f"{path.relative_to(ROOT)}"
            )

    for crate in ("ryframe-service", "ryframe-generator", "ryframe-utils"):
        relative_path = f"crates/{crate}/Cargo.toml"
        manifest = (ROOT / relative_path).read_text(encoding="utf-8")
        if re.search(r"(?m)^\s*utoipa\s*=", manifest):
            errors.append(f"{crate} directly depends on utoipa: {relative_path}")

    openapi_source = (ROOT / "crates/ryframe-api/src/openapi.rs").read_text(
        encoding="utf-8"
    )
    if "ryframe_service::" in openapi_source:
        errors.append("OpenAPI components reference service-owned response schemas")

    service_response = re.compile(
        r"\bApi(?:Page)?Response\s*<\s*ryframe_service::"
    )
    for path in rust_sources("crates/ryframe-api/src/handlers"):
        if service_response.search(path.read_text(encoding="utf-8")):
            errors.append(
                "HTTP handler exposes a service-owned response type: "
                f"{path.relative_to(ROOT)}"
            )

    public_dto_source = (
        ROOT / "crates/ryframe-api/src/dto/public_dto.rs"
    ).read_text(encoding="utf-8")
    errors.extend(public_dto_conversion_violations(public_dto_source))

    service_file_source = (
        ROOT / "crates/ryframe-service/src/system/file_service.rs"
    ).read_text(encoding="utf-8")
    service_upload = re.search(
        r"pub\s+struct\s+UploadResponse\s*\{(?P<fields>.*?)\}",
        service_file_source,
        re.DOTALL,
    )
    service_upload_fields = (
        re.findall(r"\bpub\s+([A-Za-z0-9_]+)\s*:", service_upload.group("fields"))
        if service_upload is not None
        else []
    )
    if service_upload_fields != ["file_id", "bucket", "file_name", "file_path"]:
        errors.append("service upload response must expose storage identity without an HTTP URL")
    if re.search(r"\bfn\s+build_file_url\b", service_file_source):
        errors.append("service layer must not build private HTTP file URLs")

    api_upload = re.search(
        r"pub\s+struct\s+UploadResponse\s*\{(?P<fields>.*?)\}",
        public_dto_source,
        re.DOTALL,
    )
    api_upload_fields = (
        re.findall(r"\bpub\s+([A-Za-z0-9_]+)\s*:", api_upload.group("fields"))
        if api_upload is not None
        else []
    )
    if api_upload_fields != ["file_id", "file_name", "file_path", "file_url"]:
        errors.append("API upload response must own the public file URL contract")
    if "fn private_file_url(" not in public_dto_source:
        errors.append("API public DTO boundary is missing private_file_url")

    with (ROOT / "crates/ryframe-http/Cargo.toml").open("rb") as source:
        http_manifest = tomllib.load(source)
    http_features = http_manifest.get("features", {})
    utoipa_dependency = http_manifest.get("dependencies", {}).get("utoipa")
    if http_features.get("default") != []:
        errors.append("ryframe-http must keep OpenAPI support disabled by default")
    if http_features.get("openapi") != ["dep:utoipa"]:
        errors.append("ryframe-http OpenAPI feature must exclusively enable utoipa")
    if not isinstance(utoipa_dependency, dict) or not utoipa_dependency.get("optional"):
        errors.append("ryframe-http utoipa dependency must remain optional")

    feature_consumers = {
        "ryframe-api": True,
        "ryframe-monitor": True,
        "ryframe-auth": False,
    }
    for crate, should_enable in feature_consumers.items():
        with (ROOT / f"crates/{crate}/Cargo.toml").open("rb") as source:
            manifest = tomllib.load(source)
        http_dependency = manifest.get("dependencies", {}).get("ryframe-http", {})
        enabled_features = (
            http_dependency.get("features", [])
            if isinstance(http_dependency, dict)
            else []
        )
        enables_openapi = "openapi" in enabled_features
        if enables_openapi != should_enable:
            expected = "enable" if should_enable else "not enable"
            errors.append(f"{crate} must {expected} ryframe-http/openapi")


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


def rust_function_body(source: str, function_name: str) -> str | None:
    """提取简单 Rust 函数体，供源码边界守卫使用。"""
    signature = re.search(
        rf"(?m)^\s*(?:pub\s+)?async\s+fn\s+{re.escape(function_name)}\b",
        source,
    )
    if signature is None:
        return None
    body_start = source.find("{", signature.end())
    if body_start < 0:
        return None

    depth = 0
    for index in range(body_start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[body_start : index + 1]
    return None


def readiness_handler_performs_network_io(source: str, function_name: str) -> bool:
    """识别就绪请求路径中禁止出现的直接依赖探测。"""
    body = rust_function_body(source, function_name)
    if body is None:
        return False
    return any(
        fragment in body
        for fragment in (".ping(", "connection::ping(", "check_storage(")
    )


def check_readiness_snapshot_boundary(errors: list[str]) -> None:
    """强制 API 与 Worker 的 readyz 只读后台生成的内存快照。"""
    targets = (
        ("crates/ryframe-api/src/probes.rs", "readyz", "state.monitor.readiness.snapshot()"),
        (
            "crates/ryframe/src/bin/ryframe_worker.rs",
            "worker_readyz",
            "state.readiness.snapshot()",
        ),
    )
    for relative_path, function_name, snapshot_fragment in targets:
        path = ROOT / relative_path
        source = path.read_text(encoding="utf-8")
        body = rust_function_body(source, function_name)
        if body is None:
            errors.append(f"readiness handler is missing: {relative_path}:{function_name}")
            continue
        if snapshot_fragment not in body:
            errors.append(
                f"readiness handler does not read the shared snapshot: "
                f"{relative_path}:{function_name}"
            )
        if readiness_handler_performs_network_io(source, function_name):
            errors.append(
                f"readiness handler performs direct network I/O: "
                f"{relative_path}:{function_name}"
            )

    background_source = (
        ROOT / "crates/ryframe/src/boot/readiness.rs"
    ).read_text(encoding="utf-8")
    for fragment in ("database.ping()", "redis.ping()", "file_service.check_storage()"):
        if fragment not in background_source:
            errors.append(f"background readiness probe is missing: {fragment}")

    main_source = (ROOT / "crates/ryframe/src/main.rs").read_text(encoding="utf-8")
    if "boot::readiness::spawn(" not in main_source:
        errors.append("API process does not start the background readiness probe")
    worker_source = (
        ROOT / "crates/ryframe/src/bin/ryframe_worker.rs"
    ).read_text(encoding="utf-8")
    if "process_readiness::spawn(" not in worker_source:
        errors.append("worker process does not start the background readiness probe")


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
            "pub fn select_read(&self, consistency: ReadConsistency) -> SelectedDatabase",
            "pub fn source(&self, name: &str) -> Option<&DatabaseConnection>",
            "pub fn with_sources_and_replica_slots(",
            "pub fn record_replica_probe(",
            "consecutive_failures: self.consecutive_failures.load(Ordering::Acquire)",
            "consecutive_successes: self.consecutive_successes.load(Ordering::Acquire)",
            "consecutive_failures: 0",
            "consecutive_successes: 0",
            "fetch_add(1, Ordering::Relaxed)",
        ),
        "crates/ryframe-core/src/database_monitor.rs": (
            "pub consecutive_failures: usize",
            "pub consecutive_successes: usize",
        ),
        "crates/ryframe-api/src/router.rs": (
            "consecutive_failures: replica.consecutive_failures",
            "consecutive_successes: replica.consecutive_successes",
            "consecutive_failures: usize",
            "consecutive_successes: usize",
        ),
        "crates/ryframe/src/boot/datasource.rs": (
            "config.database.primary",
            "config.database.replicas",
            "config.database.sources",
            "DatabaseCluster::with_sources_and_replica_slots(",
            "verify_schema",
            "ryframe_db_migration::verify(db)",
            "spawn_replica_health_monitor",
        ),
        "crates/ryframe-config/src/object_storage_config.rs": (
            "Rustfs",
            'Self::Rustfs => "rustfs"',
        ),
        "crates/ryframe/src/boot/storage.rs": (
            "StorageBackend::Rustfs",
            "storage.ensure_bucket(bucket).await",
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

    cluster_path = ROOT / "crates/ryframe-db/src/cluster.rs"
    cluster_source = cluster_path.read_text(encoding="utf-8")
    cluster_impl_source = cluster_source.split("impl DatabaseCluster {", maxsplit=1)[1]
    legacy_constructors = (
        r"pub\s+fn\s+new\s*\(",
        r"pub\s+fn\s+with_sources\s*\(",
        r"pub\s+fn\s+with_sources_and_replica_health\s*\(",
    )
    for legacy_constructor in legacy_constructors:
        if re.search(legacy_constructor, cluster_impl_source):
            errors.append(
                "database cluster restores a removed constructor: "
                f"{legacy_constructor}"
            )
    for legacy_signature in ("pub fn read(&self)", "pub fn read_strong(&self)"):
        if legacy_signature in cluster_source:
            errors.append(
                "database cluster restores a removed read helper in "
                f"{cluster_path.relative_to(ROOT)}: {legacy_signature}"
            )

    legacy_read_call = re.compile(
        r"\b(?:self\.)?(?:db|database|cluster)\s*\.\s*"
        r"(?:read|read_strong)\s*\(\s*\)"
    )
    for path in rust_sources("crates"):
        if legacy_read_call.search(path.read_text(encoding="utf-8")):
            errors.append(
                "database read must declare ReadConsistency through select_read: "
                f"{path.relative_to(ROOT)}"
            )


def check_embedded_swagger_ui(errors: list[str]) -> None:
    """确保 Swagger UI 只使用本地嵌入资源和限定范围的 CSP。"""
    required_fragments = {
        "Cargo.toml": (
            'utoipa-swagger-ui = { version = "9.0.2", default-features = false, features = ["vendored"] }',
        ),
        "crates/ryframe-api/Cargo.toml": (
            "utoipa-swagger-ui = { workspace = true }",
        ),
        "crates/ryframe-api/src/router.rs": (
            '.route("/swagger-ui", get_route(swagger_ui_index))',
            '.route("/swagger-ui/{*asset}", get_route(swagger_ui_asset))',
            "serve as serve_swagger_ui",
            'validator_url("none")',
            "fn swagger_ui_base_element() -> String",
            'format!("<base href=\\\"{}/swagger-ui/\\\">", API_PREFIX)',
            'SwaggerUiConfig::from(api_path("api-docs/openapi.json"))',
        ),
        "crates/ryframe-middleware/src/security_headers.rs": (
            "script-src 'self';",
            "style-src 'self' 'unsafe-inline';",
            "request_path.strip_prefix(ryframe_http::API_PREFIX)",
            '== Some("/swagger-ui")',
        ),
        "crates/ryframe-config/src/app_config.rs": (
            "self.api_docs.enabled",
            "production requires api_docs.enabled = false",
        ),
    }
    for relative_path, fragments in required_fragments.items():
        source = (ROOT / relative_path).read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"embedded Swagger UI contract is missing in {relative_path}: {fragment}"
                )

    router_path = "crates/ryframe-api/src/router.rs"
    router_source = (ROOT / router_path).read_text(encoding="utf-8")
    router_production = router_source.split("#[cfg(test)]", maxsplit=1)[0]
    for fragment in ("cdn.jsdelivr.net", "http://", "https://", "SwaggerUIBundle({", "Redirect"):
        if fragment in router_production:
            errors.append(
                f"Swagger UI router contains a removed external or redirect surface in "
                f"{router_path}: {fragment}"
            )

    security_path = "crates/ryframe-middleware/src/security_headers.rs"
    security_source = (ROOT / security_path).read_text(encoding="utf-8")
    for fragment in ("unsafe-eval", "https:"):
        if fragment in security_source:
            errors.append(
                f"security headers contain a removed CSP source in {security_path}: {fragment}"
            )


def check_api_prefix_contract(errors: list[str]) -> None:
    """确保公开 API 前缀只有一个后端来源并生成到前端契约。"""
    http_path = "crates/ryframe-http/src/lib.rs"
    http_source = (ROOT / http_path).read_text(encoding="utf-8")
    if len(re.findall(r'pub const API_PREFIX: &str = "/api/v1";', http_source)) != 1:
        errors.append(f"canonical API prefix must be defined exactly once in {http_path}")
    for fragment in (
        "pub fn api_path(relative_path: &str) -> String",
        "format!(\"{API_PREFIX}/{relative_path}\")",
    ):
        if fragment not in http_source:
            errors.append(f"canonical API path helper is missing in {http_path}: {fragment}")

    required_fragments = {
        "crates/ryframe/src/app.rs": (
            "ryframe_api::VersionedRouter::new()",
            ".with_v1(ryframe_api::api_router(",
        ),
        "crates/ryframe-api/src/versioning.rs": (
            "use ryframe_http::API_PREFIX;",
            "API_PREFIX.to_owned()",
        ),
        "crates/ryframe-api/src/router.rs": (
            "use ryframe_http::{API_PREFIX, ApiResponse, HttpAppError, HttpResult, api_path};",
            "api_prefix: API_PREFIX.to_owned()",
            'openapi: api_path("api-docs/openapi.json")',
            'swagger: api_path("swagger-ui")',
        ),
        "crates/ryframe-api/src/openapi.rs": (
            '"value": ryframe_http::API_PREFIX',
            'insert("x-ryframe-api-prefix".into(), api_prefix_contract())',
            ".strip_prefix(ryframe_http::API_PREFIX)",
        ),
        "crates/ryframe-api/src/handlers/auth_handler.rs": (
            '.path(api_path("auth"))',
            'format!("POST {}", api_path("auth/login"))',
        ),
        "crates/ryframe-middleware/src/body_limit.rs": (
            "use ryframe_http::{API_PREFIX, ApiResponse};",
            "path.strip_prefix(API_PREFIX)",
        ),
        "crates/ryframe-middleware/src/timeout.rs": (
            "use ryframe_http::{API_PREFIX, ApiResponse};",
            "path.strip_prefix(API_PREFIX)",
        ),
    }
    for relative_path, fragments in required_fragments.items():
        source = (ROOT / relative_path).read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"canonical API prefix wiring is missing in {relative_path}: {fragment}"
                )

    for path in rust_sources("crates/ryframe-service/src"):
        if re.search(r'(?:r#*)?"/api/v1(?:/|\")', path.read_text(encoding="utf-8")):
            errors.append(
                "service layer must not construct versioned HTTP URLs: "
                f"{path.relative_to(ROOT)}"
            )

    for relative_path in (
        "crates/ryframe-middleware/src/body_limit.rs",
        "crates/ryframe-middleware/src/timeout.rs",
        "crates/ryframe-middleware/src/security_headers.rs",
    ):
        production_source = (ROOT / relative_path).read_text(encoding="utf-8").split(
            "#[cfg(test)]", maxsplit=1
        )[0]
        if re.search(r'(?:r#*)?"/api/v1(?:/|\")', production_source):
            errors.append(
                f"middleware runtime hard-codes the canonical API prefix: {relative_path}"
            )

    for path in rust_sources("crates"):
        if re.search(r"\bAPI_V1_PREFIX\b", path.read_text(encoding="utf-8")):
            errors.append(
                f"removed API prefix compatibility alias returned: {path.relative_to(ROOT)}"
            )


def response_envelope_policy_violations(
    response_source: str, app_source: str
) -> list[str]:
    """校验统一响应只缓冲有界 JSON，并保留文档与协议升级流。"""
    production = response_source.split("#[cfg(test)]", maxsplit=1)[0]
    violations: list[str] = []
    required_fragments = {
        "canonical API prefix": "use ryframe_http::{API_PREFIX, api_path};",
        "finite JSON response limit": (
            "const API_JSON_RESPONSE_LIMIT_BYTES: usize = 16 * 1024 * 1024;"
        ),
        "bounded response buffering": (
            "to_bytes(body, API_JSON_RESPONSE_LIMIT_BYTES).await"
        ),
        "successful contract document bypass": (
            "status.is_success() && bypass_contract_document"
        ),
        "OpenAPI document bypass path": (
            'path == api_path("api-docs/openapi.json")'
        ),
        "Swagger document bypass path": 'strip_prefix(&api_path("swagger-ui"))',
        "protocol upgrade bypass": "status == StatusCode::SWITCHING_PROTOCOLS",
        "response parts preservation helper": "fn error_response_from_parts(",
        "response parts reuse": "Response::from_parts(parts, Body::from(body))",
        "injected response localizer": "State(localizer): State<Arc<Localizer>>",
        "localized response message": "localizer.translate(locale, message_key)",
        "canonical response language header": "ensure_locale_headers(response.headers_mut(), locale)",
    }
    for label, fragment in required_fragments.items():
        if fragment not in production:
            violations.append(f"response envelope is missing {label}")

    if re.search(
        r"to_bytes\s*\(\s*body\s*,\s*usize::MAX\s*\)", production
    ):
        violations.append("response envelope uses unbounded production body buffering")

    if not re.search(
        r"\.layer\(from_fn_with_state\(\s*response_localizer,\s*"
        r"ryframe_middleware::api_response_envelope_middleware,?\s*\)\)",
        app_source,
    ):
        violations.append("application does not mount the API response envelope middleware")
    return violations


def check_response_envelope_boundary(errors: list[str]) -> None:
    response_path = ROOT / "crates/ryframe-middleware/src/response_envelope.rs"
    app_path = ROOT / "crates/ryframe/src/app.rs"
    errors.extend(
        response_envelope_policy_violations(
            response_path.read_text(encoding="utf-8"),
            app_path.read_text(encoding="utf-8"),
        )
    )
    response_sources = {
        "crates/ryframe-http/src/lib.rs": (
            ROOT / "crates/ryframe-http/src/lib.rs"
        ).read_text(encoding="utf-8").split("#[cfg(test)]", maxsplit=1)[0],
        "crates/ryframe-generator/src/template/handler.rs": (
            ROOT / "crates/ryframe-generator/src/template/handler.rs"
        ).read_text(encoding="utf-8"),
    }
    response_sources.update(
        {
            str(path.relative_to(ROOT)): path.read_text(encoding="utf-8")
            for path in rust_sources("crates/ryframe-api/src")
        }
    )
    for relative_path, source in response_sources.items():
        for removed_constructor in (
            "success_msg(",
            "success_no_data_with_msg(",
            "ApiPageResponse::new(",
        ):
            if removed_constructor in source:
                errors.append(
                    f"removed response message constructor returned in {relative_path}: "
                    f"{removed_constructor}"
                )


def check_release_artifacts(errors: list[str]) -> None:
    workflow_path = ".github/workflows/release.yml"
    workflow = (ROOT / workflow_path).read_text(encoding="utf-8")
    required_fragments = (
        "name: Publish source-only GitHub release",
        "--backend-repository",
        "--backend-commit",
        "--frontend-repository",
        "--frontend-commit",
        "ref: ${{ needs.validate-release.outputs.backend_commit }}",
        "ref: ${{ needs.validate-release.outputs.frontend_commit }}",
        "name: Purge custom assets from target release",
        "releases/assets/${asset_id}",
        "python scripts/validate_release.py",
        "frontend/CHANGELOG.md",
        "body_path: release_body.md",
        "name: Verify published notes and zero custom assets",
        "(.assets | length == 0)",
        ".zipball_url",
        ".tarball_url",
        "backend_tag_oid:",
        "frontend_tag_oid:",
        "name: Revalidate tag objects",
        "name: Confirm tag refs immediately before publishing",
    )
    forbidden_fragments = (
        "RYFRAME_PRODUCTION_API_BASE_URL",
        "git archive",
        "gh release upload",
        "release-assets/",
        "SHA256SUMS",
        ".cdx.json",
        "\n          body:",
        "generate_release_notes:",
        "git tag -f nightly",
        "release-manifest.json",
        "publish-oci:",
        "stable-approval:",
        "environment:\n      name: stable-release",
        "docker/build-push-action",
        "docker/login-action",
        "docker/setup-qemu-action",
        "docker/setup-buildx-action",
        "anchore/sbom-action",
        "sigstore/cosign-installer",
        "cosign sign",
        "cosign attest",
        "ghcr.io/",
        "packages: write",
        "id-token: write",
        "actions/upload-artifact",
        "actions/download-artifact",
    )

    for fragment in required_fragments:
        if fragment not in workflow:
            errors.append(
                f"release artifact contract is missing in {workflow_path}: {fragment}"
            )
    for action in ("softprops/action-gh-release",):
        if not has_pinned_action(workflow, action):
            errors.append(
                f"release artifact contract is missing a pinned action in "
                f"{workflow_path}: {action}"
            )
    for fragment in forbidden_fragments:
        if fragment in workflow:
            errors.append(
                f"release artifact contract forbids in {workflow_path}: {fragment}"
            )

    # 稳定版工作流是唯一发布入口，禁止重新引入移动标签或预发布工作流。
    for path in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
        source = path.read_text(encoding="utf-8")
        relative_path = path.relative_to(ROOT)
        if "nightly" in path.name.lower():
            errors.append(f"stable-only release forbids Nightly workflow: {relative_path}")
        if path.name != "release.yml" and any(
            fragment in source
            for fragment in (
                "softprops/action-gh-release",
                "prerelease: true",
                "refs/tags/nightly",
                "tag_name: nightly",
            )
        ):
            errors.append(
                f"stable-only release forbids another publishing workflow: {relative_path}"
            )

    dockerfile_path = "deploy/Dockerfile"
    dockerfile = (ROOT / dockerfile_path).read_text(encoding="utf-8")
    for fragment in (
        "ARG RYFRAME_BUILD_COMMIT",
        'RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}"',
        'org.opencontainers.image.revision="${RYFRAME_BUILD_COMMIT}"',
    ):
        if fragment not in dockerfile:
            errors.append(
                f"release image build identity is missing in {dockerfile_path}: {fragment}"
            )
    if "ARG RYFRAME_BUILD_COMMIT=" in dockerfile:
        errors.append(
            f"release image build identity must be explicit in {dockerfile_path}"
        )

    production_compose_path = "deploy/compose.prod.yml"
    production_compose = (ROOT / production_compose_path).read_text(encoding="utf-8")
    for fragment in (
        "migrate:",
        "api:",
        "worker:",
        "image: ${RYFRAME_IMAGE:?",
        "APP_DATABASE_MIGRATION_MODE: verify",
        "APP_JOBS_MODE: external",
        "SNOWFLAKE_WORKER_ID:",
        "read_only: true",
        "internal: true",
        "entrypoint: [\"/usr/local/bin/ryframe-migrate\"]",
        "entrypoint: [\"/usr/local/bin/ryframe-worker\"]",
        "APP_OBJECT_STORAGE_USE_SSL: \"true\"",
    ):
        if fragment not in production_compose:
            errors.append(
                f"production Compose contract is missing in {production_compose_path}: {fragment}"
            )

    production_environment_path = "deploy/.env.production.example"
    production_environment = (ROOT / production_environment_path).read_text(encoding="utf-8")
    for fragment in ("ghcr.io/", "release-manifest"):
        if fragment in production_environment:
            errors.append(
                f"source-only deployment example contains removed release input in "
                f"{production_environment_path}: {fragment}"
            )
    if not re.search(
        r"(?m)^RYFRAME_IMAGE=(?!ghcr\.io/)[^\s]+@sha256:[^\s]+$",
        production_environment,
    ):
        errors.append(
            f"source-only deployment example must use a deployment-built image digest in "
            f"{production_environment_path}"
        )


def check_release_governance(errors: list[str]) -> None:
    release_path = ROOT / ".github/workflows/release.yml"
    release = release_path.read_text(encoding="utf-8")
    for fragment in (
        "Existing coordinated stable tag to validate and publish",
        "prerelease: false",
        "name: Publish source-only GitHub release",
        "name: Verify published notes and zero custom assets",
        "FRONTEND_REPOSITORY:",
        "${{ vars.RYFRAME_FRONTEND_REPOSITORY",
    ):
        if fragment not in release:
            errors.append(
                f"stable release governance is missing in .github/workflows/release.yml: {fragment}"
            )
    forbidden_patterns = (
        r"\bRC\b",
        r"release-candidate",
        r"minimum-rc-hours",
        r"prerelease:\s*true",
        r"stable-approval:",
        r"publish-oci:",
        r"packages:\s*write",
        r"id-token:\s*write",
    )
    for pattern in forbidden_patterns:
        if re.search(pattern, release):
            errors.append(
                f"stable release workflow must not contain prerelease policy: {pattern}"
            )

    ci_path = ROOT / ".github/workflows/ci.yml"
    ci = ci_path.read_text(encoding="utf-8")
    if not re.search(
        r"docker://rhysd/actionlint@sha256:[0-9a-f]{64}\b", ci
    ):
        errors.append(
            "CI must run the pinned actionlint workflow validator"
        )
    clippy_command = "cargo clippy --locked --workspace --lib --bins -- -D warnings"
    if ci.count(clippy_command) != 1:
        errors.append(
            "CI must compile production targets exactly once through the Clippy gate"
        )
    check_job = ci.split("\n  check:\n", maxsplit=1)
    security_job = ci.split("\n  security-audit:\n", maxsplit=1)
    if (
        len(check_job) != 2
        or "\n  security-audit:\n" not in check_job[1]
        or "    if: ${{ github.event_name != 'schedule' }}"
        not in check_job[1].split("\n  security-audit:\n", maxsplit=1)[0]
    ):
        errors.append("scheduled CI must skip the compilation and static-check job")
    if "\n  schedule:\n" not in ci or len(security_job) != 2:
        errors.append("scheduled CI must retain the dependency security job")
    elif "\n    if:" in security_job[1].split("\n    steps:\n", maxsplit=1)[0]:
        errors.append("dependency security job must remain unconditional for weekly schedules")
    inactive_rkyv_guard = (
        "cargo tree --locked --workspace --all-features --target all --invert rkyv@0.7.46"
    )
    strict_cargo_audit = "run: cargo audit --deny warnings"
    audit_config = (ROOT / ".cargo/audit.toml").read_text(encoding="utf-8")
    if ci.count(inactive_rkyv_guard) != 1:
        errors.append("CI 必须确认被忽略的 rkyv 版本仍未进入实际构建图")
    if ci.count(strict_cargo_audit) != 1 or "--ignore RUSTSEC-2026-0235" in ci:
        errors.append("CI 必须通过集中审计配置执行严格 cargo audit")
    if audit_config.count('"RUSTSEC-2026-0235"') != 1:
        errors.append("rkyv advisory 例外必须唯一记录在 .cargo/audit.toml")
    if "RUSTDOCFLAGS" in ci:
        errors.append("CI must not configure rustdoc flags without a documentation gate")
    forbidden_test_commands = {
        r"\bcargo\s+test\b": "cargo test",
        r"\bcargo\s+nextest\b": "cargo nextest",
        r"\bcargo\s+llvm-cov\b": "cargo llvm-cov",
        r"\b(?:python|python3)\s+-m\s+unittest\b": "Python unittest",
        r"\bpytest\b": "pytest",
        r"\b(?:node\s+--test|pnpm\s+test)\b": "JavaScript test runner",
        r"(?:^|\s)(?:bash|node|pwsh|powershell(?:\.exe)?)\s+(?:deploy/tests/|scripts/(?:runtime_acceptance|file_a_acceptance))": "local acceptance test script",
        r"\b(?:scripts/tests|deploy/tests|docker-compose\.test\.yml)\b": "test-only path",
    }
    for pattern, label in forbidden_test_commands.items():
        if re.search(pattern, ci):
            errors.append(f"CI must not run {label}")
    for path in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
        source = path.read_text(encoding="utf-8")
        if "auto-promote.yml" in source:
            errors.append(
                f"workflow references removed auto-promotion flow: {path.relative_to(ROOT)}"
            )


def check_validated_pagination_boundary(errors: list[str]) -> None:
    """保证原始分页参数只能在 API 边界转换为已校验值对象。"""
    core_path = ROOT / "crates/ryframe-core/src/repository.rs"
    core_source = core_path.read_text(encoding="utf-8")
    required_core_fragments = (
        "pub struct ValidatedPageQuery",
        "page: u64",
        "page_size: u64",
        "offset: u64",
        "pub fn from_optional(",
        "pub fn new(",
        "checked_mul(page_size)",
        "pub const fn page(&self)",
        "pub const fn page_size(&self)",
        "pub const fn offset(&self)",
    )
    for fragment in required_core_fragments:
        if fragment not in core_source:
            errors.append(f"validated pagination contract is missing: {fragment}")

    struct_match = re.search(
        r"pub struct ValidatedPageQuery\s*\{(?P<body>.*?)\n\}",
        core_source,
        re.DOTALL,
    )
    if struct_match is None:
        errors.append("validated pagination value object definition is missing")
    else:
        body = struct_match.group("body")
        if re.search(r"\bpub(?:\([^)]*\))?\s+(?:page|page_size|offset)\s*:", body):
            errors.append("validated pagination fields must remain private")

    if re.search(
        r"#\[derive\([^\]]*\b(?:Deserialize|Default)\b[^\]]*\)\]\s*"
        r"(?:#\[[^\]]+\]\s*)*pub struct ValidatedPageQuery",
        core_source,
    ):
        errors.append("validated pagination must not derive Deserialize or Default")
    if "impl Default for ValidatedPageQuery" in core_source:
        errors.append("validated pagination must not implement Default")

    old_type = re.compile(r"\bPageQuery\b")
    guarded_paths = [
        *sorted((ROOT / "crates").glob("**/*.rs")),
        *sorted((ROOT / "docs").glob("**/*.md")),
    ]
    for path in guarded_paths:
        source = path.read_text(encoding="utf-8")
        if old_type.search(source):
            errors.append(
                f"removed unvalidated pagination type returned: {path.relative_to(ROOT)}"
            )
        if "ValidatedPageQuery::default()" in source:
            errors.append(
                f"validated pagination bypasses runtime policy: {path.relative_to(ROOT)}"
            )
        if (
            path.suffix == ".rs"
            and path != core_path
            and re.search(
                r"(?:=|&|\(|,)\s*\bValidatedPageQuery\s*\{",
                source,
            )
        ):
            errors.append(
                f"validated pagination uses a field literal: {path.relative_to(ROOT)}"
            )

    for relative_dir in (
        "crates/ryframe-service/src",
        "crates/ryframe-db/src/repositories",
    ):
        for path in rust_sources(relative_dir):
            source = path.read_text(encoding="utf-8")
            if re.search(r"\bValidatedPageQuery::(?:new|from_optional)\s*\(", source):
                errors.append(
                    "service or repository constructs pagination instead of receiving the "
                    f"validated API value: {path.relative_to(ROOT)}"
                )
            for function_name, parameter_name, parameter_type in raw_pagination_parameters(
                source
            ):
                errors.append(
                    "service or repository accepts a raw pagination parameter instead of "
                    "ValidatedPageQuery: "
                    f"{path.relative_to(ROOT)}::{function_name} "
                    f"({parameter_name}: {parameter_type})"
                )

    api_macro = (ROOT / "crates/ryframe-api/src/macros.rs").read_text(encoding="utf-8")
    if "ValidatedPageQuery::from_optional(" not in api_macro:
        errors.append("API list query macro does not validate raw pagination parameters")


def raw_pagination_parameters(source: str) -> list[tuple[str, str, str]]:
    """提取 Service 或 Repository 函数签名中的原始分页数值参数。"""
    function_pattern = re.compile(
        r"\b(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+"
        r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*"
        r"(?:<[^;{}]*?>\s*)?\((?P<parameters>[^)]*)\)",
        re.DOTALL,
    )
    parameter_pattern = re.compile(
        r"(?:^|,)\s*(?P<name>page|page_size|offset)\s*:\s*"
        r"(?P<type>u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize)\b"
    )
    violations: list[tuple[str, str, str]] = []

    for function_match in function_pattern.finditer(source):
        for parameter_match in parameter_pattern.finditer(
            function_match.group("parameters")
        ):
            violations.append(
                (
                    function_match.group("name"),
                    parameter_match.group("name"),
                    parameter_match.group("type"),
                )
            )

    return violations


def has_pinned_action(source: str, action: str) -> bool:
    """检查指定的远程 Action 是否使用不可变提交引用。"""
    return re.search(
        rf"{re.escape(action)}@[0-9a-f]{{7,40}}\b", source
    ) is not None


def check_messaging_runtime_policy(errors: list[str]) -> None:
    """确保消息中心容量策略只来自强类型配置，并保持数据库侧收件人快照。"""
    config_source = (
        ROOT / "crates/ryframe-config/src/messaging_config.rs"
    ).read_text(encoding="utf-8")
    app_config_source = (
        ROOT / "crates/ryframe-config/src/app_config.rs"
    ).read_text(encoding="utf-8")
    service_source = (
        ROOT / "crates/ryframe-service/src/system/message_service.rs"
    ).read_text(encoding="utf-8")
    ticket_source = (
        ROOT / "crates/ryframe-service/src/system/websocket_ticket_service.rs"
    ).read_text(encoding="utf-8")
    socket_source = (
        ROOT / "crates/ryframe-api/src/message_socket.rs"
    ).read_text(encoding="utf-8")
    metrics_source = (
        ROOT / "crates/ryframe-middleware/src/metrics.rs"
    ).read_text(encoding="utf-8")
    repository_source = (
        ROOT / "crates/ryframe-db/src/repositories/message_repo.rs"
    ).read_text(encoding="utf-8")

    for field in (
        "enabled",
        "ticket_ttl_seconds",
        "retention_days",
        "max_connections_per_user",
        "outbound_buffer",
        "max_recipients_per_message",
        "replay_interval_seconds",
        "replay_jitter_seconds",
        "replay_batch_size",
    ):
        if not re.search(rf"\bpub\s+{field}\s*:", config_source):
            errors.append(f"typed messaging configuration is missing field: {field}")

    forbidden_constants = (
        "WEBSOCKET_TICKET_TTL_SECONDS",
        "DEFAULT_RETENTION_DAYS",
        "CONNECTION_QUEUE_CAPACITY",
        "RESYNC_INTERVAL",
        "RESYNC_BATCH_SIZE",
    )
    for name in forbidden_constants:
        if any(name in source for source in (service_source, ticket_source, socket_source)):
            errors.append(f"messaging runtime restores a hard-coded policy constant: {name}")

    if "self.messaging.enabled" not in app_config_source or "RedisMode::Required" not in app_config_source:
        errors.append("production messaging does not explicitly require Redis required mode")
    if "config: MessagingConfig" not in service_source or "self.ensure_enabled()?" not in service_source:
        errors.append("message service does not receive and enforce MessagingConfig")
    if "config: MessagingConfig" not in ticket_source or "self.config.ticket_ttl_seconds" not in ticket_source:
        errors.append("WebSocket ticket service does not use configured ticket TTL")
    if "max_connections_per_user" not in socket_source or ".entry(identity.clone())" not in socket_source:
        errors.append("message hub does not atomically enforce the per-identity connection limit")
    if "spawn_replay_scheduler" not in socket_source or "online_identities" not in socket_source:
        errors.append("message inbox replay is not owned by a shared identity scheduler")
    if "let mut resync" in socket_source or socket_source.count("unacknowledged_for_identity") != 1:
        errors.append("message inbox replay performs per-connection duplicate queries")
    if 'record_message_replay_query("success")' not in socket_source:
        errors.append("shared message replay queries are not observable with bounded metrics")
    if (
        '"message_redis_listener_connected"' not in metrics_source
        or socket_source.count("set_message_redis_listener_connected(true)") != 1
        or socket_source.count("set_message_redis_listener_connected(false)") < 3
    ):
        errors.append("message Redis listener state is not observable with a bounded gauge")

    publish_start = repository_source.find("pub async fn publish_in_transaction")
    publish_end = repository_source.find("pub async fn inbox", publish_start)
    publish_source = repository_source[publish_start:publish_end]
    if ".select_from(recipient_select)" not in repository_source:
        errors.append("message recipient snapshot is not written with INSERT SELECT")
    if "max_recipients.saturating_add(1)" not in repository_source:
        errors.append("message recipient overflow detection must use max + 1")
    if "resolve_recipients" in repository_source or "insert_many(recipient_models)" in publish_source:
        errors.append("message publishing loads recipient IDs into Rust before snapshot insertion")


def check_logging_retention_policy(errors: list[str]) -> None:
    """确保日志输出强类型、文件保留有界且容器默认使用 stdout。"""
    config_source = (
        ROOT / "crates/ryframe-config/src/logger_config.rs"
    ).read_text(encoding="utf-8")
    override_source = (
        ROOT / "crates/ryframe-config/src/app_config/environment_overrides/spec.rs"
    ).read_text(encoding="utf-8")
    logging_source = (
        ROOT / "crates/ryframe/src/boot/logging.rs"
    ).read_text(encoding="utf-8")
    worker_source = (
        ROOT / "crates/ryframe/src/bin/ryframe_worker.rs"
    ).read_text(encoding="utf-8")
    production_config = (ROOT / "config/app.prod.toml").read_text(encoding="utf-8")
    production_compose = (ROOT / "deploy/compose.prod.yml").read_text(encoding="utf-8")

    for fragment in (
        "pub enum LoggerLevel",
        "pub enum LoggerFormat",
        "pub enum LoggerOutput",
        "pub level: LoggerLevel",
        "pub format: LoggerFormat",
        "pub output: LoggerOutput",
        "pub retention_days: usize",
        "(1..=3_650).contains(&self.retention_days)",
    ):
        if fragment not in config_source:
            errors.append(f"typed logger policy is missing: {fragment}")

    for variable in (
        "APP_LOGGER_LEVEL",
        "APP_LOGGER_FORMAT",
        "APP_LOGGER_OUTPUT",
        "APP_LOGGER_RETENTION_DAYS",
    ):
        if variable not in override_source:
            errors.append(f"logger environment override is missing: {variable}")

    for fragment in (
        "prepare_file_appender(",
        ".rotation(Rotation::DAILY)",
        ".max_log_files(retention_days)",
        'Path::new("logs")',
        "LoggerOutput::Stdout => Ok(None)",
        "LoggerOutput::File => build_file_appender(directory, retention_days).map(Some)",
    ):
        if fragment not in logging_source:
            errors.append(f"bounded file logging policy is missing: {fragment}")
    if "tracing_appender::rolling::daily" in logging_source:
        errors.append("logging restores the unbounded daily appender helper")

    for fragment in (
        '#[path = "../boot/logging.rs"]',
        "process_logging::init(&config)?",
    ):
        if fragment not in worker_source:
            errors.append(f"worker does not share API logging initialization: {fragment}")
    if re.search(r"\bfn\s+init_logging\s*\(", worker_source):
        errors.append("worker restores its duplicate logging initializer")

    if not re.search(r'(?m)^output\s*=\s*"stdout"\s*$', production_config):
        errors.append("production configuration must default logger output to stdout")
    if "APP_LOGGER_OUTPUT: ${APP_LOGGER_OUTPUT:-stdout}" not in production_compose:
        errors.append("production Compose must default logger output to stdout")


def file_runtime_policy_violations(source: str) -> list[str]:
    """识别正常运行时代码中被禁止的旧文件摘要和预留标记。"""
    patterns = {
        "md5 implementation": r"\bmd5\s*::",
        "legacy MD5 variable": r"\blegacy_md5\b",
        "legacy MD5 entity access": r"\b(?:file_md5|FileMd5)\b",
        "dual digest lookup": r"\bfind_by_digests(?:_any_status_in_txn)?\b",
        "legacy upload reservation constant": r"\bDEL_FLAG_UPLOAD_RESERVED\b",
        "legacy upload reservation literal": (
            r"(?is)\b(?:del_flag|DelFlag)\b.{0,80}?[=.(]\s*[\"']3[\"']"
        ),
    }
    return [name for name, pattern in patterns.items() if re.search(pattern, source)]


def check_file_digest_runtime_policy(errors: list[str]) -> None:
    """保证 API/Worker 只使用 SHA-256 与 upload_status，旧输入仅进入维护命令。"""
    maintenance_path = ROOT / "crates/ryframe/src/bin/ryframe_file_maintenance.rs"
    migration_root = ROOT / "crates/ryframe-db-migration"
    for path in production_rust_sources():
        if path == maintenance_path or migration_root in path.parents:
            continue
        source = path.read_text(encoding="utf-8")
        for violation in file_runtime_policy_violations(source):
            errors.append(
                f"normal runtime restores {violation}: {path.relative_to(ROOT)}"
            )

    for manifest_path in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        if manifest_path == ROOT / "crates/ryframe/Cargo.toml":
            continue
        if re.search(
            r"(?m)^\s*md5\s*=", manifest_path.read_text(encoding="utf-8")
        ):
            errors.append(
                "normal runtime crate depends on MD5: "
                f"{manifest_path.relative_to(ROOT)}"
            )

    ryframe_manifest = (ROOT / "crates/ryframe/Cargo.toml").read_text(
        encoding="utf-8"
    )
    for fragment in (
        'md5 = { version = "0.8", optional = true }',
        'file-maintenance = ["dep:md5", "dep:sha2", "dep:hex", "dep:chrono"]',
        'name = "ryframe-file-maintenance"',
        'required-features = ["file-maintenance"]',
    ):
        if fragment not in ryframe_manifest:
            errors.append(f"one-time file maintenance isolation is missing: {fragment}")

    if not maintenance_path.is_file():
        errors.append("one-time file maintenance command is missing")
        return
    maintenance_source = maintenance_path.read_text(encoding="utf-8")
    for fragment in (
        "backfill_sha256(",
        "drain_legacy_reservations(",
        "APPLY_CONFIRMATION",
        "normalize_legacy_md5(",
        "file_md5",
        "LEGACY_RESERVED_FLAG",
    ):
        if fragment not in maintenance_source:
            errors.append(f"one-time file maintenance command is incomplete: {fragment}")


def check_persisted_trace_context(errors: list[str]) -> None:
    """保证后台任务和 Outbox 持久化完整的 W3C Trace Context。"""
    required_fragments = {
        "crates/ryframe-service/src/trace_context.rs": (
            "struct PersistedTraceContext",
            "pub traceparent: Option<String>",
            "pub tracestate: Option<String>",
            "fn current_trace_context()",
            "tracestate: Option<&str>",
        ),
        "crates/ryframe-db/src/entities/background_job.rs": (
            "pub traceparent: Option<String>",
            "pub tracestate: Option<String>",
        ),
        "crates/ryframe-db/src/entities/outbox_event.rs": (
            "pub traceparent: Option<String>",
            "pub tracestate: Option<String>",
        ),
        "crates/ryframe-db/src/repositories/background_job_repo.rs": (
            "traceparent: Set(command.traceparent)",
            "tracestate: Set(command.tracestate)",
        ),
        "crates/ryframe-db/src/repositories/outbox_event_repo.rs": (
            "traceparent: Set(event.traceparent)",
            "tracestate: Set(event.tracestate)",
        ),
        "crates/ryframe-db-migration/src/m20260805_000019_trace_context_state.rs": (
            'const TRACE_STATE_COLUMN: &str = "tracestate"',
            ".string_len(512)",
        ),
        "crates/ryframe-db-migration/src/schema.rs": (
            "`traceparent` VARCHAR(255)",
            "`tracestate` VARCHAR(512)",
        ),
        "sql/ryframe_config.sql": (
            "`traceparent`",
            "`tracestate`",
        ),
    }
    for relative_path, fragments in required_fragments.items():
        source = (ROOT / relative_path).read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"persisted trace context is incomplete in {relative_path}: {fragment}"
                )

    for path in rust_sources("crates/ryframe-service/src"):
        if "current_traceparent(" in path.read_text(encoding="utf-8"):
            errors.append(
                "service restores traceparent-only persistence: "
                f"{path.relative_to(ROOT)}"
            )


def check_message_time_precision(errors: list[str]) -> None:
    """保证消息写入、筛选与数据库时钟使用一致的微秒精度。"""
    required_fragments = {
        "crates/ryframe-db-migration/src/m20260522_000000_mysql_baseline.rs": (
            "`published_at` DATETIME(6)",
            "`read_at` DATETIME(6)",
        ),
        "crates/ryframe-db-migration/src/m20260726_000009_message_center.rs": (
            "`published_at` DATETIME(6)",
            "`read_at` DATETIME(6)",
        ),
        "crates/ryframe-db-migration/src/m20260805_000020_message_time_precision.rs": (
            "ALTER_MESSAGE_TIME_PRECISION_SQL",
            "ALTER_RECIPIENT_TIME_PRECISION_SQL",
            "DATETIME(6)",
        ),
        "sql/ryframe_config.sql": (
            "`published_at` DATETIME(6)",
            "`read_at` DATETIME(6)",
        ),
    }
    for relative_path, fragments in required_fragments.items():
        source = (ROOT / relative_path).read_text(encoding="utf-8")
        for fragment in fragments:
            if fragment not in source:
                errors.append(
                    f"message time precision is incomplete in {relative_path}: {fragment}"
                )


def check_pinned_workflow_actions(errors: list[str]) -> None:
    """禁止工作流重新引入可变的第三方 Action 或容器镜像标签。"""
    uses_pattern = re.compile(r"^\s*(?:-\s+)?uses:\s+([^\s#]+)")
    commit_pattern = re.compile(r"^[0-9a-f]{7,40}$")
    image_digest_pattern = re.compile(r"^sha256:[0-9a-f]{64}$")

    for path in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = uses_pattern.match(line)
            if not match:
                continue
            reference = match.group(1)
            if reference.startswith("./"):
                continue
            if "@" not in reference:
                errors.append(
                    f"workflow Action is missing an immutable reference: "
                    f"{path.relative_to(ROOT)}:{line_number}"
                )
                continue
            action, revision = reference.rsplit("@", 1)
            if action.startswith("docker://"):
                if not image_digest_pattern.fullmatch(revision):
                    errors.append(
                        f"workflow container Action must use a sha256 digest: "
                        f"{path.relative_to(ROOT)}:{line_number}"
                    )
            elif not commit_pattern.fullmatch(revision):
                errors.append(
                    f"workflow Action must use a commit SHA: "
                    f"{path.relative_to(ROOT)}:{line_number}"
                )


def main() -> int:
    errors: list[str] = []
    check_dependency_graph(errors)
    check_feature_registry(errors)
    check_removed_common_crate(errors)
    check_secret_source_policy(errors)
    check_kernel_manifest(errors)
    check_unsigned_replay_contract(errors)
    check_removed_compatibility_surfaces(errors)
    check_removed_oper_log_job(errors)
    check_removed_repository_wrapper(errors)
    check_http_error_boundary(errors)
    check_source_boundaries(errors)
    check_public_dto_boundary(errors)
    check_readiness_snapshot_boundary(errors)
    check_validated_pagination_boundary(errors)
    check_messaging_runtime_policy(errors)
    check_logging_retention_policy(errors)
    check_file_digest_runtime_policy(errors)
    check_persisted_trace_context(errors)
    check_message_time_precision(errors)
    check_database_and_storage_topology(errors)
    check_api_prefix_contract(errors)
    check_response_envelope_boundary(errors)
    check_embedded_swagger_ui(errors)
    check_release_artifacts(errors)
    check_release_governance(errors)
    check_pinned_workflow_actions(errors)
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
