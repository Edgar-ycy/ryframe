use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

pub type AgentLimitFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;
pub type AgentLeaseReleaseFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

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
pub trait AgentConcurrencyLease: Send {
    fn release(self: Box<Self>) -> AgentLeaseReleaseFuture;
}

/// Agent 限流与并发租约端口。
pub trait AgentLimiter: Send + Sync {
    fn guard_pre_auth_ip<'a>(&'a self, ip: &'a str, limit: u32) -> AgentLimitFuture<'a, ()>;

    fn acquire<'a>(
        &'a self,
        input: AgentLimitInput<'a>,
    ) -> AgentLimitFuture<'a, Box<dyn AgentConcurrencyLease>>;
}
