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

use std::{net::SocketAddr, time::Duration};

use axum::{extract::ConnectInfo, middleware::Next, response::Response};
use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{RandomIdGenerator, Sampler, SdkTracer, SdkTracerProvider},
};
use tracing::{error, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::Layer;

// ============ 配置 ============

/// 链路追踪配置
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// 是否启用链路追踪
    pub enabled: bool,
    /// OTLP 收集器地址（HTTP 协议，如 `http://localhost:4318/v1/traces`）
    pub endpoint: String,
    /// 服务名称
    pub service_name: String,
    /// 采样率（0.0 ~ 1.0，1.0 = 全部采样）
    pub sample_rate: f64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://localhost:4318/v1/traces".into(),
            service_name: "ryframe".into(),
            sample_rate: 1.0,
        }
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
        self.tracer
            .as_ref()
            .map(|tracer| tracing_opentelemetry::layer().with_tracer(tracer.clone()))
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(ref provider) = self.tracer_provider {
            let _ = provider.force_flush();
            let _ = provider.shutdown();
        }
    }
}

// ============ 初始化 ============

/// 初始化 OpenTelemetry TracerProvider 并设为全局
///
/// 返回 TelemetryGuard，必须在程序运行期间保持存活。
/// 通过 `guard.tracing_layer()` 获取 Layer 注册到 subscriber。
pub fn init_tracer_provider(config: &TelemetryConfig) -> TelemetryGuard {
    if !config.enabled {
        info!("链路追踪: 未启用");
        return TelemetryGuard {
            tracer_provider: None,
            tracer: None,
        };
    }

    let resource = Resource::builder()
        .with_attributes(vec![
            KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                config.service_name.clone(),
            ),
            KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
                env!("CARGO_PKG_VERSION"),
            ),
        ])
        .build();

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(&config.endpoint)
        .with_timeout(Duration::from_secs(5))
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, endpoint = %config.endpoint, "OTLP exporter 创建失败，链路追踪降级为禁用");
            return TelemetryGuard {
                tracer_provider: None,
                tracer: None,
            };
        }
    };

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(Sampler::TraceIdRatioBased(config.sample_rate))
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(resource)
        .build();

    // 获取 SdkTracer（Clone 的，持有 Arc<...>）
    let tracer = tracer_provider.tracer(config.service_name.clone());

    // 设为全局 tracer provider
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());

    info!(
        endpoint = %config.endpoint,
        service_name = %config.service_name,
        sample_rate = config.sample_rate,
        "链路追踪已启用"
    );

    TelemetryGuard {
        tracer_provider: Some(tracer_provider),
        tracer: Some(tracer),
    }
}

// ============ HTTP Span 中间件 ============

/// HTTP 请求 Span 中间件
///
/// 为每个 HTTP 请求自动创建 OpenTelemetry Span，记录：
/// - HTTP method / route / status_code
/// - client_ip / request_id
/// - user_id（从 JWT Claims 提取）
/// - content_length / response_size
/// - 请求耗时
/// - 慢请求告警（>1s）
/// - 客户端错误记录（4xx）和服务端错误告警（5xx）
///
/// **必须放在 request_id 中间件之后**，以便 Span 中包含请求上下文。
pub async fn telemetry_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let client_ip = addr.ip().to_string();

    // 获取 request_id（由 request_id_middleware 注入）
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    // 获取 content_length
    let content_length = request
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    // 尝试从 JWT Claims 提取 user_id
    let user_id = request
        .extensions()
        .get::<ryframe_auth::jwt::Claims>()
        .map(|c| c.sub.clone())
        .unwrap_or_else(|| "anonymous".to_string());

    let span = tracing::info_span!(
        "HTTP",
        http.method = %method,
        http.route = %path,
        http.client_ip = %client_ip,
        http.request_id = %request_id,
        http.user_id = %user_id,
        http.content_length = content_length,
    );

    // 将当前 OTel Context 设为父 Context（实现跨服务追踪链）
    let _ = span.set_parent(opentelemetry::Context::current());

    let _enter = span.enter();

    let start = std::time::Instant::now();
    let response = next.run(request).await;
    let elapsed = start.elapsed();

    // 记录响应状态
    let status = response.status().as_u16();
    span.record("http.status_code", status.to_string());
    span.record("http.duration_ms", elapsed.as_millis() as u64);

    // 记录响应体大小
    if let Some(size) = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        span.record("http.response_size", size);
    }

    match classify_http_response(status) {
        HttpResponseClass::ClientError => info!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            http.user_id = %user_id,
            "HTTP 客户端错误响应"
        ),
        HttpResponseClass::ServerError => error!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            http.user_id = %user_id,
            "HTTP 服务端错误响应"
        ),
        HttpResponseClass::Success => {}
    }

    if elapsed.as_millis() > 1000 {
        tracing::warn!(
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            http.user_id = %user_id,
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
    use super::{HttpResponseClass, classify_http_response};

    #[test]
    fn response_statuses_use_operational_log_severity() {
        assert_eq!(classify_http_response(200), HttpResponseClass::Success);
        assert_eq!(classify_http_response(401), HttpResponseClass::ClientError);
        assert_eq!(classify_http_response(403), HttpResponseClass::ClientError);
        assert_eq!(classify_http_response(500), HttpResponseClass::ServerError);
        assert_eq!(classify_http_response(503), HttpResponseClass::ServerError);
    }
}
