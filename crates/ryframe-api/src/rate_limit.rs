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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::{api_client_key, tenant_key, tenant_user_key};

    #[test]
    fn rate_limit_keys_preserve_existing_namespaces() {
        assert_eq!(tenant_key("tenant-a"), "tenant:tenant-a");
        assert_eq!(tenant_user_key("tenant-a", "42"), "tenant_user:tenant-a:42");
        assert_eq!(
            api_client_key("/api/v1/users", IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "api:/api/v1/users:ip:127.0.0.1"
        );
    }
}
