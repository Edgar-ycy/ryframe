const MANIFEST: &str = include_str!("../Cargo.toml");
const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");
const ADAPTERS_MANIFEST: &str = include_str!("../../ryframe-adapters/Cargo.toml");
const APPLICATION_MANIFEST: &str = include_str!("../../ryframe-application/Cargo.toml");
const DB_MANIFEST: &str = include_str!("../../ryframe-db/Cargo.toml");
const OTLP_MANIFEST: &str = include_str!("../../../vendor/opentelemetry-otlp/Cargo.toml");

fn feature_members(feature: &str) -> &'static str {
    let marker = format!("{feature} = [");
    let (_, remainder) = MANIFEST
        .split_once(&marker)
        .unwrap_or_else(|| panic!("根清单缺少 {feature} feature"));
    remainder
        .split_once(']')
        .map(|(members, _)| members)
        .expect("feature 定义必须闭合")
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
    assert!(DB_MANIFEST.contains("telemetry = ["));
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
