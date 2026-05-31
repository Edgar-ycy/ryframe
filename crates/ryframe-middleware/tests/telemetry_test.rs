/// telemetry 模块测试
/// 从 crates/ryframe-middleware/src/telemetry.rs 内联测试迁移
use ryframe_middleware::telemetry::{TelemetryConfig, child_span, init_tracer_provider};

#[test]
fn test_config_default() {
    let config = TelemetryConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.service_name, "ryframe");
    assert!((config.sample_rate - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_disabled_telemetry_returns_empty_guard() {
    let config = TelemetryConfig {
        enabled: false,
        ..Default::default()
    };
    let guard = init_tracer_provider(&config);
    assert!(guard.tracer_provider.is_none());
    assert!(guard.tracer.is_none());
}

#[test]
fn test_child_span_created() {
    let span = child_span("test.op", &[("key", String::from("val"))]);
    drop(span);
}
