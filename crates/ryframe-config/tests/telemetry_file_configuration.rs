use std::collections::BTreeMap;

use ryframe_config::{TelemetryConfig, TelemetrySpanLimits};

#[test]
fn omitted_fields_use_explicit_file_configuration_defaults() {
    let config: TelemetryConfig = toml::from_str("").expect("空配置应使用默认值");
    assert!(!config.enabled);
    assert_eq!(config.endpoint, "http://localhost:4318/v1/traces");
    assert_eq!(config.service_name, "ryframe");
    assert_eq!(config.sample_ratio, 0.1);
    assert_eq!(config.export_timeout_secs, 5);
    assert_eq!(config.max_queue_size, 2048);
    assert!(config.headers.is_empty());
    assert!(config.resource_attributes.is_empty());
    assert_eq!(config.scheduled_delay_millis, 5000);
    assert_eq!(config.batch_export_timeout_secs, 30);
    assert_eq!(config.max_concurrent_exports, 1);
    assert_eq!(config.max_export_batch_size, None);
    let limits = TelemetrySpanLimits {
        max_attributes_per_span: 128,
        max_events_per_span: 128,
        max_links_per_span: 128,
        max_attributes_per_event: 128,
        max_attributes_per_link: 128,
    };
    assert_eq!(config.span_limits, limits);
    assert_eq!(TelemetrySpanLimits::default(), limits);
    assert!(config.validate().is_ok());
    assert!(TelemetryConfig::default().validate().is_ok());
}

#[test]
fn checked_in_defaults_keep_following_sections_outside_span_limits() {
    let document: toml::Value =
        toml::from_str(include_str!("../../../config/app.toml")).expect("默认配置应为有效 TOML");
    let config: TelemetryConfig = document["telemetry"]
        .clone()
        .try_into()
        .expect("遥测段应只包含遥测字段");
    assert!(config.validate().is_ok());
    assert_eq!(config.scheduled_delay_millis, 5000);
    assert_eq!(config.batch_export_timeout_secs, 30);
    assert_eq!(config.max_concurrent_exports, 1);
    assert_eq!(config.max_export_batch_size, None);
    assert!(config.headers.is_empty());
    assert!(config.resource_attributes.is_empty());
    assert_eq!(config.span_limits, TelemetrySpanLimits::default());
    assert_eq!(document["messaging"]["enabled"].as_bool(), Some(true));
    assert!(
        document["telemetry"]["span_limits"]
            .get("enabled")
            .is_none()
    );
    assert!(document.get("object_storage").is_some());
}

#[test]
fn batch_configuration_accepts_limits_and_rejects_outside_values() {
    for (field, min, max) in [
        ("scheduled_delay_millis", 1, 600_000),
        ("batch_export_timeout_secs", 1, 60),
        ("max_concurrent_exports", 1, 64),
        ("max_export_batch_size", 1, 2048),
    ] {
        for value in [min, max] {
            let config: TelemetryConfig =
                toml::from_str(&format!("{field} = {value}")).expect("边界值应可解析");
            assert!(config.validate().is_ok(), "{field} 边界值应有效");
        }
        for value in [0, max + 1] {
            let config: TelemetryConfig =
                toml::from_str(&format!("{field} = {value}")).expect("越界值应进入校验");
            let error = config.validate().expect_err("越界值应拒绝");
            assert!(error.contains(&format!("telemetry.{field}")));
            assert!(error.contains("请设置"));
        }
    }
}

#[test]
fn batch_size_tracks_queue_capacity_without_requiring_an_override() {
    for queue in [1, 64, 512, 65_536] {
        let config = TelemetryConfig {
            max_queue_size: queue,
            ..TelemetryConfig::default()
        };
        assert!(config.validate().is_ok());
        let explicit = TelemetryConfig {
            max_export_batch_size: Some(queue),
            ..config.clone()
        };
        assert!(explicit.validate().is_ok());
        let overflow = TelemetryConfig {
            max_export_batch_size: Some(queue + 1),
            ..config
        };
        assert!(overflow.validate().is_err());
    }
}

