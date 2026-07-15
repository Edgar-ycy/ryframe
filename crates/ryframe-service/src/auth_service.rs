use std::sync::Arc;

use ryframe_auth::{jwt, password};
use ryframe_common::{AppError, AppResult};
use ryframe_config::AppConfig;
use ryframe_core::{LoggedRepo, RedisClient, TenantContext, with_tenant_context};
use ryframe_db::{
    PermissionRepository, RoleRepository, TenantRepository, UserRepository, entities::user,
};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use utoipa::ToSchema;

/// 登录响应（内部使用，最终由 API 层序列化为 JSON）
#[derive(Debug, Serialize)]
pub struct LoginResult {
    pub access_token: String,
    pub refresh_token: String,
    /// 令牌唯一标识，用于在线用户管理
    pub token_id: String,
    pub user_info: UserInfo,
}

/// 用户信息
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserInfo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub tenant_id: String,
    pub tenant_name: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub roles: Vec<String>,
    pub perms: Vec<String>,
}

impl From<&user::Model> for UserInfo {
    fn from(u: &user::Model) -> Self {
        Self {
            id: u.id.to_string(),
            tenant_id: u.tenant_id.clone(),
            tenant_name: String::new(),
            username: u.username.clone(),
            nickname: u.nickname.clone(),
            email: u.email.clone(),
            phone: u.phone.clone(),
            avatar: u.avatar.clone(),
            roles: vec![],
            perms: vec![],
        }
    }
}

/// 认证服务
pub struct AuthServiceImpl {
    pub user_repo: LoggedRepo<UserRepository>,
    pub role_repo: LoggedRepo<RoleRepository>,
    pub perm_repo: LoggedRepo<PermissionRepository>,
    pub config: Arc<AppConfig>,
    /// Redis 客户端（用于登录暴力破解防护，可空）
    pub redis: Option<RedisClient>,
}

