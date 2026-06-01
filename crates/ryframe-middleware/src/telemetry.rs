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
//! // 创建 Span 工具函数（不依赖实际 OTLP 后端）
//! let span = ryframe_middleware::telemetry::child_span(
//!     "db.query_user",
//!     &[("db.user_id", "42".to_string())]
//! );
//! drop(span);
//! ```

use std::{net::SocketAddr, time::Duration};

use axum::{extract::ConnectInfo, middleware::Next, response::Response};
use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{RandomIdGenerator, Sampler, SdkTracer, SdkTracerProvider},
};
use tracing::{Span, info, warn};
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
/// - 错误请求告警（status >= 400）
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

    if status >= 400 {
        tracing::warn!(
            http.status_code = status,
            http.duration_ms = elapsed.as_millis(),
            http.route = %path,
            http.user_id = %user_id,
            "HTTP 错误响应"
        );
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

// ============ 工具函数 ============

/// 为外部调用（如数据库查询、RPC 调用）创建子 Span
///
/// # 示例
/// ```
/// let span = ryframe_middleware::telemetry::child_span(
///     "db.query_user",
///     &[("db.user_id", "42".to_string())]
/// );
/// let _guard = span.enter();
/// // ... 数据库操作
/// drop(_guard);
/// drop(span);
/// ```
pub fn child_span(name: &str, attrs: &[(&str, String)]) -> Span {
    let span = tracing::info_span!("otel.child", otel.name = name);

    for (k, v) in attrs {
        span.record(*k, v.as_str());
    }

    span
}

// ========== 外部调用 Span 工具 ==========

/// HTTP 客户端调用 Span
///
/// 为 HTTP 出站请求创建符合 OTel 语义约定的 Span。
/// 使用方式：在 span 的 context 内执行 HTTP 调用。
///
/// # 示例
/// ```
/// # use ryframe_middleware::telemetry::http_client_span;
/// let span = http_client_span("GET", "https://api.example.com/users", None);
/// let _guard = span.enter();
/// // let resp = reqwest::get("https://api.example.com/users").await?;
/// // span.record("http.status_code", resp.status().as_u16());
/// drop(_guard);
/// drop(span);
/// ```
pub fn http_client_span(method: &str, url: &str, body_size: Option<u64>) -> Span {
    let span = tracing::info_span!(
        "http.client",
        otel.name = format!("HTTP {}", method),
        otel.kind = "client",
        http.method = %method,
        http.url = %url,
    );
    if let Some(size) = body_size {
        span.record("http.request_content_length", size);
    }
    span
}

/// Redis 操作 Span
///
/// 为 Redis 操作创建 Span。
///
/// # 示例
/// ```
/// # use ryframe_middleware::telemetry::redis_span;
/// let span = redis_span("GET", "user:42");
/// let _guard = span.enter();
/// // let val = redis_client.get::<String>("user:42").await?;
/// drop(_guard);
/// drop(span);
/// ```
pub fn redis_span(command: &str, key: &str) -> Span {
    let span = tracing::info_span!(
        "redis",
        otel.name = format!("REDIS {}", command),
        otel.kind = "client",
        db.system = "redis",
        redis.command = %command,
        redis.key = %key,
    );
    span
}

/// gRPC 客户端调用 Span
///
/// 为 gRPC 出站调用创建符合 OTel 语义约定的 Span。
///
/// # 示例
/// ```
/// # use ryframe_middleware::telemetry::grpc_client_span;
/// let span = grpc_client_span("user.UserService", "GetUser");
/// let _guard = span.enter();
/// // let resp = grpc_client.get_user(request).await?;
/// drop(_guard);
/// drop(span);
/// ```
pub fn grpc_client_span(service: &str, method: &str) -> Span {
    let span = tracing::info_span!(
        "grpc.client",
        otel.name = format!("gRPC /{}/{}", service, method),
        otel.kind = "client",
        rpc.service = %service,
        rpc.method = %method,
        rpc.system = "grpc",
    );
    span
}

/// 消息队列发布 Span
///
/// 为消息发布操作创建 Span。
///
/// # 示例
/// ```
/// # use ryframe_middleware::telemetry::mq_produce_span;
/// let span = mq_produce_span("kafka", "order-events");
/// let _guard = span.enter();
/// // mq.publish("order-events", payload).await?;
/// drop(_guard);
/// drop(span);
/// ```
pub fn mq_produce_span(broker: &str, topic: &str) -> Span {
    let span = tracing::info_span!(
        "mq.publish",
        otel.name = format!("MQ PUB {}", topic),
        otel.kind = "producer",
        messaging.system = %broker,
        messaging.destination = %topic,
    );
    span
}

/// 缓存操作 Span（通用，用于 memory cache 等非 Redis 缓存）
///
/// # 示例
/// ```
/// # use ryframe_middleware::telemetry::cache_span;
/// let span = cache_span("get", "user:profile:42");
/// let _guard = span.enter();
/// // let val = cache.get("user:profile:42").await?;
/// drop(_guard);
/// drop(span);
/// ```
pub fn cache_span(operation: &str, key: &str) -> Span {
    let span = tracing::info_span!(
        "cache",
        otel.name = format!("CACHE {}", operation),
        otel.kind = "client",
        cache.operation = %operation,
        cache.key = %key,
    );
    span
}

/// 外部调用 Span 辅助宏
///
/// 为指定的 Span 名称和属性自动创建 Span 并进入上下文。
/// 适用于无法使用上述预定义函数的自定义场景。
///
/// # 示例
/// ```
/// # use ryframe_middleware::telemetry::external_span;
/// let span = external_span("email.send", &[("email.to", "user@example.com")]);
/// let _guard = span.enter();
/// // ... 发送邮件
/// drop(_guard);
/// drop(span);
/// ```
pub fn external_span(name: &str, attrs: &[(&str, &str)]) -> Span {
    let span = tracing::info_span!("external", otel.name = name, otel.kind = "client",);
    for (k, v) in attrs {
        span.record(*k, *v);
    }
    span
}