#[test]
fn all_span_limits_accept_zero_and_maximum_and_reject_overflow() {
    for field in [
        "max_attributes_per_span",
        "max_events_per_span",
        "max_links_per_span",
        "max_attributes_per_event",
        "max_attributes_per_link",
    ] {
        for value in [0, 65_536, 65_537] {
            let config: TelemetryConfig =
                toml::from_str(&format!("[span_limits]\n{field} = {value}"))
                    .expect("容量值应进入校验");
            let result = config.validate();
            if value <= 65_536 {
                assert!(result.is_ok(), "{field} 应允许边界值");
            } else {
                assert!(
                    result
                        .expect_err("容量越界应拒绝")
                        .contains(&format!("telemetry.span_limits.{field}"))
                );
            }
        }
    }
}

#[test]
fn file_configuration_preserves_headers_attributes_and_partial_limits() {
    let config: TelemetryConfig = toml::from_str(
        r#"
enabled = true
endpoint = "https://collector.example/v1/traces"
max_export_batch_size = 17
headers = { Authorization = "Bearer file-token", "X-Scope" = "tenant-a" }
resource_attributes = { "deployment.environment.name" = "test", "region" = "华东" }
[span_limits]
max_events_per_span = 0
"#,
    )
    .expect("文件配置应可解析");
    assert!(config.validate().is_ok());
    assert_eq!(config.max_export_batch_size, Some(17));
    assert_eq!(config.headers["Authorization"], "Bearer file-token");
    assert_eq!(config.resource_attributes["region"], "华东");
    assert_eq!(config.span_limits.max_events_per_span, 0);
    assert_eq!(config.span_limits.max_attributes_per_event, 128);
}

#[test]
fn headers_accept_http_token_names_and_permitted_value_bytes() {
    for value in ["", "\t", "Bearer token", "obs-华东", " !~"] {
        let config = with_header("X-!#$%&'*+-.^_`|~09AZaz", value);
        assert!(config.validate().is_ok());
    }
}

#[test]
fn headers_reject_invalid_names_controls_and_case_duplicates() {
    for name in [
        "", " X-Token", "X Token", "X:Token", "X/Token", "中文", "X\r\n",
    ] {
        assert!(with_header(name, "secret").validate().is_err());
    }
    for byte in (0_u8..=31).chain(std::iter::once(127)) {
        if byte == b'\t' {
            continue;
        }
        assert!(
            with_header("X-Token", &char::from(byte).to_string())
                .validate()
                .is_err()
        );
    }
    let config = TelemetryConfig {
        headers: BTreeMap::from([
            ("Authorization".into(), "first".into()),
            ("authorization".into(), "second".into()),
        ]),
        ..TelemetryConfig::default()
    };
    assert!(
        config
            .validate()
            .expect_err("同名请求头应拒绝")
            .contains("重复")
    );
}

#[test]
fn exporter_managed_headers_cannot_be_overridden() {
    for name in [
        "Host",
        "Content-Length",
        "Content-Type",
        "Content-Encoding",
        "Transfer-Encoding",
        "Connection",
        "Keep-Alive",
        "Proxy-Connection",
        "TE",
        "Trailer",
        "Upgrade",
    ] {
        let error = with_header(name, "secret")
            .validate()
            .expect_err("运输层字段应拒绝");
        assert!(error.contains("telemetry.headers"));
        assert!(!error.contains("secret"));
    }
}

