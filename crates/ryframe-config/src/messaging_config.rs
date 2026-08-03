use serde::Deserialize;

/// 消息中心的运行时配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MessagingConfig {
    /// 是否启用消息收件箱、发布、WebSocket 票据与实时投递。
    pub enabled: bool,
    /// 一次性 WebSocket 票据的有效期，单位为秒。
    pub ticket_ttl_seconds: u64,
    /// 消息允许保留的最大天数。
    pub retention_days: u32,
    /// 每个租户用户在单个 API 实例上的最大 WebSocket 连接数。
    pub max_connections_per_user: usize,
    /// 每条 WebSocket 连接的有界出站队列容量。
    pub outbound_buffer: usize,
    /// 单条消息允许固化的最大收件人数。
    pub max_recipients_per_message: u64,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ticket_ttl_seconds: 60,
            retention_days: 90,
            max_connections_per_user: 5,
            outbound_buffer: 256,
            max_recipients_per_message: 100_000,
        }
    }
}

impl MessagingConfig {
    /// 校验消息中心所有容量与生命周期边界。
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=3_600).contains(&self.ticket_ttl_seconds) {
            return Err("messaging.ticket_ttl_seconds 必须在 1 到 3600 之间".into());
        }
        if !(1..=3_650).contains(&self.retention_days) {
            return Err("messaging.retention_days 必须在 1 到 3650 之间".into());
        }
        if !(1..=100).contains(&self.max_connections_per_user) {
            return Err("messaging.max_connections_per_user 必须在 1 到 100 之间".into());
        }
        if !(1..=65_536).contains(&self.outbound_buffer) {
            return Err("messaging.outbound_buffer 必须在 1 到 65536 之间".into());
        }
        if !(1..=1_000_000).contains(&self.max_recipients_per_message) {
            return Err("messaging.max_recipients_per_message 必须在 1 到 1000000 之间".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MessagingConfig;

    #[test]
    fn defaults_match_the_runtime_policy() {
        let config = MessagingConfig::default();

        assert!(config.enabled);
        assert_eq!(config.ticket_ttl_seconds, 60);
        assert_eq!(config.retention_days, 90);
        assert_eq!(config.max_connections_per_user, 5);
        assert_eq!(config.outbound_buffer, 256);
        assert_eq!(config.max_recipients_per_message, 100_000);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn zero_capacity_is_rejected_even_when_messaging_is_disabled() {
        let config = MessagingConfig {
            enabled: false,
            outbound_buffer: 0,
            ..MessagingConfig::default()
        };

        assert!(config.validate().is_err());
    }
}
