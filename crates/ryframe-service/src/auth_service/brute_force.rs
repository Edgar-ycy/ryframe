use ryframe_common::{AppError, AppResult};

use super::AuthService;

impl AuthService {
    /// 检查登录暴力破解。
    ///
    /// Redis 已配置但不可用时 fail-closed，避免绕过分布式锁定状态。
    pub async fn check_brute_force(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        let max_attempts = self.config.auth.max_login_attempts;
        let Some(redis) = self.redis.as_ref() else {
            return Ok(());
        };

        let user_key = login_subject_key(tenant_id, username);
        if let Some(count) = redis.get(&user_key).await.map_err(redis_unavailable)? {
            let count = count.parse::<u32>().map_err(|_| {
                AppError::ServiceUnavailable("login protection state is invalid".into())
            })?;
            if count >= max_attempts {
                let ttl = redis.ttl(&user_key).await.map_err(redis_unavailable)?;
                if ttl > 0 {
                    return Err(AppError::Authentication(format!(
                        "账户已被锁定，请 {ttl} 秒后再试"
                    )));
                }
            }
        }

        let ip_key = format!("ryframe:v0.5:login_fail:{tenant_id}:ip:{ip}");
        if let Some(count) = redis.get(&ip_key).await.map_err(redis_unavailable)? {
            let count = count.parse::<u32>().map_err(|_| {
                AppError::ServiceUnavailable("login protection state is invalid".into())
            })?;
            if count >= max_attempts * 2 {
                let ttl = redis.ttl(&ip_key).await.map_err(redis_unavailable)?;
                if ttl > 0 {
                    return Err(AppError::Authentication(format!(
                        "IP 已被临时限制，请 {ttl} 秒后再试"
                    )));
                }
            }
        }

        Ok(())
    }

    /// 记录登录失败并刷新计数器过期时间。
    pub async fn record_login_failure(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        let Some(redis) = self.redis.as_ref() else {
            return Ok(());
        };
        let lockout_seconds = (self.config.auth.lockout_duration_minutes * 60) as u64;
        let user_key = login_subject_key(tenant_id, username);
        let ip_key = format!("ryframe:v0.5:login_fail:{tenant_id}:ip:{ip}");

        redis.incr(&user_key).await.map_err(redis_unavailable)?;
        redis
            .expire(&user_key, lockout_seconds)
            .await
            .map_err(redis_unavailable)?;
        redis.incr(&ip_key).await.map_err(redis_unavailable)?;
        redis
            .expire(&ip_key, lockout_seconds)
            .await
            .map_err(redis_unavailable)?;
        Ok(())
    }

    /// 登录成功后清除失败计数。
    pub async fn clear_login_failures(
        &self,
        tenant_id: &str,
        username: &str,
        ip: &str,
    ) -> AppResult<()> {
        let Some(redis) = self.redis.as_ref() else {
            return Ok(());
        };
        let user_key = login_subject_key(tenant_id, username);
        let ip_key = format!("ryframe:v0.5:login_fail:{tenant_id}:ip:{ip}");
        redis.del(&user_key).await.map_err(redis_unavailable)?;
        redis.del(&ip_key).await.map_err(redis_unavailable)?;
        Ok(())
    }
}

fn redis_unavailable(error: impl std::fmt::Display) -> AppError {
    tracing::error!(%error, "login protection Redis operation failed");
    AppError::ServiceUnavailable("login protection service unavailable".into())
}

fn login_subject_key(tenant_id: &str, username: &str) -> String {
    let normalized_username = username.trim().to_lowercase();
    let digest =
        ryframe_common::utils::key::stable_scope_digest(&[tenant_id, &normalized_username]);
    format!("ryframe:v0.5:login_fail:principal:{digest}")
}
