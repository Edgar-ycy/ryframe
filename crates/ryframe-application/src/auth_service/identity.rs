use ryframe_auth::jwt::Claims;
use ryframe_kernel::{AppError, AppResult};

use crate::{IdentityRoleRecord, IdentityTenantRecord, IdentityUserRecord};

use super::{AuthService, UserInfo};

pub(super) struct ValidatedIdentity {
    pub(super) tenant: IdentityTenantRecord,
    pub(super) user: IdentityUserRecord,
}

pub(super) struct AuthorizationProfile {
    pub(super) roles: Vec<IdentityRoleRecord>,
    pub(super) permissions: Vec<String>,
    pub(super) is_super_admin: bool,
}

impl AuthService {
    /// 校验一次性 WebSocket 票据绑定的会话仍处于有效状态。
    ///
    /// 票据在 Redis 中只有 60 秒寿命，但登出、租户会话版本变更或用户权限变更都应当
    /// 在握手前立即生效，因此这里同时校验持久化身份版本和刷新会话族状态。
    pub async fn validate_websocket_session(
        &self,
        tenant_id: &str,
        user_id: i64,
        session_id: &str,
        user_authorization_version: i32,
        tenant_session_version: i32,
    ) -> AppResult<()> {
        crate::enforce_tenant_scope(tenant_id)?;
        if user_id <= 0 || session_id.trim().is_empty() {
            return Err(AppError::Authentication("WebSocket 票据身份无效".into()));
        }
        let tenant = self.available_tenant(tenant_id).await?;
        if tenant.session_version != tenant_session_version {
            return Err(AppError::Authentication(
                "租户会话已失效，请重新登录".into(),
            ));
        }
        let user = self
            .authorization_resolver
            .user_by_id(tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;
        if !user.is_enabled() || user.authorization_version != user_authorization_version {
            return Err(AppError::Authentication(
                "用户会话已失效，请重新登录".into(),
            ));
        }
        if !self
            .refresh_sessions
            .is_active_for_identity(session_id, tenant_id, user_id)
            .await?
        {
            return Err(AppError::Authentication(
                "WebSocket 会话已失效，请重新登录".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn validate_token_identity(
        &self,
        claims: &Claims,
    ) -> AppResult<ValidatedIdentity> {
        crate::enforce_tenant_scope(&claims.tenant_id)?;
        let tenant = self.available_tenant(&claims.tenant_id).await?;
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
            .authorization_resolver
            .user_by_id(&claims.tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authentication("账号已停用或锁定".into()));
        }
        if claims.user_authorization_version != user.authorization_version {
            return Err(AppError::Authentication(
                "用户权限已变更，请重新登录".into(),
            ));
        }

        Ok(ValidatedIdentity { tenant, user })
    }

    pub(super) async fn load_authorization_profile(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<AuthorizationProfile> {
        let user = self
            .authorization_resolver
            .user_by_id(tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;
        let resolved = self
            .authorization_resolver
            .resolve(tenant_id, &user)
            .await?;
        let is_super_admin = resolved.is_super_admin();

        Ok(AuthorizationProfile {
            roles: resolved.roles,
            permissions: resolved.permission_codes,
            is_super_admin,
        })
    }

    pub(super) async fn build_user_info(
        &self,
        tenant_name: &str,
        user: &IdentityUserRecord,
        authorization: AuthorizationProfile,
    ) -> AppResult<UserInfo> {
        let AuthorizationProfile {
            roles,
            permissions,
            is_super_admin,
        } = authorization;
        let mut user_info = UserInfo::from(user);
        user_info.tenant_name = tenant_name.to_owned();
        user_info.dept_name = match user.dept_id {
            Some(dept_id) => {
                self.authorization_resolver
                    .department_name(&user.tenant_id, dept_id)
                    .await?
            }
            None => None,
        };
        user_info.roles = roles.into_iter().map(|role| role.code).collect();
        user_info.perms = permissions;
        user_info.is_super_admin = is_super_admin;
        Ok(user_info)
    }

    pub(super) async fn available_tenant(
        &self,
        tenant_id: &str,
    ) -> AppResult<IdentityTenantRecord> {
        let tenant = self
            .authorization_resolver
            .tenant(tenant_id)
            .await?
            .ok_or_else(|| AppError::Authentication("租户不存在".into()))?;
        if tenant.status != "enabled" {
            return Err(AppError::Authentication("租户已停用".into()));
        }
        if tenant
            .expire_at
            .is_some_and(|expire_at| expire_at <= chrono::Utc::now())
        {
            return Err(AppError::Authentication("租户已到期".into()));
        }
        Ok(tenant)
    }
}
