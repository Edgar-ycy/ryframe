use std::fmt::Write;
use std::io;
use std::io::Write as _;
use tracing::Event;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// 若依风格的 SQL 日志 Layer
///
/// 仅拦截 `target = "sqlx::query"` 的事件，格式化输出：
///   [SQL] SELECT * FROM sys_user WHERE ... [耗时: 0.81ms] [返回: 1行]
///
/// 其他事件透传给下游 Layer。
pub struct SqlLogLayer {
    level: ryframe_config::SqlLogLevel,
}

impl SqlLogLayer {
    pub fn new(level: ryframe_config::SqlLogLevel) -> Self {
        Self { level }
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
