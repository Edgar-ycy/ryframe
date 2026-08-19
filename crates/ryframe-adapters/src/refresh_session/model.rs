/// 刷新令牌族的权威状态。
#[derive(Debug, Clone)]
pub struct RefreshFamily {
    pub sid: String,
    pub tenant_id: String,
    pub user_id: i64,
    pub current_jti: String,
    pub previous_jti: Option<String>,
    pub last_attempt_id: Option<String>,
    pub rotated_at: i64,
    pub absolute_exp: i64,
    pub revoked: bool,
}

/// 活跃刷新会话的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshSessionIdentity {
    pub tenant_id: String,
    pub user_id: i64,
    pub absolute_exp: i64,
}

/// 按租户和用户撤销会话的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshSessionRevocation {
    Revoked,
    AlreadyRevoked,
    NotFoundOrForeign,
}

/// 刷新令牌轮换结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshRotation {
    Rotated { current_jti: String, issued_at: i64 },
    Recovered { current_jti: String, issued_at: i64 },
    Concurrent,
    Replayed,
    MissingOrRevoked,
}
