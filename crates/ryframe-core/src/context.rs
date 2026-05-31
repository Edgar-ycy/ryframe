use chrono::{DateTime, Utc};
use ryframe_config::AppConfig;

/// 应用全局上下文
///
/// 在 main.rs 启动时创建，通过 Axum State 注入到所有 Handler 中。
#[derive(Debug, Clone)]
pub struct AppContext {
    /// 应用配置
    pub config: AppConfig,
    /// 服务启动时间
    pub start_time: DateTime<Utc>,
}

impl AppContext {
    /// 创建应用上下文
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            start_time: Utc::now(),
        }
    }

    /// 服务已运行时长
    pub fn uptime(&self) -> chrono::Duration {
        Utc::now() - self.start_time
    }
}