impl AuthServiceImpl {
    /// 用户登录
    ///
    /// 验证用户名密码 → 查询角色权限 → 签发双 token → 返回用户信息和令牌。
    /// 用户名或密码错误统一返回 "用户名或密码错误"，防止用户枚举攻击。
    pub async fn login(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        username: &str,
        password: &str,
    ) -> AppResult<LoginResult> {
        let tenant = TenantRepository.ensure_available(db, tenant_id).await?;
        let user = self
            .user_repo
            .find_by_username_in_tenant(db, tenant_id, username)
            .await?
            .ok_or_else(|| AppError::Authentication("用户名或密码错误".into()))?;

        let valid = password::verify(password, &user.password_hash)?;
        if !valid {
            return Err(AppError::Authentication("用户名或密码错误".into()));
        }
        if !user.is_enabled() {
            return Err(AppError::Authentication("账号已停用或锁定".into()));
        }
        let (role_codes, perm_codes) = with_tenant_context(
            TenantContext {
                tenant_id: tenant_id.to_string(),
                is_admin: false,
            },
            async {
                let roles = self.role_repo.find_user_roles(db, user.id).await?;
                let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
                let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

                let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
                let perm_codes: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();
                Ok::<_, AppError>((role_codes, perm_codes))
            },
        )
        .await?;

        let identity = jwt::TokenIdentity {
            user_id: user.id,
            tenant_id: &user.tenant_id,
            tenant_session_version: tenant.session_version,
            user_auth_version: user.auth_version,
            username: &user.username,
        };
        let (access_token, token_id) = jwt::encode_access(&identity, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh(&identity, &self.config.auth)?;

        let mut user_info = UserInfo::from(&user);
        user_info.tenant_name = tenant.name.clone();
        user_info.roles = role_codes;
        user_info.perms = perm_codes;

        Ok(LoginResult {
            access_token,
            refresh_token,
            token_id,
            user_info,
        })
    }

    /// 刷新令牌
    ///
    /// 验证 refresh_token → 查用户 → 重新签发 access_token（权限即时生效）。
    pub async fn refresh_token(
        &self,
        db: &DatabaseConnection,
        token: &str,
    ) -> AppResult<LoginResult> {
        let claims = jwt::decode_token(token, &self.config.auth.jwt_secret)?;

        if claims.token_type != "refresh" {
            return Err(AppError::Authentication(
                "令牌类型错误，请使用刷新令牌".into(),
            ));
        }
        let tenant = TenantRepository
            .ensure_available(db, &claims.tenant_id)
            .await?;
        if claims.tenant_session_version != tenant.session_version {
            return Err(AppError::Authentication(
                "租户会话已失效，请重新登录".into(),
            ));
        }

        let user_id = claims
            .sub
            .parse::<i64>()
            .map_err(|_| AppError::Authentication("令牌中的用户ID无效".into()))?;

        let user = self
            .user_repo
            .find_by_id_in_tenant(db, &claims.tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authentication("账号已停用或锁定".into()));
        }
        if claims.user_auth_version != user.auth_version {
            return Err(AppError::Authentication(
                "用户权限已变更，请重新登录".into(),
            ));
        }

        let (role_codes, perm_codes) = with_tenant_context(
            TenantContext {
                tenant_id: claims.tenant_id.clone(),
                is_admin: false,
            },
            async {
                let roles = self.role_repo.find_user_roles(db, user.id).await?;
                let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
                let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

                let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
                let perm_codes: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();
                Ok::<_, AppError>((role_codes, perm_codes))
            },
        )
        .await?;

        let identity = jwt::TokenIdentity {
            user_id: user.id,
            tenant_id: &user.tenant_id,
            tenant_session_version: tenant.session_version,
            user_auth_version: user.auth_version,
            username: &user.username,
        };
        let (access_token, token_id) = jwt::encode_access(&identity, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh(&identity, &self.config.auth)?;

        let mut user_info = UserInfo::from(&user);
        user_info.tenant_name = tenant.name.clone();
        user_info.roles = role_codes;
        user_info.perms = perm_codes;

        Ok(LoginResult {
            access_token,
            refresh_token,
            token_id,
            user_info,
        })
    }

    /// 获取当前用户信息
    pub async fn get_current_user(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<UserInfo> {
        let tenant = TenantRepository.ensure_available(db, tenant_id).await?;
        let user = self
            .user_repo
            .find_by_id_in_tenant(db, tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authentication("账号已停用或锁定".into()));
        }

        let roles = self.role_repo.find_user_roles(db, user.id).await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

        let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
        let perm_codes: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        let mut user_info = UserInfo::from(&user);
        user_info.tenant_name = tenant.name;
        user_info.roles = role_codes;
        user_info.perms = perm_codes;

        Ok(user_info)
    }

    // ==================== 暴力破解防护 ====================

    /// 检查登录暴力破解
    ///
    /// 使用 Redis 记录失败次数，按用户名/IP 维度分别限流。
    /// - 连续失败超限后锁定指定分钟
    /// - Redis 不可用时降级为无条件放行
    pub async fn check_brute_force(&self, username: &str, ip: &str) -> AppResult<()> {
        let max_attempts = self.config.auth.max_login_attempts;
        let lockout_seconds = (self.config.auth.lockout_duration_minutes * 60) as u64;

        let redis = match &self.redis {
            Some(r) => r,
            None => return Ok(()),
        };

        // 按用户名限流
        let user_key = format!("login_fail:user:{}", username);
        if let Ok(Some(count)) = redis.get(&user_key).await
            && let Ok(c) = count.parse::<u32>()
            && c >= max_attempts
        {
            let ttl = redis.ttl(&user_key).await.unwrap_or(lockout_seconds as i64);
            if ttl > 0 {
                return Err(AppError::Authentication(format!(
                    "账户已被锁定，请 {} 秒后再试",
                    ttl
                )));
            }
        }

        // 按 IP 限流
        let ip_key = format!("login_fail:ip:{}", ip);
        if let Ok(Some(count)) = redis.get(&ip_key).await
            && let Ok(c) = count.parse::<u32>()
            && c >= max_attempts * 2
        {
            let ttl = redis.ttl(&ip_key).await.unwrap_or(lockout_seconds as i64);
            if ttl > 0 {
                return Err(AppError::Authentication(format!(
                    "IP 已被临时限制，请 {} 秒后再试",
                    ttl
                )));
            }
        }

        Ok(())
    }

    /// 记录登录失败（递增 Redis 计数器）
    pub async fn record_login_failure(&self, username: &str, ip: &str) {
        let redis = match &self.redis {
            Some(r) => r,
            None => return,
        };
        let lockout_seconds = (self.config.auth.lockout_duration_minutes * 60) as u64;
        let user_key = format!("login_fail:user:{}", username);
        let ip_key = format!("login_fail:ip:{}", ip);

        let _ = redis.incr(&user_key).await;
        let _ = redis.expire(&user_key, lockout_seconds).await;
        let _ = redis.incr(&ip_key).await;
        let _ = redis.expire(&ip_key, lockout_seconds).await;
    }

    /// 登录成功后清除失败计数
    pub async fn clear_login_failures(&self, username: &str, ip: &str) {
        let redis = match &self.redis {
            Some(r) => r,
            None => return,
        };
        let user_key = format!("login_fail:user:{}", username);
        let ip_key = format!("login_fail:ip:{}", ip);
        let _ = redis.del(&user_key).await;
        let _ = redis.del(&ip_key).await;
    }
}
