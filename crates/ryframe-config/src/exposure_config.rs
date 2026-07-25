use serde::Deserialize;

/// Controls whether interactive API documentation is exposed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiDocsConfig {
    #[serde(default = "default_api_docs_enabled")]
    pub enabled: bool,
}

impl Default for ApiDocsConfig {
    fn default() -> Self {
        Self {
            enabled: default_api_docs_enabled(),
        }
    }
}

const fn default_api_docs_enabled() -> bool {
    true
}

/// Controls access to operational endpoints that are scraped by monitoring systems.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorConfig {
    /// Static bearer token required by `/api/v1/monitor/metrics`.
    ///
    /// Development may leave this empty. Production validation requires at least
    /// 32 bytes so the endpoint is never accidentally exposed without authentication.
    #[serde(default)]
    pub metrics_bearer_token: String,
}
