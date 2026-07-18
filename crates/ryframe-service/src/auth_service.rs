use std::sync::Arc;

use ryframe_config::AppConfig;
use ryframe_core::{LoggedRepo, RedisClient, RefreshSessionStore};
use ryframe_db::DatabaseCluster;
use ryframe_db::{
    DeptRepository, PermissionRepository, RoleRepository, UserRepository, entities::user,
};
use serde::Serialize;
use utoipa::ToSchema;

mod brute_force;
mod identity;
mod principal_resolution;
mod session;

/// 登录响应（内部使用，最终由 API 层序列化为 JSON）
pub struct LoginResult {
    pub access_token: String,
    pub refresh_token: String,
    /// 令牌唯一标识，用于在线用户管理
    pub sid: String,
    pub user_info: UserInfo,
    pub expires_in: usize,
    pub refresh_expires_at: usize,
}

/// 用户信息
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UserInfo {
    /// id 使用 String 避免 Snowflake 64 位 ID 超出 JS Number.MAX_SAFE_INTEGER
    pub id: String,
    pub tenant_id: String,
    pub tenant_name: String,
    pub dept_name: Option<String>,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: Option<String>,
    pub roles: Vec<String>,
    pub perms: Vec<String>,
}

impl From<&user::Model> for UserInfo {
    fn from(user: &user::Model) -> Self {
        Self {
            id: user.id.to_string(),
            tenant_id: user.tenant_id.clone(),
            tenant_name: String::new(),
            dept_name: None,
            username: user.username.clone(),
            nickname: user.nickname.clone(),
            email: user.email.clone(),
            phone: user.phone.clone(),
            avatar: user.avatar.clone(),
            roles: Vec::new(),
            perms: Vec::new(),
        }
    }
}

/// 认证服务
pub struct AuthService {
    db: DatabaseCluster,
    user_repo: LoggedRepo<UserRepository>,
    role_repo: LoggedRepo<RoleRepository>,
    perm_repo: LoggedRepo<PermissionRepository>,
    dept_repo: LoggedRepo<DeptRepository>,
    config: Arc<AppConfig>,
    /// Redis 客户端（用于 refresh family 与登录暴力破解防护，可空）
    redis: Option<RedisClient>,
    refresh_sessions: RefreshSessionStore,
}

impl AuthService {
    pub fn new(db: DatabaseCluster, config: Arc<AppConfig>, redis: Option<RedisClient>) -> Self {
        let refresh_sessions = RefreshSessionStore::new(redis.clone());
        Self {
            db,
            user_repo: LoggedRepo::new(UserRepository),
            role_repo: LoggedRepo::new(RoleRepository),
            perm_repo: LoggedRepo::new(PermissionRepository),
            dept_repo: LoggedRepo::new(DeptRepository),
            config,
            redis,
            refresh_sessions,
        }
    }

    pub fn refresh_sessions(&self) -> RefreshSessionStore {
        self.refresh_sessions.clone()
    }
}
