use ryframe_kernel::AppResult;

use super::AuthService;

impl AuthService {
    /// 检查登录暴力破解。
    ///
    /// 分布式状态不可用时由端口实现失败关闭，避免绕过锁定状态。
    pub async fn check_brute_force(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        crate::enforce_tenant_scope(tenant_id)?;
        self.login_protection
            .ensure_allowed(tenant_id, username, ip, self.policy.max_login_attempts)
            .await
    }

    /// 记录登录失败并刷新计数器过期时间。
    pub async fn record_login_failure(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        crate::enforce_tenant_scope(tenant_id)?;
        self.login_protection
            .record_failure(tenant_id, username, ip, self.policy.lockout_seconds)
            .await
    }

    /// 登录成功后清除失败计数。
    pub async fn clear_login_failures(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        crate::enforce_tenant_scope(tenant_id)?;
        self.login_protection.clear(tenant_id, username, ip).await
    }
}
