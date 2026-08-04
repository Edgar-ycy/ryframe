//! OpenTelemetry 链路追踪
//!
//! 集成 OpenTelemetry + tracing 生态：
//! - 自动 Span 创建（HTTP 请求/响应）
//! - OTLP HTTP 导出（支持 Jaeger、Tempo、Datadog 等）
//! - 采样策略配置
//! - 优雅关闭
//!
//! # 使用示例
//!
//! ```
//! use ryframe_middleware::telemetry::TelemetryConfig;
//!
//! // 配置链路追踪
//! let config = TelemetryConfig::default();
//! assert!(!config.enabled);
//! assert_eq!(config.service_name, "ryframe");
//!
//! ```

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use axum::{
    extract::{MatchedPath, Request},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use opentelemetry::{KeyValue, global, propagation::Extractor, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{
        BatchConfigBuilder, BatchSpanProcessor, RandomIdGenerator, Sampler, SdkTracer,
        SdkTracerProvider, SpanData, SpanExporter,
    },
};
use tracing::{Instrument, error, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{Layer, filter::FilterFn};

use crate::{
    metrics::{
        record_otel_exporter_failure, record_otel_exporter_runtime_failure,
        set_otel_exporter_degraded,
    },
    request_log::REQUEST_LOG_SPAN_TARGET,
};

// ============ 配置 ============

pub use ryframe_config::TelemetryConfig;

const BUILD_COMMIT: &str = env!("RYFRAME_BUILD_COMMIT");
const TELEMETRY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// 仅从 HTTP 请求头读取 W3C 传播字段的适配器。
struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

/// 链路追踪守卫
///
/// 持有 `SdkTracerProvider` 和 `SdkTracer`。
/// 通过 `tracing_layer()` 获取可注册到 tracing subscriber 的 Layer。
/// 离开作用域时自动 flush + shutdown。
#[must_use = "必须持有此守卫，否则 Trace 数据会在程序退出前丢失"]
pub struct TelemetryGuard {
    pub tracer_provider: Option<SdkTracerProvider>,
    pub tracer: Option<SdkTracer>,
    shutdown_timeout: Duration,
    shutdown_called: AtomicBool,
}

impl TelemetryGuard {
    /// 获取 tracing → OpenTelemetry 桥接 Layer
    ///
    /// 将此 layer 注册到 tracing_subscriber 中，所有 `tracing` Span 将自动导出为 OTel Span。
    ///
    /// 返回 `None` 表示链路追踪未启用。
    pub fn tracing_layer<S>(&self) -> Option<impl Layer<S>>
    where
        S: tracing::Subscriber
            + for<'span> tracing_subscriber::registry::LookupSpan<'span>
            + 'static,
    {
        self.tracer.as_ref().map(|tracer| {
            // 只把 Span 导出到 OTLP：日志事件（尤其 sqlx 事件）可能包含未脱敏的
            // 查询文本；请求日志 Span 也保留在本地日志，避免身份字段外发。
            let filter = FilterFn::new(|metadata| {
                metadata.is_span() && metadata.target() != REQUEST_LOG_SPAN_TARGET
            });
            tracing_opentelemetry::layer()
                .with_tracer(tracer.clone())
                .with_filter(filter)
        })
    }

    /// 主动关闭并在导出器无法完成 flush 时记录运行期失败。
    ///
    /// 此方法可安全重复调用；守卫析构时会自动调用，正常退出路径可提前调用以
    /// 便于控制关闭顺序。
    pub fn shutdown(&self) {
        let Some(provider) = self.tracer_provider.as_ref() else {
            return;
        };
        if self
            .shutdown_called
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if provider
            .shutdown_with_timeout(self.shutdown_timeout)
            .is_err()
        {
            record_otel_exporter_runtime_failure();
            warn!(
                failure_stage = "shutdown",
                "OTLP 导出器关闭时未能在时限内完成 flush"
            );
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============ 初始化 ============

/// 初始化 OpenTelemetry TracerProvider 并设为全局
///
/// 返回 TelemetryGuard，必须在程序运行期间保持存活。
/// 通过 `guard.tracing_layer()` 获取 Layer 注册到 subscriber。
pub fn init_tracer_provider(config: &TelemetryConfig) -> TelemetryGuard {
    if !config.enabled {
        set_otel_exporter_degraded(false);
        info!("链路追踪: 未启用");
        return TelemetryGuard {
            tracer_provider: None,
            tracer: None,
            shutdown_timeout: TELEMETRY_SHUTDOWN_TIMEOUT,
            shutdown_called: AtomicBool::new(false),
        };
    }

    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(&config.endpoint)
        .with_timeout(Duration::from_secs(config.export_timeout_secs))
        .build()
    {
        Ok(exporter) => exporter,
        Err(_) => {
            record_otel_exporter_failure();
            warn!(
                failure_stage = "initialization",
                "OTLP 导出器创建失败，链路追踪降级为禁用"
            );
            return TelemetryGuard {
                tracer_provider: None,
                tracer: None,
                shutdown_timeout: TELEMETRY_SHUTDOWN_TIMEOUT,
                shutdown_called: AtomicBool::new(false),
            };
        }
    };

    let guard = build_telemetry_guard(config, exporter, TELEMETRY_SHUTDOWN_TIMEOUT);
    let tracer_provider = guard
        .tracer_provider
        .as_ref()
        .expect("已启用链路追踪时必须存在 TracerProvider");
    global::set_tracer_provider(tracer_provider.clone());
    set_otel_exporter_degraded(false);

    info!(
        service_name = %config.service_name,
        sample_ratio = config.sample_ratio,
        "链路追踪已启用"
    );

    guard
}

fn build_telemetry_guard<E>(
    config: &TelemetryConfig,
    exporter: E,
    shutdown_timeout: Duration,
) -> TelemetryGuard
where
    E: SpanExporter + Send + 'static,
{
    let batch_config = BatchConfigBuilder::default()
        .with_max_queue_size(config.max_queue_size)
        .with_max_export_batch_size(config.max_queue_size.min(512))
        .build();
    let span_processor = BatchSpanProcessor::builder(FailureCountingSpanExporter::new(exporter))
        .with_batch_config(batch_config)
        .build();
    let tracer_provider = SdkTracerProvider::builder()
        .with_span_processor(span_processor)
        .with_sampler(Sampler::TraceIdRatioBased(config.sample_ratio))
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(telemetry_resource(config.service_name.clone()))
        .build();

    let tracer = tracer_provider.tracer(config.service_name.clone());

    TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        tracer: Some(tracer),
        shutdown_timeout,
        shutdown_called: AtomicBool::new(false),
    }
}

/// 为 OTLP exporter 统一记录运行期失败，避免导出线程的错误只停留在内部日志。
#[derive(Debug)]
struct FailureCountingSpanExporter<E> {
    inner: E,
}

impl<E> FailureCountingSpanExporter<E> {
    fn new(inner: E) -> Self {
        Self { inner }
    }
}

impl<E> SpanExporter for FailureCountingSpanExporter<E>
where
    E: SpanExporter,
{
    async fn export(&self, batch: Vec<SpanData>) -> opentelemetry_sdk::error::OTelSdkResult {
        let result = self.inner.export(batch).await;
        if result.is_err() {
            record_otel_exporter_runtime_failure();
            warn!(failure_stage = "export", "OTLP 导出失败，业务请求继续执行");
        }
        result
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

fn telemetry_resource(service_name: String) -> Resource {
    Resource::builder()
        .with_attributes(vec![
            KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_name,
            ),
            KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
                env!("CARGO_PKG_VERSION"),
            ),
            KeyValue::new("vcs.ref.head.revision", BUILD_COMMIT),
        ])
        .build()
}

// ============ HTTP Span 中间件 ============

/// HTTP 请求 Span 中间件
///
/// 为每个 HTTP 请求自动创建 OpenTelemetry Span，记录：
/// - HTTP 方法 / 路由 / 状态码（status_code）
/// - 由 span 生命周期自动计算的请求耗时
/// - 慢请求告警（>1s）
/// - 客户端错误记录（4xx）和服务端错误告警（5xx）
///
/// **必须放在 request_id 中间件之后**，以便 Span 中包含请求上下文。
pub async fn telemetry_middleware(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| "/unmatched".to_owned());
    let parent_context = global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(request.headers()))
    });

    let span = tracing::info_span!(
        "HTTP",
        http.method = %method,
        http.route = %path,
        http.status_code = tracing::field::Empty,
    );

    // 将当前 OTel Context 设为父 Context（实现跨服务追踪链）
    let _ = span.set_parent(parent_context);

    let start = std::time::Instant::now();
    let response = next.run(request).instrument(span.clone()).await;
    let elapsed = start.elapsed();

    // 记录响应状态
    let status = response.status().as_u16();
    span.record("http.status_code", status.to_string());

    match classify_http_response(status) {
        HttpResponseClass::ClientError => info!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            "HTTP 客户端错误响应"
        ),
        HttpResponseClass::ServerError => error!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            "HTTP 服务端错误响应"
        ),
        HttpResponseClass::Success => {}
    }

    if elapsed.as_millis() > 1000 {
        warn!(
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            "慢请求"
        );
    }

    response
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpResponseClass {
    Success,
    ClientError,
    ServerError,
}

