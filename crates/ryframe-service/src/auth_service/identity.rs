use ryframe_auth::jwt::Claims;
use ryframe_core::Repository;
use ryframe_db::{
    TenantRepository,
    entities::{role, tenant, user},
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::DatabaseConnection;

use super::{AuthService, UserInfo};

pub(super) struct ValidatedIdentity {
    pub(super) tenant: tenant::Model,
    pub(super) user: user::Model,
}

pub(super) struct AuthorizationProfile {
    pub(super) roles: Vec<role::Model>,
    pub(super) permissions: Vec<String>,
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
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        if user_id <= 0 || session_id.trim().is_empty() {
            return Err(AppError::Authentication("WebSocket 票据身份无效".into()));
        }
        let tenant = TenantRepository
            .ensure_available(self.db.write(), tenant_id)
            .await?;
        if tenant.session_version != tenant_session_version {
            return Err(AppError::Authentication(
                "租户会话已失效，请重新登录".into(),
            ));
        }
        let user = self
            .user_repo
            .find_by_id(self.db.write(), tenant_id, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;
        if !user.is_enabled() || user.authorization_version != user_authorization_version {
            return Err(AppError::Authentication(
                "用户会话已失效，请重新登录".into(),
            ));
        }
        if !self.refresh_sessions.is_active(session_id).await? {
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
        self.validate_token_identity_on(self.db.write(), claims)
            .await
    }

    pub(super) async fn validate_token_identity_on(
        &self,
        db: &DatabaseConnection,
        claims: &Claims,
    ) -> AppResult<ValidatedIdentity> {
        ryframe_core::validate_explicit_tenant(&claims.tenant_id)?;
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
            .find_by_id(db, &claims.tenant_id, user_id)
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
        self.load_authorization_profile_on(self.db.write(), tenant_id, user_id)
            .await
    }

    pub(super) async fn load_authorization_profile_on(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<AuthorizationProfile> {
        let roles = self
            .role_repo
            .find_user_roles(db, tenant_id, user_id)
            .await?;
        let is_super_admin = roles.iter().any(|role| role.is_super == 1);
        let permissions = if is_super_admin {
            vec!["*:*:*".to_owned()]
        } else {
            self.load_permission_codes_on(db, tenant_id, &roles).await?
        };

        Ok(AuthorizationProfile { roles, permissions })
    }

    pub(super) async fn build_user_info(
        &self,
        tenant_name: &str,
        user: &user::Model,
        authorization: &AuthorizationProfile,
    ) -> AppResult<UserInfo> {
        let mut user_info = UserInfo::from(user);
        user_info.tenant_name = tenant_name.to_owned();
        user_info.dept_name = match user.dept_id {
            Some(dept_id) => self
                .dept_repo
                .find_by_id(self.db.write(), &user.tenant_id, dept_id)
                .await?
                .map(|dept| dept.name),
            None => None,
        };
        user_info.roles = authorization
            .roles
            .iter()
            .map(|role| role.code.clone())
            .collect();
        user_info.perms = authorization.permissions.clone();
        Ok(user_info)
    }

    async fn load_permission_codes_on(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        roles: &[role::Model],
    ) -> AppResult<Vec<String>> {
        let role_ids = roles.iter().map(|role| role.id).collect::<Vec<_>>();
        let mut codes = self
            .perm_repo
            .find_role_perms(db, tenant_id, &role_ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect::<Vec<_>>();
        codes.sort();
        codes.dedup();
        Ok(codes)
    }
}
