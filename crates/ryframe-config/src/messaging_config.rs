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
    /// 共享补拉调度器扫描在线身份的间隔，单位为秒。
    pub replay_interval_seconds: u64,
    /// 共享补拉调度器的启动抖动上限，单位为秒。
    pub replay_jitter_seconds: u64,
    /// 单个租户用户每次补拉的最大消息数。
    pub replay_batch_size: u64,
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
            replay_interval_seconds: 15,
            replay_jitter_seconds: 5,
            replay_batch_size: 100,
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
        if !(1..=3_600).contains(&self.replay_interval_seconds) {
            return Err("messaging.replay_interval_seconds 必须在 1 到 3600 之间".into());
        }
        if self.replay_jitter_seconds > self.replay_interval_seconds {
            return Err(
                "messaging.replay_jitter_seconds 不能大于 messaging.replay_interval_seconds".into(),
            );
        }
        if !(1..=1_000).contains(&self.replay_batch_size) {
            return Err("messaging.replay_batch_size 必须在 1 到 1000 之间".into());
        }
        Ok(())
    }
}
