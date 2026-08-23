use ryframe_kernel::AppResult;

/// 登录失败计数与临时锁定的出站端口。
#[async_trait::async_trait]
pub trait LoginProtectionPort: Send + Sync {
    async fn ensure_allowed(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
        max_attempts: u32,
    ) -> AppResult<()>;

    async fn record_failure(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
        lockout_seconds: u64,
    ) -> AppResult<()>;

    async fn clear(&self, tenant_id: &str, username: &str, ip: &str) -> AppResult<()>;
}
