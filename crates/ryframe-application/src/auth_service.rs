use std::sync::Arc;

use ryframe_auth::jwt::TokenSettings;
use serde::Serialize;

use crate::{
    AuthPolicy, AuthorizationCache, AuthorizationResolver,
    ports::auth::{IdentityAuthorizationReadPort, IdentityUserRecord},
    ports::auth::{LoginProtectionPort, RefreshSessionPort},
};

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
    /// 后端内部使用的精确用户 ID，避免从公开字符串 DTO 反向解析。
    pub user_id: i64,
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
    /// 仅来自角色表的显式超级管理员标记，不根据角色编码或名称推断。
    pub is_super_admin: bool,
    pub roles: Vec<String>,
    pub perms: Vec<String>,
}

impl From<&IdentityUserRecord> for UserInfo {
    fn from(user: &IdentityUserRecord) -> Self {
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
            is_super_admin: false,
            roles: Vec::new(),
            perms: Vec::new(),
        }
    }
}

/// 认证服务
pub struct AuthService {
    authorization_resolver: AuthorizationResolver,
    policy: AuthPolicy,
    token_settings: Arc<TokenSettings>,
    login_protection: Arc<dyn LoginProtectionPort>,
    refresh_sessions: Arc<dyn RefreshSessionPort>,
    authorization_cache: AuthorizationCache,
}

impl AuthService {
    pub fn new(
        identity_read: Arc<dyn IdentityAuthorizationReadPort>,
        policy: AuthPolicy,
        token_settings: Arc<TokenSettings>,
        login_protection: Arc<dyn LoginProtectionPort>,
        refresh_sessions: Arc<dyn RefreshSessionPort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        ryframe_auth::password::warm_dummy_hash();
        Self {
            authorization_resolver: AuthorizationResolver::new(identity_read),
            policy,
            token_settings,
            login_protection,
            refresh_sessions,
            authorization_cache,
        }
    }

    pub fn refresh_sessions(&self) -> Arc<dyn RefreshSessionPort> {
        Arc::clone(&self.refresh_sessions)
    }

    pub fn token_settings(&self) -> Arc<TokenSettings> {
        Arc::clone(&self.token_settings)
    }
}
