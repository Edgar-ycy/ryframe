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
//! ```text
//! use ryframe_middleware::telemetry::TelemetryConfig;
//!
//! // 配置链路追踪
//! let config = TelemetryConfig::default();
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
