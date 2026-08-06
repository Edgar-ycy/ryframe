use tracing::{
    Event,
    field::{Field, Visit},
};

/// 从 SQLx 查询事件中提取的稳定字段。
#[derive(Default)]
pub(super) struct SqlxEventFields {
    message: Option<String>,
    summary: Option<String>,
    statement: Option<String>,
    pub(super) rows_returned: Option<u64>,
    pub(super) rows_affected: Option<u64>,
    pub(super) elapsed_secs: Option<f64>,
    pub(super) slow: bool,
}

impl SqlxEventFields {
    pub(super) fn from_event(event: &Event<'_>) -> Self {
        let mut fields = Self::default();
        event.record(&mut fields);
        fields
    }

    pub(super) fn summary(&self) -> &str {
        non_empty(self.summary.as_deref())
            .or_else(|| {
                non_empty(self.message.as_deref())
                    .filter(|message| !message.starts_with("slow statement:"))
            })
            .or_else(|| non_empty(self.statement.as_deref()))
            .unwrap_or("SQL 不可用")
    }

    pub(super) fn statement(&self) -> &str {
        non_empty(self.statement.as_deref()).unwrap_or_else(|| self.summary())
    }

    pub(super) fn elapsed_ms(&self) -> f64 {
        self.elapsed_secs.unwrap_or_default() * 1_000.0
    }
}

impl Visit for SqlxEventFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_text(field.name(), value.to_owned());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "rows_returned" => self.rows_returned = Some(value),
            "rows_affected" => self.rows_affected = Some(value),
            _ => {}
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.elapsed_secs = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let value = format!("{value:?}");
        match field.name() {
            "message" | "summary" | "db.statement" => {
                self.record_text(field.name(), value.trim_matches('"').to_owned());
            }
            "rows_returned" => self.rows_returned = value.parse().ok(),
            "rows_affected" => self.rows_affected = value.parse().ok(),
            "elapsed_secs" => self.elapsed_secs = value.parse().ok(),
            "slow_threshold" => self.slow = true,
            _ => {}
        }
    }
}

impl SqlxEventFields {
    fn record_text(&mut self, name: &str, value: String) {
        match name {
            "message" => self.message = Some(value),
            "summary" => self.summary = Some(value),
            "db.statement" => self.statement = Some(value),
            _ => {}
        }
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub(super) fn clean_sql(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn extract_sql_operation(sql: &str) -> &'static str {
    let first = sql
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('(');
    if first.eq_ignore_ascii_case("SELECT") {
        "SELECT"
    } else if first.eq_ignore_ascii_case("INSERT") {
        "INSERT"
    } else if first.eq_ignore_ascii_case("UPDATE") {
        "UPDATE"
    } else if first.eq_ignore_ascii_case("DELETE") {
        "DELETE"
    } else if ["CREATE", "ALTER", "DROP", "TRUNCATE"]
        .iter()
        .any(|operation| first.eq_ignore_ascii_case(operation))
    {
        "DDL"
    } else if ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT"]
        .iter()
        .any(|operation| first.eq_ignore_ascii_case(operation))
    {
        "TXN"
    } else {
        "OTHER"
    }
}
