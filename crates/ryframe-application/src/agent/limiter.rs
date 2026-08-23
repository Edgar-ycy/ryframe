use async_trait::async_trait;
use ryframe_kernel::AppResult;

/// Agent 七维限流决策输入。
pub struct AgentLimitInput<'a> {
    pub ip: &'a str,
    pub tenant_id: &'a str,
    pub tenant_limit: i32,
    pub account_id: i64,
    pub account_limit: i32,
    pub credential_id: i64,
    pub represented_user_id: Option<i64>,
    pub capability_key: &'static str,
    pub capability_cost: u32,
    pub default_limit: u32,
    pub concurrency_limit: u32,
    pub concurrency_ttl_ms: u64,
    pub owner: &'a str,
}

/// Agent 并发槽位租约。
#[async_trait]
pub trait AgentConcurrencyLease: Send {
    async fn release(self: Box<Self>);
}

/// Agent 限流与并发租约端口。
#[async_trait]
pub trait AgentLimiter: Send + Sync {
    async fn guard_pre_auth_ip(&self, ip: &str, limit: u32) -> AppResult<()>;

    async fn acquire(
        &self,
        input: AgentLimitInput<'_>,
    ) -> AppResult<Box<dyn AgentConcurrencyLease>>;
}