const fn classify_http_response(status: u16) -> HttpResponseClass {
    match status {
        500.. => HttpResponseClass::ServerError,
        400..=499 => HttpResponseClass::ClientError,
        _ => HttpResponseClass::Success,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BUILD_COMMIT, FailureCountingSpanExporter, HeaderExtractor, HttpResponseClass, SpanData,
        SpanExporter, TELEMETRY_SHUTDOWN_TIMEOUT, TelemetryConfig, build_telemetry_guard,
        classify_http_response, telemetry_middleware, telemetry_resource,
    };
    use axum::{
        Router,
        body::Body,
        http::{HeaderMap, HeaderValue, Request, StatusCode},
        middleware::from_fn,
        routing::get,
    };
    use opentelemetry::{Key, global, trace::TraceContextExt};
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use std::{
        io::Write,
        net::TcpListener,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use tower::ServiceExt;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    #[test]
    fn response_statuses_use_operational_log_severity() {
        assert_eq!(classify_http_response(200), HttpResponseClass::Success);
        assert_eq!(classify_http_response(401), HttpResponseClass::ClientError);
        assert_eq!(classify_http_response(403), HttpResponseClass::ClientError);
        assert_eq!(classify_http_response(500), HttpResponseClass::ServerError);
        assert_eq!(classify_http_response(503), HttpResponseClass::ServerError);
    }

    #[test]
    fn build_commit_is_development_or_a_full_sha() {
        assert!(
            BUILD_COMMIT == "development"
                || (BUILD_COMMIT.len() == 40
                    && BUILD_COMMIT.bytes().all(|byte| byte.is_ascii_hexdigit()))
        );
    }

    #[test]
    fn resource_contains_service_version_and_build_revision() {
        let resource = telemetry_resource("ryframe-test".to_owned());
        assert_eq!(
            resource
                .get(&Key::new(
                    opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                ))
                .map(|value| value.to_string()),
            Some("ryframe-test".to_owned())
        );
        assert_eq!(
            resource
                .get(&Key::new("vcs.ref.head.revision",))
                .map(|value| value.to_string()),
            Some(BUILD_COMMIT.to_owned())
        );
    }

    #[test]
    fn header_extractor_restores_w3c_traceparent_and_tracestate() {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        headers.insert("tracestate", HeaderValue::from_static("vendor=value"));

        let context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(&headers))
        });
        let span = context.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid());
        assert!(span_context.is_remote());
        assert_eq!(
            span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(span_context.trace_state().header(), "vendor=value");
    }

    #[derive(Debug)]
    struct FailingExporter;

    impl SpanExporter for FailingExporter {
        async fn export(&self, _batch: Vec<SpanData>) -> OTelSdkResult {
            Err(OTelSdkError::InternalFailure("测试导出失败".to_owned()))
        }
    }

    #[test]
    fn invalid_otlp_endpoint_degrades_initialization_without_exposing_endpoint() {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer({
                let logs = logs.clone();
                move || logs.clone()
            })
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let endpoint = "invalid_uri/secret";
        let config = TelemetryConfig {
            enabled: true,
            endpoint: endpoint.to_owned(),
            ..TelemetryConfig::default()
        };
        let before = metric_counter("ryframe_otel_exporter_failures_total");

        let guard = super::init_tracer_provider(&config);

        assert!(guard.tracer_provider.is_none());
        assert!(guard.tracer.is_none());
        assert!(metric_counter("ryframe_otel_exporter_failures_total") >= before + 1.0);
        assert_eq!(metric_counter("ryframe_otel_exporter_degraded"), 1.0);
        let output = logs.text();
        assert!(output.contains("OTLP 导出器创建失败，链路追踪降级为禁用"));
        assert!(output.contains("failure_stage=\"initialization\""));
        assert!(!output.contains(endpoint));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exporter_runtime_failures_are_counted_and_logged_without_error_details() {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer({
                let logs = logs.clone();
                move || logs.clone()
            })
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let before = metric_counter("ryframe_otel_exporter_runtime_failures_total");
        let exporter = FailureCountingSpanExporter::new(FailingExporter);
        assert!(exporter.export(Vec::new()).await.is_err());
        assert!(metric_counter("ryframe_otel_exporter_runtime_failures_total") >= before + 1.0);

        let output = logs.text();
        assert!(output.contains("OTLP 导出失败，业务请求继续执行"));
        assert!(output.contains("failure_stage=\"export\""));
        assert!(!output.contains("测试导出失败"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unavailable_otlp_endpoint_does_not_affect_routes_and_shutdown_is_bounded() {
        // 保持监听但不接收请求，让本地 OTLP 端点稳定地在客户端超时，而不依赖外部服务。
        let listener = TcpListener::bind("127.0.0.1:0").expect("应能绑定本地测试端口");
        let endpoint = format!(
            "http://{}/v1/traces",
            listener.local_addr().expect("应能读取本地测试地址")
        );
        let config = TelemetryConfig {
            enabled: true,
            endpoint,
            service_name: "ryframe-otel-failure-test".to_owned(),
            sample_ratio: 1.0,
            export_timeout_secs: 1,
            max_queue_size: 8,
        };
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(&config.endpoint)
            .with_timeout(Duration::from_secs(config.export_timeout_secs))
            .build()
            .expect("有效的本地 HTTP 地址应能创建导出器");
        let guard = build_telemetry_guard(&config, exporter, TELEMETRY_SHUTDOWN_TIMEOUT);
        let tracer = guard.tracer.as_ref().expect("应创建 tracer").clone();
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
        let _subscriber_guard = subscriber.set_default();
        let app = Router::new()
            .route("/api/v1/ping", get(|| async { StatusCode::OK }))
            .route("/readyz", get(|| async { StatusCode::OK }))
            .layer(from_fn(telemetry_middleware));

        assert_route_status(&app, "/api/v1/ping", StatusCode::OK).await;
        assert_route_status(&app, "/readyz", StatusCode::OK).await;

        let before = metric_counter("ryframe_otel_exporter_runtime_failures_total");
        let flush_started = Instant::now();
        assert!(
            guard
                .tracer_provider
                .as_ref()
                .expect("应创建 TracerProvider")
                .force_flush()
                .is_err()
        );
        assert!(flush_started.elapsed() < TELEMETRY_SHUTDOWN_TIMEOUT);
        assert!(metric_counter("ryframe_otel_exporter_runtime_failures_total") >= before + 1.0);

        // 导出失败后再次访问，确认业务和就绪响应都保持不变。
        assert_route_status(&app, "/api/v1/ping", StatusCode::OK).await;
        assert_route_status(&app, "/readyz", StatusCode::OK).await;

        drop(listener);
        let shutdown_started = Instant::now();
        guard.shutdown();
        assert!(shutdown_started.elapsed() <= TELEMETRY_SHUTDOWN_TIMEOUT);
    }

    #[test]
    fn otel_failure_metrics_have_no_dynamic_labels() {
        crate::metrics::record_otel_exporter_failure();
        crate::metrics::record_otel_exporter_runtime_failure();
        let metrics = crate::metrics::metrics_text();
        let samples = metrics
            .lines()
            .filter(|line| line.starts_with("ryframe_otel_"))
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 3);
        assert!(samples.iter().all(|line| !line.contains('{')));
    }

    async fn assert_route_status(app: &Router, path: &str, expected: StatusCode) {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl LogBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for LogBuffer {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn metric_counter(metric_name: &str) -> f64 {
        crate::metrics::metrics_text()
            .lines()
            .find_map(|line| {
                line.strip_prefix(metric_name)
                    .and_then(|value| value.trim().parse::<f64>().ok())
            })
            .unwrap_or_default()
    }
}
