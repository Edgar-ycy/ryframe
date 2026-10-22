use std::{error::Error, fmt};

pub(crate) struct SessionError {
    pub category: &'static str,
    pub operation: Option<String>,
    pub status: Option<u16>,
}

impl SessionError {
    pub(super) fn new(
        category: &'static str,
        operation: Option<&str>,
        status: Option<u16>,
    ) -> Self {
        Self {
            category: category_name(category),
            operation: operation
                .filter(|value| valid_operation(value))
                .map(str::to_owned),
            status: status.filter(|value| (100..=599).contains(value)),
        }
    }
}

pub(super) fn valid_operation(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn category_name(category: &'static str) -> &'static str {
    match category {
        "invalid_configuration"
        | "invalid_identity"
        | "missing_credentials"
        | "invalid_operation"
        | "unsupported_step"
        | "invalid_request"
        | "crypto_provider"
        | "client_configuration"
        | "request_failed"
        | "request_timeout"
        | "http_status"
        | "invalid_response"
        | "business_response"
        | "hook_before"
        | "hook_complete" => category,
        _ => "request_failed",
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(category_name(self.category))?;
        if let Some(operation) = self
            .operation
            .as_deref()
            .filter(|value| valid_operation(value))
        {
            write!(formatter, ": {operation}")?;
        }
        if let Some(status) = self.status.filter(|value| (100..=599).contains(value)) {
            write!(formatter, " (HTTP {status})")?;
        }
        Ok(())
    }
}

impl fmt::Debug for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for SessionError {}
