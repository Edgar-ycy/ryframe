use serde_json::{Value, json};
use std::fmt;

pub(crate) type NativeResult<T> = Result<T, Failure>;

/// 只携带固定分类及契约 operation，不携带 URL、正文、SQL 或凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub category: &'static str,
    pub operation: Option<String>,
    pub status: Option<u16>,
    pub secondary: Vec<&'static str>,
}

impl Failure {
    pub(crate) fn new(category: &'static str) -> Self {
        Self {
            category,
            operation: None,
            status: None,
            secondary: Vec::new(),
        }
    }
    pub(crate) fn request(category: &'static str, operation: &str, status: Option<u16>) -> Self {
        if operation.is_empty()
            || operation.len() > 128
            || !operation.as_bytes()[0].is_ascii_lowercase()
            || !operation
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Self::new("request_failed");
        }
        Self {
            category,
            operation: Some(operation.into()),
            status: status.filter(|value| (400..=599).contains(value)),
            secondary: Vec::new(),
        }
    }
    pub(crate) fn document(&self) -> Value {
        if matches!(self.category, "request_timeout" | "request_cancelled") {
            return json!({"category": self.category});
        }
        if !matches!(
            self.category,
            "http_status"
                | "business_response"
                | "invalid_response"
                | "poll_failed"
                | "poll_timeout"
        ) {
            return json!({"category":"request_failed"});
        }
        let mut value = json!({"category": self.category});
        if let Some(operation) = &self.operation {
            value["operation"] = json!(operation);
        }
        if let Some(status) = self.status {
            value["status"] = json!(status);
        }
        value
    }
    pub(crate) fn and(mut self, other: Failure) -> Self {
        self.secondary.push(other.category);
        self.secondary.extend(other.secondary);
        self
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "运行时测量失败：{}", self.category)?;
        for category in &self.secondary {
            write!(formatter, "；{category}")?;
        }
        formatter.write_str("；请核对同次运行的隔离配置及阶段证据，未知写入不得自动重放")
    }
}
impl std::error::Error for Failure {}
