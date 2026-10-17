use std::fmt;

pub(crate) type GateResult<T> = std::result::Result<T, GateError>;

#[derive(Debug)]
pub(crate) struct GateError {
    kind: &'static str,
    message: String,
}

impl GateError {
    pub(crate) fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) const fn kind(&self) -> &'static str {
        self.kind
    }

    pub(crate) fn io(
        kind: &'static str,
        action: &str,
        path: &std::path::Path,
        error: &std::io::Error,
    ) -> Self {
        Self::new(kind, format!("{action} {} 失败：{error}", path.display()))
    }
}

impl fmt::Display for GateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GateError {}
