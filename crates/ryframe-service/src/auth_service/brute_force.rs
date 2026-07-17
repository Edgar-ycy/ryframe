use ryframe_common::{AppError, AppResult};

use super::AuthService;

impl AuthService {
    /// 检查登录暴力破解。
    ///
    /// Redis 不可用时降级为无条件放行。
    pub async fn check_brute_force(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        let max_attempts = self.config.auth.max_login_attempts;
        let lockout_seconds = (self.config.auth.lockout_duration_minutes * 60) as u64;

        let Some(redis) = self.redis.as_ref() else {
            return Ok(());
        };

        let user_key = format!("login_fail:{tenant_id}:user:{username}");
        if let Ok(Some(count)) = redis.get(&user_key).await
            && let Ok(count) = count.parse::<u32>()
            && count >= max_attempts
        {
            let ttl = redis.ttl(&user_key).await.unwrap_or(lockout_seconds as i64);
            if ttl > 0 {
                return Err(AppError::Authentication(format!(
                    "账户已被锁定，请 {ttl} 秒后再试"
                )));
            }
        }

        let ip_key = format!("login_fail:{tenant_id}:ip:{ip}");
        if let Ok(Some(count)) = redis.get(&ip_key).await
            && let Ok(count) = count.parse::<u32>()
            && count >= max_attempts * 2
        {
            let ttl = redis.ttl(&ip_key).await.unwrap_or(lockout_seconds as i64);
            if ttl > 0 {
                return Err(AppError::Authentication(format!(
                    "IP 已被临时限制，请 {ttl} 秒后再试"
                )));
            }
        }

        Ok(())
    }

    /// 记录登录失败并刷新计数器过期时间。
    pub async fn record_login_failure(&self, tenant_id: &str, username: &str, ip: &str) {
        let Some(redis) = self.redis.as_ref() else {
            return;
        };
        let lockout_seconds = (self.config.auth.lockout_duration_minutes * 60) as u64;
        let user_key = format!("login_fail:{tenant_id}:user:{username}");
        let ip_key = format!("login_fail:{tenant_id}:ip:{ip}");

        let _ = redis.incr(&user_key).await;
        let _ = redis.expire(&user_key, lockout_seconds).await;
        let _ = redis.incr(&ip_key).await;
        let _ = redis.expire(&ip_key, lockout_seconds).await;
    }

    /// 登录成功后清除失败计数。
    pub async fn clear_login_failures(&self, tenant_id: &str, username: &str, ip: &str) {
        let Some(redis) = self.redis.as_ref() else {
            return;
        };
        let user_key = format!("login_fail:{tenant_id}:user:{username}");
        let ip_key = format!("login_fail:{tenant_id}:ip:{ip}");
        let _ = redis.del(&user_key).await;
        let _ = redis.del(&ip_key).await;
    }
}