#[test]
fn resource_attributes_reject_invalid_keys_values_and_system_identity() {
    for (key, value) in [
        ("", "value"),
        ("   ", "value"),
        ("key\n", "value"),
        ("key\u{85}", "value"),
        ("key", "value\0"),
        ("key", "value\t"),
        ("key", "value\u{7f}"),
        ("service.name", "override"),
        ("service.version", "override"),
        ("vcs.ref.head.revision", "override"),
        ("telemetry.sdk.name", "override"),
        ("telemetry.sdk.version", "override"),
        ("telemetry.sdk.language", "override"),
        ("telemetry.sdk.custom", "override"),
    ] {
        let config = TelemetryConfig {
            resource_attributes: BTreeMap::from([(key.into(), value.into())]),
            ..TelemetryConfig::default()
        };
        let error = config.validate().expect_err("无效资源属性应拒绝");
        assert!(error.contains("telemetry.resource_attributes"));
        assert!(error.contains("请"));
    }
}

#[test]
fn debug_and_validation_errors_do_not_disclose_sensitive_inputs() {
    let config = TelemetryConfig {
        endpoint: "https://secret-user:secret-password@collector.example/path?secret-query".into(),
        headers: BTreeMap::from([("Authorization".into(), "secret-header-token".into())]),
        resource_attributes: BTreeMap::from([("custom".into(), "secret-attribute-value".into())]),
        ..TelemetryConfig::default()
    };
    for debug in [format!("{config:?}"), format!("{config:#?}")] {
        for secret in [
            "secret-user",
            "secret-password",
            "collector.example",
            "secret-query",
            "secret-header-token",
            "secret-attribute-value",
        ] {
            assert!(!debug.contains(secret), "Debug 不应包含敏感输入");
        }
        for field in ["endpoint", "headers", "resource_attributes", "[REDACTED]"] {
            assert!(debug.contains(field));
        }
    }
    let invalid = with_header("secret-invalid-name\n", "secret-value\r\n");
    let error = invalid.validate().expect_err("非法请求头应拒绝");
    assert!(!error.contains("secret-invalid-name"));
    assert!(!error.contains("secret-value"));
    let invalid = TelemetryConfig {
        resource_attributes: BTreeMap::from([("secret-key\n".into(), "secret-value".into())]),
        ..config
    };
    let error = invalid.validate().expect_err("非法属性应拒绝");
    assert!(!error.contains("secret-key"));
    assert!(!error.contains("secret-value"));
}

#[test]
fn existing_validation_boundaries_remain_enforced() {
    for input in [
        "service_name = ''",
        "service_name = '   '",
        "enabled = true\nendpoint = ''",
        "sample_ratio = -0.1",
        "sample_ratio = 1.1",
        "sample_ratio = nan",
        "sample_ratio = inf",
        "export_timeout_secs = 0",
        "export_timeout_secs = 61",
        "max_queue_size = 0",
        "max_queue_size = 65537",
    ] {
        let config: TelemetryConfig = toml::from_str(input).expect("旧字段应可解析");
        assert!(config.validate().is_err());
    }
    let config = TelemetryConfig {
        service_name: "x".repeat(129),
        ..TelemetryConfig::default()
    };
    assert!(config.validate().is_err());
    for ratio in [0.0, 1.0] {
        let config = TelemetryConfig {
            sample_ratio: ratio,
            export_timeout_secs: 60,
            max_queue_size: 65_536,
            ..TelemetryConfig::default()
        };
        assert!(config.validate().is_ok());
    }
    let disabled = TelemetryConfig {
        endpoint: String::new(),
        ..TelemetryConfig::default()
    };
    assert!(disabled.validate().is_ok());
}

#[test]
fn unknown_fields_and_negative_unsigned_limits_are_rejected() {
    for input in [
        "unknown = true",
        "scheduled_delay_millis = -1",
        "[span_limits]\nunknown = 1",
        "[span_limits]\nmax_links_per_span = -1",
    ] {
        assert!(toml::from_str::<TelemetryConfig>(input).is_err());
    }
}

fn with_header(name: &str, value: &str) -> TelemetryConfig {
    TelemetryConfig {
        headers: BTreeMap::from([(name.into(), value.into())]),
        ..TelemetryConfig::default()
    }
}
