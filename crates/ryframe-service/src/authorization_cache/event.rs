use serde::{Deserialize, Serialize};

/// 跨 API 实例广播租户授权纪元变化的 Redis 频道。
pub const AUTHORIZATION_CHANGED_REDIS_CHANNEL: &str = "ryframe:authorization:changed";

/// 授权规则变化后的轻量实时通知；权限明细仍由认证接口重新读取。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuthorizationChangedEvent {
    pub tenant_id: String,
    pub authorization_epoch: i32,
}
