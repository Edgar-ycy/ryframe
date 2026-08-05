use std::{
    fmt::Write,
    io,
    io::Write as _,
    time::{Duration, SystemTime},
};

use opentelemetry::{
    Context as OtelContext, KeyValue, global,
    trace::{Span as _, SpanKind, Tracer as _},
};
use opentelemetry_semantic_conventions::attribute::{DB_OPERATION_NAME, DB_SYSTEM_NAME};
use tracing::{
    Event,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// SQL 日志 Layer
///
/// 仅拦截 `target = "sqlx::query"` 的事件，格式化输出：
///   `[SQL]` SELECT * FROM sys_user WHERE ... [耗时: 0.81ms] [返回: 1行]
///
/// 当 `slow_query_threshold_ms > 0` 且查询耗时超过阈值时，额外输出 WARN 级别日志。
/// 其他事件透传给下游 Layer。
pub struct SqlLogLayer {
    level: ryframe_config::SqlLogLevel,
    slow_threshold_ms: u64,
}

impl SqlLogLayer {
    pub fn new(level: ryframe_config::SqlLogLevel, slow_threshold_ms: u64) -> Self {
        Self {
            level,
            slow_threshold_ms,
        }
    }
}

impl<S> Layer<S> for SqlLogLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();

        // 非 sqlx 事件或关闭模式，静默透传
        if meta.target() != "sqlx::query" || self.level == ryframe_config::SqlLogLevel::Off {
            return;
        }

        let mut visitor = SqlxVisitor::default();
        event.record(&mut visitor);

        // 获取 SQL 语句（优先用 db.statement，其次用 summary）
        let sql = visitor
            .statement
            .as_deref()
            .or(visitor.summary.as_deref())
            .unwrap_or("");
        let sql_clean = clean_sql(sql);

        // 耗时
        let elapsed_ms = visitor.elapsed_secs.unwrap_or(0.0) * 1000.0;
        let rows = visitor.rows_returned.or(visitor.rows_affected).unwrap_or(0);

        // 构建日志行
        let mut line = format!("[SQL] {}", sql_clean);

        if elapsed_ms > 0.0 {
            write!(line, " [耗时: {:.2}ms]", elapsed_ms).ok();

            // 慢查询告警
            if self.slow_threshold_ms > 0 && elapsed_ms > self.slow_threshold_ms as f64 {
                writeln!(
                    io::stderr(),
                    "[SLOW QUERY WARN]  {}  [耗时: {:.2}ms > 阈值: {}ms]",
                    sql_clean,
                    elapsed_ms,
                    self.slow_threshold_ms
                )
                .ok();
            }
        }
        if visitor.rows_returned.is_some() {
            write!(line, " [返回: {}行]", rows).ok();
        } else if visitor.rows_affected.is_some() && rows > 0 {
            write!(line, " [影响: {}行]", rows).ok();
        }

        // full 模式额外输出完整 SQL（去空白）
        if self.level == ryframe_config::SqlLogLevel::Full
            && let Some(ref stmt) = visitor.statement
        {
            let full_sql = clean_sql(stmt);
            if full_sql != sql_clean {
                writeln!(io::stdout(), "[SQL] 完整: {}", full_sql).ok();
            }
        }

        writeln!(io::stdout(), "{}", line).ok();
    }
}

/// 清洗 SQL：去除前导换行和多余空白
fn clean_sql(raw: &str) -> String {
    let trimmed = raw.trim();
    // 将连续空白替换为单个空格
    let single_line: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line
}

/// 访问器：从 tracing Event 中提取 sqlx 结构化字段
#[derive(Default)]
struct SqlxVisitor {
    summary: Option<String>,
    statement: Option<String>,
    rows_returned: Option<u64>,
    rows_affected: Option<u64>,
    elapsed_secs: Option<f64>,
}

