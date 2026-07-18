use ryframe_auth::{jwt, password};
use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::{RefreshFamily, RefreshRotation, Repository};
use ryframe_db::{TenantRepository, entities::user};

use super::{AuthService, LoginResult, UserInfo};

const MAX_REFRESH_SESSION_SECONDS: usize = 7 * 24 * 60 * 60;

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

        let authorization = self.load_authorization_profile(tenant_id, user.id).await?;
        let user_info = self
            .build_user_info(&tenant.name, &user, &authorization)
            .await?;
        self.issue_login_result(&user, tenant.session_version, user_info)
            .await
    }

    /// 使用刷新令牌重新装载权限并签发一组新令牌。
    pub async fn refresh_token(
        &self,
        token: &str,
        rotation_attempt_id: &str,
    ) -> AppResult<LoginResult> {
        let claims = jwt::decode_token(token, &self.config.auth.jwt_secret)?;
        if claims.token_type != "refresh" {
            return Err(AppError::Authentication(
                "令牌类型错误，请使用刷新令牌".into(),
            ));
        }

        let identity = self.validate_token_identity(&claims).await?;
        let authorization = self
            .load_authorization_profile(&identity.user.tenant_id, identity.user.id)
            .await?;
        let user_info = self
            .build_user_info(&identity.tenant.name, &identity.user, &authorization)
            .await?;
        self.issue_refresh_result(
            &identity.user,
            identity.tenant.session_version,
            user_info,
            &claims,
            rotation_attempt_id,
        )
        .await
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

        let authorization = self.load_authorization_profile(tenant_id, user.id).await?;
        self.build_user_info(&tenant.name, &user, &authorization)
            .await
    }

    async fn issue_login_result(
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
        let sid = jwt::new_sid();
        let refresh_jti = jwt::generate_jti();
        let now = chrono::Utc::now().timestamp() as usize;
        let expires_in = jwt::parse_duration(&self.config.auth.access_token_expire)?;
        let refresh_ttl = jwt::parse_duration(&self.config.auth.refresh_token_expire)?
            .min(MAX_REFRESH_SESSION_SECONDS);
        let refresh_expires_at = now
            .checked_add(refresh_ttl)
            .ok_or_else(|| AppError::Config("refresh token expiry is too large".into()))?;
        let (access_token, _) = jwt::encode_access_for_session(&identity, &sid, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh_for_session(
            &identity,
            &sid,
            refresh_jti.clone(),
            refresh_expires_at,
            &self.config.auth,
        )?;
        self.refresh_sessions
            .register(RefreshFamily {
                sid: sid.clone(),
                tenant_id: user.tenant_id.clone(),
                user_id: user.id,
                current_jti: refresh_jti,
                previous_jti: None,
                last_attempt_id: None,
                rotated_at: now as i64,
                absolute_exp: refresh_expires_at as i64,
                revoked: false,
            })
            .await?;

        Ok(LoginResult {
            access_token,
            refresh_token,
            sid,
            user_info,
            expires_in,
            refresh_expires_at,
        })
    }

    async fn issue_refresh_result(
        &self,
        user: &user::Model,
        tenant_session_version: i32,
        user_info: UserInfo,
        claims: &jwt::Claims,
        rotation_attempt_id: &str,
    ) -> AppResult<LoginResult> {
        if claims.sid.is_empty() {
            return Err(AppError::Authentication(
                "legacy refresh token is not accepted".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        if claims.exp <= now as usize {
            return Err(AppError::Authentication("refresh token expired".into()));
        }
        let identity = jwt::TokenIdentity {
            user_id: user.id,
            tenant_id: &user.tenant_id,
            tenant_session_version,
            user_auth_version: user.auth_version,
            username: &user.username,
        };
        let expires_in = jwt::parse_duration(&self.config.auth.access_token_expire)?;
        let proposed_jti = jwt::generate_jti();
        let (committed_jti, issued_at) = match self
            .refresh_sessions
            .rotate(
                &claims.sid,
                &claims.jti,
                &proposed_jti,
                now,
                rotation_attempt_id,
            )
            .await?
        {
            RefreshRotation::Rotated {
                current_jti,
                issued_at,
            }
            | RefreshRotation::Recovered {
                current_jti,
                issued_at,
            } => (current_jti, issued_at),
            RefreshRotation::Concurrent => {
                return Err(AppError::Conflict("refresh already in progress".into()));
            }
            RefreshRotation::Replayed => {
                return Err(AppError::Authentication(
                    "refresh token replay detected; session revoked".into(),
                ));
            }
            RefreshRotation::MissingOrRevoked => {
                return Err(AppError::Authentication(
                    "refresh session is not active".into(),
                ));
            }
        };
        let (access_token, _) =
            jwt::encode_access_for_session(&identity, &claims.sid, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh_for_session_at(
            &identity,
            &claims.sid,
            committed_jti,
            issued_at.max(0) as usize,
            claims.exp,
            &self.config.auth,
        )?;
        Ok(LoginResult {
            access_token,
            refresh_token,
            sid: claims.sid.clone(),
            user_info,
            expires_in,
            refresh_expires_at: claims.exp,
        })
    }
}
