use std::{future::Future, net::IpAddr, pin::Pin};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub retry_after_secs: u64,
}

pub type RateLimitFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RateLimitDecision, String>> + Send + 'a>>;

/// HTTP 传输层所需的限流端口，由组合根绑定具体实现。
pub trait HttpRateLimiter: Send + Sync {
    fn acquire<'a>(&'a self, key: &'a str, window_secs: u64, limit: u32) -> RateLimitFuture<'a>;
}

pub fn tenant_key(tenant_id: &str) -> String {
    format!("tenant:{tenant_id}")
}

pub fn tenant_user_key(tenant_id: &str, user_id: &str) -> String {
    format!("tenant_user:{tenant_id}:{user_id}")
}

pub fn api_client_key(path: &str, client_ip: IpAddr) -> String {
    format!("api:{path}:ip:{client_ip}")
}
