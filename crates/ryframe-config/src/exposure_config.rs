use serde::Deserialize;

/// 控制是否暴露交互式 API 文档。
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

/// 控制监控系统抓取的运维端点访问权限。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorConfig {
    /// `/api/v1/monitor/metrics` 所需的静态 Bearer 令牌。
    ///
    /// 开发环境可以留空；生产环境校验要求至少 32 字节，避免端点意外在未认证状态下暴露。
    #[serde(default)]
    pub metrics_bearer_token: String,
}
