#[path = "support/configuration.rs"]
mod configuration;
use configuration::{Configuration, ScopedClient};

use opentelemetry::trace::{Span, Tracer};
use ryframe_adapters::{metrics, telemetry::init_tracer_provider_with_http_client};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otlp_exporter_flushes_over_a_real_https_endpoint() {
    if !configuration::enabled("otlp-https") {
        return;
    }
    tokio::task::yield_now().await;

    let before = metric_value(
        &metrics::metrics_text(),
        "ryframe_otel_exporter_runtime_failures_total",
    );
    let snapshot = Configuration::load("otlp-https", "otlp-flush-https");
    let config = &snapshot.app.telemetry;
    assert!(
        config.enabled && config.endpoint.starts_with("https://"),
        "OTLP HTTPS 测试必须启用 telemetry 并使用 HTTPS"
    );
    let pem = snapshot.ca().unwrap_or_else(|error| panic!("{error}"));
    let root_ca = reqwest::Certificate::from_pem(&pem).expect("解析显式 OTLP HTTPS 测试 CA");
    let http_client = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(root_ca)
        .timeout(std::time::Duration::from_secs(config.export_timeout_secs))
        .build()
        .expect("创建 OTLP HTTPS 测试客户端");
    let guard = init_tracer_provider_with_http_client(config, Some(http_client));
    let guard = ScopedClient {
        client: guard,
        configuration: snapshot,
    };
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
