use ryframe_auth::{jwt, password};
use ryframe_common::{AppError, AppResult};
use ryframe_config::AppConfig;
use ryframe_db::entities::user;
use ryframe_db::{PermissionRepository, RoleRepository, UserRepository};
use sea_orm::DatabaseConnection;
use serde::Serialize;
use std::sync::Arc;
use ryframe_core::Repository;
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
    pub id: i64,
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
            id: u.id,
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
    pub user_repo: UserRepository,
    pub role_repo: RoleRepository,
    pub perm_repo: PermissionRepository,
    pub config: Arc<AppConfig>,
}

impl AuthServiceImpl {
    /// 用户登录
    ///
    /// 验证用户名密码 → 查询角色权限 → 签发双 token → 返回用户信息和令牌。
    /// 用户名或密码错误统一返回 "用户名或密码错误"，防止用户枚举攻击。
    pub async fn login(
        &self,
        db: &DatabaseConnection,
        username: &str,
        password: &str,
    ) -> AppResult<LoginResult> {
        let user = self
            .user_repo
            .find_by_username(db, username)
            .await?
            .ok_or_else(|| AppError::Authentication("用户名或密码错误".into()))?;

        let valid = password::verify(password, &user.password_hash)?;
        if !valid {
            return Err(AppError::Authentication("用户名或密码错误".into()));
        }

        let roles = self.role_repo.find_user_roles(db, user.id).await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

        let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
        let perm_codes: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        let (access_token, token_id) =
            jwt::encode_access(user.id, &user.username, &role_codes, &perm_codes, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh(user.id, &user.username, &self.config.auth)?;

        let mut user_info = UserInfo::from(&user);
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
            return Err(AppError::Authentication("令牌类型错误，请使用刷新令牌".into()));
        }

        let user_id = claims.sub.parse::<i64>()
            .map_err(|_| AppError::Authentication("令牌中的用户ID无效".into()))?;

        let user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;

        let roles = self.role_repo.find_user_roles(db, user.id).await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

        let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
        let perm_codes: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        let (access_token, token_id) =
            jwt::encode_access(user.id, &user.username, &role_codes, &perm_codes, &self.config.auth)?;
        let refresh_token = jwt::encode_refresh(user.id, &user.username, &self.config.auth)?;

        let mut user_info = UserInfo::from(&user);
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
        user_id: i64,
    ) -> AppResult<UserInfo> {
        let user = self
            .user_repo
            .find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

        let roles = self.role_repo.find_user_roles(db, user.id).await?;
        let role_codes: Vec<String> = roles.iter().map(|r| r.code.clone()).collect();
        let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

        let perms = self.perm_repo.find_role_perms(db, &role_ids).await?;
        let perm_codes: Vec<String> = perms.iter().map(|p| p.code.clone()).collect();

        let mut user_info = UserInfo::from(&user);
        user_info.roles = role_codes;
        user_info.perms = perm_codes;

        Ok(user_info)
    }
}

