use std::{future::Future, pin::Pin};

use ryframe_kernel::AppResult;

pub type LoginProtectionFuture<'a> = Pin<Box<dyn Future<Output = AppResult<()>> + Send + 'a>>;

/// 登录失败计数与临时锁定的出站端口。
pub trait LoginProtectionPort: Send + Sync {
    fn ensure_allowed<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
        ip: &'a str,
        max_attempts: u32,
    ) -> LoginProtectionFuture<'a>;

    fn record_failure<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
        ip: &'a str,
        lockout_seconds: u64,
    ) -> LoginProtectionFuture<'a>;

    fn clear<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
        ip: &'a str,
    ) -> LoginProtectionFuture<'a>;
}
