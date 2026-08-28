use std::env;

use opentelemetry::trace::{Span, Tracer};
use ryframe_adapters::{metrics, telemetry::init_tracer_provider_with_http_client};
use ryframe_config::TelemetryConfig;

const ENABLE_ENV: &str = "RYFRAME_OTLP_HTTPS_INTEGRATION";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otlp_exporter_flushes_over_a_real_https_endpoint() {
    if !integration_enabled() {
        eprintln!("跳过 OTLP HTTPS 真实链路测试：设置 {ENABLE_ENV}=1 后才会访问显式端点");
        return;
    }
    tokio::task::yield_now().await;

    let before = metric_value(
        &metrics::metrics_text(),
        "ryframe_otel_exporter_runtime_failures_total",
    );
    let root_ca = env::var("RYFRAME_HTTPS_CA_PEM").ok().map(|path| {
        let pem = std::fs::read(path).expect("读取 OTLP HTTPS 测试 CA");
        reqwest::Certificate::from_pem(&pem).expect("解析 OTLP HTTPS 测试 CA")
    });
    let mut http_client = reqwest::Client::builder();
    if let Some(root_ca) = root_ca {
        http_client = http_client.add_root_certificate(root_ca);
    }
    let http_client = http_client.build().expect("创建 OTLP HTTPS 测试客户端");
    let endpoint = required_env("RYFRAME_OTLP_HTTPS_ENDPOINT");
    assert!(
        endpoint.starts_with("https://"),
        "OTLP HTTPS 测试端点不得降级为明文 HTTP"
    );
    let guard = init_tracer_provider_with_http_client(
        &TelemetryConfig {
            enabled: true,
            endpoint,
            service_name: "ryframe-aws-lc-https-test".to_owned(),
            sample_ratio: 1.0,
            export_timeout_secs: 15,
            max_queue_size: 16,
        },
        Some(http_client),
    );
    let tracer = guard
        .tracer
        .as_ref()
        .unwrap_or_else(|| panic!("OTLP HTTPS 导出器初始化失败"));
    let mut span = tracer.start("aws-lc-otlp-https");
    span.end();
    guard.shutdown();

    let after = metric_value(
        &metrics::metrics_text(),
        "ryframe_otel_exporter_runtime_failures_total",
    );
    assert_eq!(after, before, "OTLP HTTPS 导出或关闭发生运行期失败");
}

fn metric_value(metrics: &str, name: &str) -> f64 {
    metrics
        .lines()
        .find_map(|line| {
            let (metric, value) = line.split_once(' ')?;
            (metric == name).then(|| value.parse::<f64>().expect("解析 OTLP 指标值"))
        })
        .unwrap_or_else(|| panic!("未找到指标 {name}"))
}

fn integration_enabled() -> bool {
    match env::var(ENABLE_ENV) {
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("{ENABLE_ENV} 只接受精确值 1，当前值为 {value:?}"),
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => panic!("{ENABLE_ENV} 必须是有效 UTF-8"),
    }
}

fn required_env(name: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => panic!("{name} 不得为空"),
        Err(env::VarError::NotPresent) => panic!("启用测试时必须设置 {name}"),
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}
