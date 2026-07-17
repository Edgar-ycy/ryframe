use ryframe_auth::{jwt, password};
use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::Repository;
use ryframe_db::{TenantRepository, entities::user};

use super::{AuthService, LoginResult, UserInfo};

impl AuthService {
    /// 用户登录
    ///
    /// 验证用户名密码，装载角色权限，签发双 token 并返回用户信息。
    /// 用户名或密码错误统一返回同一消息，防止用户枚举攻击。
    pub async fn login(
        &self,
        tenant_id: &str,
        username: &str,
        password: &str,
    ) -> AppResult<LoginResult> {
        ryframe_core::validate_explicit_tenant(tenant_id)?;
        let tenant = TenantRepository
            .ensure_available(self.db.write(), tenant_id)
            .await?;
        let user = self
            .user_repo
            .find_by_username(self.db.write(), tenant_id, username)
            .await?
            .ok_or_else(|| AppError::Authentication("用户名或密码错误".into()))?;

        if !password::verify(password, &user.password_hash)? {
            return Err(AppError::Authentication("用户名或密码错误".into()));
        }
        if !user.is_enabled() {
            return Err(AppError::Authentication("账号已停用或锁定".into()));
        }

        let authorization = self
            .load_authorization_profile(tenant_id, user.id, false)
            .await?;
        let user_info = self
            .build_user_info(&tenant.name, &user, &authorization)
            .await?;
        self.issue_login_result(&user, tenant.session_version, user_info)
    }

    /// 使用刷新令牌重新装载权限并签发一组新令牌。
    pub async fn refresh_token(&self, token: &str) -> AppResult<LoginResult> {
        let claims = jwt::decode_token(token, &self.config.auth.jwt_secret)?;
        if claims.token_type != "refresh" {
            return Err(AppError::Authentication(
                "令牌类型错误，请使用刷新令牌".into(),
            ));
        }

        let identity = self.validate_token_identity(&claims).await?;
        let authorization = self
            .load_authorization_profile(&identity.user.tenant_id, identity.user.id, false)
            .await?;
        let user_info = self
            .build_user_info(&identity.tenant.name, &identity.user, &authorization)
            .await?;
        self.issue_login_result(&identity.user, identity.tenant.session_version, user_info)
    }

    /// 获取当前用户信息。
    pub async fn get_current_user(&self, actor: &ActorContext) -> AppResult<UserInfo> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let tenant = TenantRepository
            .ensure_available(self.db.write(), tenant_id)
            .await?;
        let user = self
            .user_repo
            .find_by_id(self.db.write(), tenant_id, actor.user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;
        if !user.is_enabled() {
            return Err(AppError::Authentication("账号已停用或锁定".into()));
        }

        let authorization = self
            .load_authorization_profile(tenant_id, user.id, false)
            .await?;
        self.build_user_info(&tenant.name, &user, &authorization)
            .await
    }

    fn issue_login_result(
        &self,
        user: &user::Model,
        tenant_session_version: i32,
        user_info: UserInfo,
    ) -> AppResult<LoginResult> {
        let identity = jwt::TokenIdentity {
            user_id: user.id,
            tenant_id: &user.tenant_id,
            tenant_session_version,
            user_auth_version: user.auth_version,
            username: &user.username,
        };
        let (access_token, token_id) = jwt::encode_access(&identity, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh(&identity, &self.config.auth)?;

        Ok(LoginResult {
            access_token,
            refresh_token,
            token_id,
            user_info,
        })
    }
}