impl Visit for SqlxVisitor {
    /// 字符串字段直接获取原始值，避免 Debug 格式加引号
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "db.statement" => self.statement = Some(value.to_string()),
            "summary" => self.summary = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let s = format!("{:?}", value);
        match field.name() {
            "rows_returned" => self.rows_returned = s.parse().ok(),
            "rows_affected" => self.rows_affected = s.parse().ok(),
            "elapsed_secs" => self.elapsed_secs = s.parse().ok(),
            _ => {}
        }
    }
}

// ========== DB Span 追踪 Layer ==========

/// DB 查询 Span 追踪 Layer
///
/// 为每个 sqlx 查询直接创建 OpenTelemetry DB Span。
///
/// - 自动提取 SQL 操作类型（SELECT / INSERT / UPDATE / DELETE）
/// - 记录数据库系统、稳定的操作类型和 sqlx 上报的真实查询耗时
/// - 不向 OpenTelemetry 写入原始 SQL，避免文本值、标识符或动态语句造成敏感信息和
///   高基数字段扩散
/// - Span 继承 tracing-opentelemetry 激活的当前上下文，在 Jaeger/Tempo 中展示完整调用链
///
/// # 使用方式
///
/// ```text
/// use ryframe_db::sql_logger::{DbSpanLayer, SqlLogLayer};
/// use ryframe_config::SqlLogLevel;
///
/// // 创建 DB Span 追踪层（自包含，无需实际 subscriber）
/// let db_span_layer = DbSpanLayer::new();
///
/// // 创建 SQL 日志层
/// let sql_log_layer = SqlLogLayer::new(SqlLogLevel::Summary, 100);
///
/// // 注册到 subscriber：
/// // tracing_subscriber::registry()
/// //     .with(db_span_layer)
/// //     .with(sql_log_layer)
/// //     .init();
/// ```
pub struct DbSpanLayer;

impl DbSpanLayer {
    /// 创建新的 DbSpanLayer
    pub fn new() -> Self {
        Self
    }
}

impl Default for DbSpanLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for DbSpanLayer
where
    S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();

        // 仅处理 sqlx::query 事件
        if meta.target() != "sqlx::query" {
            return;
        }

        let mut visitor = SqlxVisitor::default();
        event.record(&mut visitor);

        let sql = visitor
            .statement
            .as_deref()
            .or(visitor.summary.as_deref())
            .unwrap_or("");
        // 提取 SQL 操作类型
        let operation = extract_sql_operation(sql);

        emit_db_span(operation, visitor.elapsed_secs);
    }
}

fn emit_db_span(operation: &str, elapsed_secs: Option<f64>) {
    let end_time = SystemTime::now();
    let start_time = elapsed_secs
        .filter(|elapsed| elapsed.is_finite() && *elapsed >= 0.0)
        .and_then(|elapsed| Duration::try_from_secs_f64(elapsed).ok())
        .and_then(|duration| end_time.checked_sub(duration))
        .unwrap_or(end_time);
    let tracer = global::tracer("ryframe-db");
    let parent_context = OtelContext::current();
    let mut span = tracer
        .span_builder(format!("SQL {operation}"))
        .with_kind(SpanKind::Client)
        .with_start_time(start_time)
        .with_attributes([
            KeyValue::new(DB_SYSTEM_NAME, "mysql"),
            KeyValue::new(DB_OPERATION_NAME, operation.to_owned()),
        ])
        .start_with_context(&tracer, &parent_context);
    span.end_with_timestamp(end_time);
}

/// 从 SQL 语句首关键字提取操作类型
fn extract_sql_operation(sql: &str) -> &str {
    let upper = sql.trim_start().to_uppercase();
    if upper.starts_with("SELECT") {
        "SELECT"
    } else if upper.starts_with("INSERT") {
        "INSERT"
    } else if upper.starts_with("UPDATE") {
        "UPDATE"
    } else if upper.starts_with("DELETE") {
        "DELETE"
    } else if upper.starts_with("CREATE") || upper.starts_with("ALTER") || upper.starts_with("DROP")
    {
        "DDL"
    } else if upper.starts_with("BEGIN")
        || upper.starts_with("COMMIT")
        || upper.starts_with("ROLLBACK")
    {
        "TXN"
    } else {
        "OTHER"
    }
}
