const MANIFEST: &str = include_str!("../Cargo.toml");
const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");
const API_MANIFEST: &str = include_str!("../../ryframe-api/Cargo.toml");
const ADAPTERS_MANIFEST: &str = include_str!("../../ryframe-adapters/Cargo.toml");
const APPLICATION_MANIFEST: &str = include_str!("../../ryframe-application/Cargo.toml");
const DB_MANIFEST: &str = include_str!("../../ryframe-db/Cargo.toml");
const TENANT_DB_MANIFEST: &str = include_str!("../../ryframe-tenant-db/Cargo.toml");
const OTLP_MANIFEST: &str = include_str!("../../../vendor/opentelemetry-otlp/Cargo.toml");

fn manifest_feature_members<'a>(manifest: &'a str, feature: &str) -> &'a str {
    let marker = format!("{feature} = [");
    let (_, remainder) = manifest
        .split_once(&marker)
        .unwrap_or_else(|| panic!("清单缺少 {feature} feature"));
    remainder
        .split_once(']')
        .map(|(members, _)| members)
        .expect("feature 定义必须闭合")
}

fn feature_members(feature: &str) -> &'static str {
    manifest_feature_members(MANIFEST, feature)
}

#[test]
fn every_process_binary_has_an_explicit_required_feature() {
    for feature in [
        "bin-api",
        "bin-worker",
        "bin-migrate",
        "bin-tenant-data",
        "bin-file-maintenance",
        "bin-reset",
        "runtime-swagger-ui",
    ] {
        assert!(
            MANIFEST.contains(&format!("{feature} =")),
            "根清单缺少 {feature} feature"
        );
    }
    for required in [
        "required-features = [\"bin-api\"]",
        "required-features = [\"bin-worker\"]",
        "required-features = [\"bin-migrate\"]",
        "required-features = [\"bin-tenant-data\"]",
        "required-features = [\"bin-file-maintenance\"]",
        "required-features = [\"bin-reset\"]",
    ] {
        assert!(MANIFEST.contains(required), "二进制缺少 {required}");
    }
    assert!(MANIFEST.contains("default = [\"bin-api\", \"runtime-swagger-ui\"]"));
}

#[test]
fn every_client_process_installs_crypto_before_loading_configuration() {
    for (name, source, first_client_boundary) in [
        (
            "api",
            include_str!("../src/main.rs"),
            "Environment::from_env",
        ),
        (
            "worker",
            include_str!("../src/bin/ryframe_worker.rs"),
            "Environment::from_env",
        ),
        (
            "migrate",
            include_str!("../src/bin/ryframe_migrate.rs"),
            "Environment::from_env",
        ),
        (
            "tenant-data",
            include_str!("../src/bin/ryframe_tenant_data.rs"),
            "Environment::from_env",
        ),
        (
            "file-maintenance",
            include_str!("../src/bin/ryframe_file_maintenance.rs"),
            "Environment::from_required_env",
        ),
        (
            "reset",
            include_str!("../src/bin/ryframe_reset.rs"),
            "ryframe::reset::run",
        ),
    ] {
        let install = source
            .find("ryframe::crypto::install_crypto_provider()")
            .unwrap_or_else(|| panic!("{name} 未安装密码学 provider"));
        let boundary = source
            .find(first_client_boundary)
            .unwrap_or_else(|| panic!("{name} 缺少预期客户端边界"));
        assert!(install < boundary, "{name} 在客户端初始化后才安装 provider");
    }
}

#[test]
fn worker_source_has_no_api_crate_dependency() {
    assert!(!include_str!("../src/bin/ryframe_worker.rs").contains("ryframe_api"));
    assert!(!include_str!("../src/bin/ryframe_worker/health.rs").contains("ryframe_api"));
}

