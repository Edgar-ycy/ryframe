use std::sync::Arc;

use ryframe_config::AppConfig;
use ryframe_core::{RedisClient, RefreshSessionStore};
use ryframe_db::DatabaseCluster;
use ryframe_db::{
    DeptRepository, PermissionRepository, RoleRepository, UserRepository, entities::user,
};
use serde::Serialize;

use crate::AuthorizationCache;

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
#[derive(Debug, Clone, Serialize)]
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
    pub preferred_locale: Option<String>,
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
            preferred_locale: user.preferred_locale.clone(),
            roles: Vec::new(),
            perms: Vec::new(),
        }
    }
}

/// 认证服务
pub struct AuthService {
    db: DatabaseCluster,
    user_repo: UserRepository,
    role_repo: RoleRepository,
    perm_repo: PermissionRepository,
    dept_repo: DeptRepository,
    config: Arc<AppConfig>,
    /// Redis 客户端（用于 refresh family 与登录暴力破解防护，可空）
    redis: Option<RedisClient>,
    refresh_sessions: RefreshSessionStore,
    authorization_cache: AuthorizationCache,
}

impl AuthService {
    pub fn new(
        db: DatabaseCluster,
        config: Arc<AppConfig>,
        redis: Option<RedisClient>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        ryframe_auth::password::warm_dummy_hash();
        let refresh_sessions = RefreshSessionStore::new(redis.clone());
        Self {
            db,
            user_repo: UserRepository,
            role_repo: RoleRepository,
            perm_repo: PermissionRepository,
            dept_repo: DeptRepository,
            config,
            redis,
            refresh_sessions,
            authorization_cache,
        }
    }

    pub fn refresh_sessions(&self) -> RefreshSessionStore {
        self.refresh_sessions.clone()
    }
}
