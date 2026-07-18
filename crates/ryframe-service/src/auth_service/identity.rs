use ryframe_auth::jwt::Claims;
use ryframe_common::{AppError, AppResult};
use ryframe_core::Repository;
use ryframe_db::{
    TenantRepository,
    entities::{role, tenant, user},
};

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
    pub(super) async fn validate_token_identity(
        &self,
        claims: &Claims,
    ) -> AppResult<ValidatedIdentity> {
        ryframe_core::validate_explicit_tenant(&claims.tenant_id)?;
        let tenant = TenantRepository
            .ensure_available(self.db.write(), &claims.tenant_id)
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
            .find_by_id(self.db.write(), &claims.tenant_id, user_id)
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

        Ok(ValidatedIdentity { tenant, user })
    }

    pub(super) async fn load_authorization_profile(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<AuthorizationProfile> {
        let roles = self
            .role_repo
            .find_user_roles(self.db.write(), tenant_id, user_id)
            .await?;
        let is_super_admin = roles.iter().any(|role| role.is_super == 1);
        let permissions = if is_super_admin {
            vec!["*:*:*".to_owned()]
        } else {
            self.load_permission_codes(tenant_id, &roles).await?
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

    async fn load_permission_codes(
        &self,
        tenant_id: &str,
        roles: &[role::Model],
    ) -> AppResult<Vec<String>> {
        let role_ids = roles.iter().map(|role| role.id).collect::<Vec<_>>();
        let mut codes = self
            .perm_repo
            .find_role_perms(self.db.write(), tenant_id, &role_ids)
            .await?
            .into_iter()
            .map(|permission| permission.code)
            .collect::<Vec<_>>();
        codes.sort();
        codes.dedup();
        Ok(codes)
    }
}
