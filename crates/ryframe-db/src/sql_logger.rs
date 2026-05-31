use std::{fmt::Write, io, io::Write as _};

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
/// 为每个 sqlx 查询自动创建 `tracing::Span`，OpenTelemetry Layer 会将其导出为 DB Span。
///
/// - 自动提取 SQL 操作类型（SELECT / INSERT / UPDATE / DELETE）
/// - 记录耗时、影响行数、数据库系统
/// - Span 作为当前 HTTP 请求 Span 的子 Span，在 Jaeger/Tempo 中展示完整调用链
///
/// # 使用方式
///
/// ```ignore
/// tracing_subscriber::registry()
///     .with(DbSpanLayer::new())
///     .with(SqlLogLayer::new(level, threshold))
///     .with(otel_layer)
///     .init();
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
        let sql_clean = clean_sql(sql);
        let elapsed_ms = visitor.elapsed_secs.unwrap_or(0.0) * 1000.0;
        let rows = visitor.rows_returned.or(visitor.rows_affected).unwrap_or(0);

        // 提取 SQL 操作类型
        let operation = extract_sql_operation(&sql_clean);

        // 创建 DB 查询子 Span（继承当前 HTTP Span 作为父 Span）
        let span = tracing::info_span!(
            "db.query",
            otel.name = format!("SQL {}", operation),
            otel.kind = "client",
            db.system = "mysql",
            db.operation = %operation,
            db.statement = %sql_clean,
            db.rows = rows,
            db.duration_ms = elapsed_ms,
        );

        let _enter = span.enter();
        // Span 在离开作用域时自动 drop，
        // tracing-opentelemetry Layer 会在 span close 时导出 OTel Span
    }
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