#[test]
fn worker_http_probe_does_not_enable_api_websocket_features() {
    assert!(WORKSPACE_MANIFEST.contains("axum = { version = \"0.8\", default-features = false }"));
    assert!(WORKSPACE_MANIFEST.contains(
        "axum-extra = { version = \"0.12.6\", default-features = false, features = [\"cookie\"] }"
    ));
    assert!(MANIFEST.contains(
        "axum = { workspace = true, optional = true, features = [\"http1\", \"tokio\"] }"
    ));
    let api_axum = API_MANIFEST
        .split_once("axum = { workspace = true, features = [")
        .and_then(|(_, declaration)| declaration.split_once("] }"))
        .map(|(features, _)| features)
        .expect("API 必须显式声明 Axum feature");
    for feature in [
        "http1",
        "json",
        "matched-path",
        "multipart",
        "original-uri",
        "query",
        "tokio",
        "tracing",
        "ws",
    ] {
        assert!(
            api_axum.contains(&format!("\"{feature}\"")),
            "API 缺少 Axum feature {feature}"
        );
    }
    assert!(!api_axum.contains("\"form\""));
    assert!(!api_axum.contains("\"tower-log\""));
}

#[test]
fn development_profile_limits_dependency_debug_info() {
    let manifest = WORKSPACE_MANIFEST.replace("\r\n", "\n");
    assert!(manifest.contains("[profile.dev]\n# "));
    assert!(manifest.contains("\ndebug = 1\n"));
    assert!(manifest.contains("[profile.dev.package.\"*\"]\ndebug = 0"));
    assert!(manifest.contains("[profile.dev.build-override]\ndebug = 0"));
}

#[test]
fn expensive_leaf_capabilities_are_opt_in() {
    assert!(ADAPTERS_MANIFEST.contains("default = []"));
    for feature in [
        "image-processing",
        "monitoring",
        "otel",
        "redis",
        "spreadsheet",
    ] {
        assert!(
            ADAPTERS_MANIFEST.contains(&format!("{feature} =")),
            "adapters 缺少 {feature} feature"
        );
    }
    let shared = feature_members("runtime-services");
    let api = feature_members("bin-api");
    let worker = feature_members("bin-worker");
    assert!(!shared.contains("ryframe-adapters/image-processing"));
    assert!(!shared.contains("ryframe-adapters/spreadsheet"));
    assert!(api.contains("ryframe-adapters/image-processing"));
    assert!(api.contains("ryframe-adapters/spreadsheet"));
    assert!(!worker.contains("ryframe-adapters/image-processing"));
    assert!(worker.contains("ryframe-adapters/spreadsheet"));
    assert!(APPLICATION_MANIFEST.contains("default = []"));
    assert!(APPLICATION_MANIFEST.contains("test-support = []"));
    assert!(DB_MANIFEST.contains("default = []"));
    assert!(DB_MANIFEST.contains("connection = ["));
    assert!(DB_MANIFEST.contains("migration = ["));
    assert!(DB_MANIFEST.contains("repositories = ["));
    assert!(DB_MANIFEST.contains("telemetry = ["));
    assert!(TENANT_DB_MANIFEST.contains("default = []"));
    assert!(TENANT_DB_MANIFEST.contains("connection = ["));
    assert!(TENANT_DB_MANIFEST.contains("migration = ["));
    assert!(TENANT_DB_MANIFEST.contains("repositories = ["));
}

