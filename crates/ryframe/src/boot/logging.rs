use ryframe_config::AppConfig;
use ryframe_db::{DbSpanLayer, SqlLogLayer};
use ryframe_middleware::telemetry::{TelemetryConfig, init_tracer_provider};
use tracing_subscriber::{
    EnvFilter, Layer, filter::FilterFn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

/// 日志 Guard，保证滚动文件 writer 不被提前 Drop
pub struct LoggerGuard {
    _worker: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// 初始化日志系统
///
/// - `output = "stdout"` → 控制台输出
/// - `output = "file"` → 滚动文件输出（每天滚动，保留 7 天）
/// - `format = "json"` → JSON 格式，否则 text 格式
/// - 同时初始化 OpenTelemetry 链路追踪（通过环境变量控制）
pub fn init(
    config: &AppConfig,
) -> (
    LoggerGuard,
    Option<ryframe_middleware::telemetry::TelemetryGuard>,
) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logger.level));

    let is_json = config.logger.format == "json";
    let sql_log_level = config.database.sql_log_level;

    // 阻止 sqlx 查询事件到达 fmt 层（由 SqlLogLayer 单独格式化输出）
    let sqlx_filter = FilterFn::new(|meta| meta.target() != "sqlx::query");

    // 初始化链路追踪（在 subscriber 构建之前）
    let telemetry_config = TelemetryConfig {
        enabled: std::env::var("OTEL_ENABLED").unwrap_or_else(|_| "false".into()) == "true",
        endpoint: std::env::var("OTEL_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318/v1/traces".into()),
        service_name: std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "ryframe".into()),
        sample_rate: std::env::var("OTEL_SAMPLE_RATE")
            .unwrap_or_else(|_| "1.0".into())
            .parse()
            .unwrap_or(1.0),
    };
    let telemetry_guard = init_tracer_provider(&telemetry_config);
    let otel_layer = telemetry_guard.tracing_layer();

    // 构建 subscriber 的顺序很关键：
    // 1. fmt_layer（含 sqlx 过滤器）→ 2. SqlLogLayer → 3. otel(可选) → 4. env_filter
    // env_filter 放最后因为 EnvFilter: Layer<S> for all S: Subscriber，
    // 而 Filtered<FmtLayer, FilterFn, Registry> 只能 Layer<Registry>，
    // 无法 layer 到 Layered<EnvFilter, Registry> 上
    if config.logger.output == "file" {
        let file_appender = tracing_appender::rolling::daily("logs", "ryframe.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        // 构建 subscriber：先用 .boxed() 擦除 Filtered 类型
        let subscriber = if is_json {
            let fmt_layer = fmt::layer()
                .json()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(sqlx_filter)
                .boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        } else {
            let fmt_layer = fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(sqlx_filter)
                .boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        };

        if let Some(otel) = otel_layer {
            subscriber.with(otel).with(env_filter).init();
        } else {
            subscriber.with(env_filter).init();
        }

        (
            LoggerGuard {
                _worker: Some(guard),
            },
            Some(telemetry_guard),
        )
    } else {
        // 控制台输出
        let subscriber = if is_json {
            let fmt_layer = fmt::layer().json().with_filter(sqlx_filter).boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        } else {
            let fmt_layer = fmt::layer().with_filter(sqlx_filter).boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(SqlLogLayer::new(sql_log_level, 0))
        };

        if let Some(otel) = otel_layer {
            subscriber.with(otel).with(env_filter).init();
        } else {
            subscriber.with(env_filter).init();
        }

        (LoggerGuard { _worker: None }, Some(telemetry_guard))
    }
}
