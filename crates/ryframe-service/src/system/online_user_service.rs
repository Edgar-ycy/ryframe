use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use ryframe_common::{ActorContext, AppResult};
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

mod keyspace;
mod memory_backend;
mod redis_backend;
mod session_codec;

use session_codec::remaining_ttl;

/// 在线用户信息（DTO）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OnlineUserVo {
    /// Stable refresh-family session identifier, not an access-token JTI.
    pub sid: String,
    pub username: String,
    pub dept_name: Option<String>,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub login_time: String,
    pub last_access_time: String,
}

/// 在线用户会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSession {
    pub sid: String,
    pub tenant_id: String,
    pub user_id: i64,
    pub username: String,
    pub dept_name: Option<String>,
    pub ipaddr: String,
    pub login_location: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub login_time: chrono::DateTime<Utc>,
    pub last_access_time: chrono::DateTime<Utc>,
    /// Absolute refresh-family expiry. The online index must never outlive
    /// the authoritative device session.
    pub absolute_exp: i64,
}

/// 在线用户管理服务（支持 Redis / 内存双模式）
#[derive(Clone)]
pub enum OnlineUserService {
    /// Redis 存储（生产推荐，支持分布式部署）
    Redis { client: Box<RedisClient> },
    /// 内存存储（开发/降级模式）
    InMemory {
        sessions: Arc<RwLock<HashMap<String, UserSession>>>,
    },
}

impl Default for OnlineUserService {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

impl OnlineUserService {
    /// 创建 Redis 模式的在线用户服务
    pub fn new_redis(client: RedisClient) -> Self {
        Self::Redis {
            client: Box::new(client),
        }
    }

    /// 创建内存模式的在线用户服务
    pub fn new_in_memory() -> Self {
        Self::InMemory {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 添加在线用户
    pub async fn add_user(&self, session: UserSession) {
        if let Err(error) = ryframe_core::validate_tenant_identifier(&session.tenant_id) {
            tracing::error!(tenant_id = %session.tenant_id, %error, "refusing invalid tenant in online-user index");
            return;
        }
        let Some(ttl) = remaining_ttl(session.absolute_exp) else {
            return;
        };
        match self {
            Self::Redis { client } => redis_backend::add(client, &session, ttl).await,
            Self::InMemory { sessions } => memory_backend::add(sessions, session).await,
        }
    }

    /// 移除在线用户
    pub async fn remove_user(&self, tenant_id: &str, sid: &str) {
        if let Err(error) = ryframe_core::validate_tenant_identifier(tenant_id) {
            tracing::error!(tenant_id, %error, "refusing invalid tenant in online-user removal");
            return;
        }
        match self {
            Self::Redis { client } => redis_backend::remove(client, tenant_id, sid).await,
            Self::InMemory { sessions } => {
                memory_backend::remove(sessions, tenant_id, sid).await;
            }
        }
    }

    /// 获取过滤后的在线用户列表
    pub async fn list_filtered(
        &self,
        actor: &ActorContext,
        username: Option<&str>,
        ipaddr: Option<&str>,
    ) -> AppResult<Vec<OnlineUserVo>> {
        let users = self.list_online_users(actor).await?;
        Ok(users
            .into_iter()
            .filter(|u| {
                if let Some(username) = username
                    && !u.username.contains(username)
                {
                    return false;
                }
                if let Some(ipaddr) = ipaddr
                    && !u.ipaddr.contains(ipaddr)
                {
                    return false;
                }
                true
            })
            .collect())
    }

    /// 获取过滤并分页的在线用户列表
    pub async fn list_filtered_page(
        &self,
        actor: &ActorContext,
        username: Option<&str>,
        ipaddr: Option<&str>,
        page: u64,
        page_size: u64,
    ) -> AppResult<(Vec<OnlineUserVo>, u64)> {
        let filtered = self.list_filtered(actor, username, ipaddr).await?;
        let total = filtered.len() as u64;
        let offset = ((page.saturating_sub(1)) * page_size) as usize;
        let rows: Vec<OnlineUserVo> = filtered
            .into_iter()
            .skip(offset)
            .take(page_size as usize)
            .collect();
        Ok((rows, total))
    }

    /// 获取所有在线用户列表
    pub async fn list_online_users(&self, actor: &ActorContext) -> AppResult<Vec<OnlineUserVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        match self {
            Self::Redis { client } => redis_backend::list(client, tenant_id).await,
            Self::InMemory { sessions } => Ok(memory_backend::list(sessions, tenant_id).await),
        }
    }

    /// 更新用户最后访问时间
    pub async fn touch_user(&self, tenant_id: &str, sid: &str) {
        if let Err(error) = ryframe_core::validate_tenant_identifier(tenant_id) {
            tracing::error!(tenant_id, %error, "refusing invalid tenant in online-user update");
            return;
        }
        match self {
            Self::Redis { client } => redis_backend::touch(client, tenant_id, sid).await,
            Self::InMemory { sessions } => {
                memory_backend::touch(sessions, tenant_id, sid).await;
            }
        }
    }

    /// 清理过期的在线用户（仅内存模式有效，Redis 模式由 TTL 自动管理）
    pub async fn cleanup_expired(&self) {
        if let Self::InMemory { sessions } = self {
            memory_backend::cleanup_expired(sessions).await;
        }
    }

    /// 获取在线用户数量
    pub async fn count(&self, actor: &ActorContext) -> AppResult<usize> {
        Ok(self.list_online_users(actor).await?.len())
    }
}

/// UserSession → OnlineUserVo
pub fn session_to_vo(s: &UserSession) -> OnlineUserVo {
    OnlineUserVo {
        sid: s.sid.clone(),
        username: s.username.clone(),
        dept_name: s.dept_name.clone(),
        ipaddr: s.ipaddr.clone(),
        login_location: s.login_location.clone(),
        browser: s.browser.clone(),
        os: s.os.clone(),
        login_time: s.login_time.to_rfc3339(),
        last_access_time: s.last_access_time.to_rfc3339(),
    }
}