#[test]
fn process_features_select_precise_database_surfaces() {
    let db_connection = manifest_feature_members(DB_MANIFEST, "connection");
    assert!(db_connection.contains("dep:sea-orm"));
    assert!(!db_connection.contains("sea-orm-migration"));
    assert!(!db_connection.contains("ryframe-application"));
    assert!(!db_connection.contains("ryframe-macro"));
    let db_migration = manifest_feature_members(DB_MANIFEST, "migration");
    assert!(db_migration.contains("connection"));
    assert!(db_migration.contains("dep:sea-orm-migration"));
    assert!(db_migration.contains("dep:async-trait"));
    assert!(!db_migration.contains("ryframe-application"));
    assert!(!db_migration.contains("ryframe-macro"));
    assert!(!db_migration.contains("repositories"));
    let db_repositories = manifest_feature_members(DB_MANIFEST, "repositories");
    assert!(db_repositories.contains("connection"));
    assert!(!db_repositories.contains("migration"));
    assert!(!db_repositories.contains("sea-orm-migration"));
    assert!(db_repositories.contains("ryframe-application"));
    assert!(db_repositories.contains("ryframe-macro"));

    let tenant_connection = manifest_feature_members(TENANT_DB_MANIFEST, "connection");
    assert!(tenant_connection.contains("ryframe-db/connection"));
    assert!(!tenant_connection.contains("sea-orm-migration"));
    assert!(!tenant_connection.contains("ryframe-application"));
    let tenant_migration = manifest_feature_members(TENANT_DB_MANIFEST, "migration");
    assert!(tenant_migration.contains("connection"));
    assert!(tenant_migration.contains("dep:sea-orm-migration"));
    assert!(tenant_migration.contains("dep:async-trait"));
    assert!(!tenant_migration.contains("ryframe-db/migration"));
    assert!(!tenant_migration.contains("ryframe-application"));
    assert!(!tenant_migration.contains("repositories"));
    let tenant_repositories = manifest_feature_members(TENANT_DB_MANIFEST, "repositories");
    assert!(tenant_repositories.contains("connection"));
    assert!(!tenant_repositories.contains("migration"));
    assert!(!tenant_repositories.contains("sea-orm-migration"));
    assert!(tenant_repositories.contains("ryframe-db/repositories"));
    assert!(tenant_repositories.contains("ryframe-application"));

    let database = feature_members("runtime-database");
    assert!(database.contains("ryframe-db/connection"));
    assert!(database.contains("ryframe-tenant-db/connection"));
    assert!(!database.contains("ryframe-db/migration"));
    assert!(!database.contains("ryframe-tenant-db/migration"));
    assert!(!database.contains("repositories"));

    let services = feature_members("runtime-services");
    assert!(services.contains("ryframe-db/repositories"));
    assert!(services.contains("ryframe-tenant-db/repositories"));

    let migrate = feature_members("bin-migrate");
    assert!(migrate.contains("runtime-database"));
    assert!(migrate.contains("ryframe-db/migration"));
    assert!(migrate.contains("ryframe-tenant-db/migration"));
    assert!(!migrate.contains("repositories"));

    let tenant_data = feature_members("bin-tenant-data");
    assert!(tenant_data.contains("ryframe-db/repositories"));
    assert!(tenant_data.contains("ryframe-tenant-db/repositories"));

    let maintenance = feature_members("bin-file-maintenance");
    assert!(maintenance.contains("ryframe-db/repositories"));

    let reset = feature_members("bin-reset");
    assert!(!reset.contains("ryframe-db/repositories"));
    assert!(reset.contains("ryframe-db/migration"));
    assert!(reset.contains("ryframe-tenant-db/migration"));
}

#[test]
fn otlp_http_exporter_keeps_only_the_trace_signal() {
    assert!(
        WORKSPACE_MANIFEST
            .contains("opentelemetry-otlp = { path = \"vendor/opentelemetry-otlp\" }")
    );
    assert!(WORKSPACE_MANIFEST.contains(
        "opentelemetry-otlp = { version = \"0.32\", default-features = false, features = [\"trace\", \"http-proto\", \"reqwest-client\"] }"
    ));
    let http_proto = OTLP_MANIFEST
        .split_once("http-proto = [")
        .and_then(|(_, features)| features.split_once("]"))
        .map(|(features, _)| features)
        .expect("本地 OTLP 补丁必须保留 http-proto feature");
    assert!(http_proto.contains("\"trace\""));
    assert!(!http_proto.contains("\"metrics\""));
    assert!(!http_proto.contains("\"logs\""));
    assert!(!WORKSPACE_MANIFEST.contains("reqwest-blocking-client"));
}
