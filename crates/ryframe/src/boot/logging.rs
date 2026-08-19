use std::path::Path;

use ryframe_adapters::telemetry::{TelemetryGuard, init_tracer_provider};
use ryframe_config::{AppConfig, LoggerFormat, LoggerOutput};
use ryframe_db::{DbSpanLayer, SqlLogGuard, SqlLogLayer};
use ryframe_kernel::AppError;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    EnvFilter, Layer, filter::FilterFn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

/// 日志 Guard，保证滚动文件 writer 不被提前 Drop
pub struct LoggerGuard {
    // 字段按声明顺序销毁，SQL 线程会先排空，再关闭共享的非阻塞 writer。
    _sql_log_worker: SqlLogGuard,
    _worker: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// 初始化日志系统
///
/// - `output = "stdout"` → 控制台输出
/// - `output = "file"` → 每天滚动，并按 `retention_days` 有界保留
/// - `format = "json"` → JSON 格式，否则 text 格式
/// - 同时初始化 OpenTelemetry 链路追踪（通过环境变量控制）
pub fn init(config: &AppConfig) -> Result<(LoggerGuard, TelemetryGuard), AppError> {
    config.logger.validate().map_err(AppError::Config)?;
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logger.level.as_str()));

    let is_json = config.logger.format == LoggerFormat::Json;
    let sql_log_level = config.database.sql_log_level;
    let sql_slow_threshold_ms = config.database.sql_slow_threshold_ms;
    let (sql_log_layer, sql_log_guard) =
        SqlLogLayer::with_guard(sql_log_level, sql_slow_threshold_ms);

    // 原始 SQLx 事件由 SqlLogLayer 转换为 ryframe.sql 结构化事件，避免重复输出。
    let sqlx_filter = FilterFn::new(|meta| meta.target() != "sqlx::query");

    // 初始化链路追踪（在 subscriber 构建之前）
    let telemetry_guard = init_tracer_provider(&config.telemetry);
    let otel_layer = telemetry_guard.tracing_layer();

    // 构建 subscriber 的顺序很关键：
    // 1. fmt_layer（含 sqlx 过滤器）→ 2. SqlLogLayer → 3. otel(可选) → 4. env_filter
    // env_filter 放最后因为 EnvFilter: Layer<S> for all S: Subscriber，
    // 而 Filtered<FmtLayer, FilterFn, Registry> 只能 Layer<Registry>，
    // 无法 layer 到 Layered<EnvFilter, Registry> 上
    if let Some(file_appender) = prepare_file_appender(
        config.logger.output,
        Path::new("logs"),
        config.logger.retention_days,
    )? {
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
                .with(sql_log_layer)
        } else {
            let fmt_layer = fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(sqlx_filter)
                .boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(sql_log_layer)
        };

        if let Some(otel) = otel_layer {
            // 数据库 SQL 文本仅写入受 SQL 日志级别控制的本地日志；
            // OpenTelemetry 只保留 DbSpanLayer 创建的无 SQL 数据库 span。
            subscriber
                .with(otel.with_filter(FilterFn::new(|meta| {
                    !matches!(meta.target(), "sqlx::query" | "ryframe.sql")
                })))
                .with(env_filter)
                .init();
        } else {
            subscriber.with(env_filter).init();
        }

        report_sql_logging_config(config);

        Ok((
            LoggerGuard {
                _sql_log_worker: sql_log_guard,
                _worker: Some(guard),
            },
            telemetry_guard,
        ))
    } else {
        // 控制台输出
        let subscriber = if is_json {
            let fmt_layer = fmt::layer().json().with_filter(sqlx_filter).boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(sql_log_layer)
        } else {
            let fmt_layer = fmt::layer().with_filter(sqlx_filter).boxed();
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(DbSpanLayer::new())
                .with(sql_log_layer)
        };

        if let Some(otel) = otel_layer {
            // 数据库 SQL 文本仅写入受 SQL 日志级别控制的本地日志；
            // OpenTelemetry 只保留 DbSpanLayer 创建的无 SQL 数据库 span。
            subscriber
                .with(otel.with_filter(FilterFn::new(|meta| {
                    !matches!(meta.target(), "sqlx::query" | "ryframe.sql")
                })))
                .with(env_filter)
                .init();
        } else {
            subscriber.with(env_filter).init();
        }

        report_sql_logging_config(config);

        Ok((
            LoggerGuard {
                _sql_log_worker: sql_log_guard,
                _worker: None,
            },
            telemetry_guard,
        ))
    }
}

fn report_sql_logging_config(config: &AppConfig) {
    tracing::info!(
        mode = ?config.database.sql_log_level,
        slow_threshold_ms = config.database.sql_slow_threshold_ms,
        "SQL 日志配置已加载"
    );
    if config.environment.is_production()
        && matches!(
            config.database.sql_log_level,
            ryframe_config::SqlLogLevel::Summary | ryframe_config::SqlLogLevel::Full
        )
    {
        tracing::warn!(
            mode = ?config.database.sql_log_level,
            "生产环境已启用详细 SQL 日志，请仅在短时排障期间使用"
        );
    }
}

fn build_file_appender(
    directory: &Path,
    retention_days: usize,
) -> Result<RollingFileAppender, AppError> {
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("ryframe.log")
        .max_log_files(retention_days)
        .build(directory)
        .map_err(|error| AppError::Config(format!("初始化滚动日志目录失败: {error}")))
}

fn prepare_file_appender(
    output: LoggerOutput,
    directory: &Path,
    retention_days: usize,
) -> Result<Option<RollingFileAppender>, AppError> {
    match output {
        LoggerOutput::Stdout => Ok(None),
        LoggerOutput::File => build_file_appender(directory, retention_days).map(Some),
    }
}
