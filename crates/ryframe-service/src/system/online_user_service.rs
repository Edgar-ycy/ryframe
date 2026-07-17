use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

/// Redis key 前缀
const ONLINE_USER_KEY_PREFIX: &str = "online_user:";
/// 在线用户默认超时时间（30 分钟）
const DEFAULT_TIMEOUT_MINUTES: i64 = 30;

fn online_user_key(tenant_id: &str, token_id: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:{token_id}")
}

fn tenant_online_user_pattern(tenant_id: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:*")
}

/// 在线用户信息（DTO）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OnlineUserVo {
    pub token_id: String,
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
    pub token_id: String,
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
        match self {
            Self::Redis { client } => {
                let key = online_user_key(&session.tenant_id, &session.token_id);
                let json = match serde_json::to_string(&session) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!("序列化在线用户失败: {}", e);
                        return;
                    }
                };
                // 设置 30 分钟 TTL，每次 touch 时续期
                let ttl = DEFAULT_TIMEOUT_MINUTES * 60;
                if let Err(e) = client.set_ex(&key, &json, ttl as u64).await {
                    tracing::error!("Redis SET 在线用户失败: {}", e);
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                s.insert(
                    online_user_key(&session.tenant_id, &session.token_id),
                    session,
                );
            }
        }
    }

    /// 移除在线用户
    pub async fn remove_user(&self, tenant_id: &str, token_id: &str) {
        let key = online_user_key(tenant_id, token_id);
        match self {
            Self::Redis { client } => {
                if let Err(e) = client.del(&key).await {
                    tracing::error!("Redis DEL 在线用户失败: {}", e);
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                s.remove(&key);
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
            Self::Redis { client } => {
                let pattern = tenant_online_user_pattern(tenant_id);
                let keys = client.scan_keys(&pattern).await.map_err(|error| {
                    tracing::error!("Redis SCAN 在线用户失败: {}", error);
                    AppError::Internal("查询在线用户失败".into())
                })?;

                let mut users = Vec::new();
                for key in keys {
                    match client.get(&key).await {
                        Ok(Some(json)) => {
                            let session =
                                serde_json::from_str::<UserSession>(&json).map_err(|error| {
                                    tracing::error!("反序列化在线用户失败: {}", error);
                                    AppError::Internal("在线用户数据损坏".into())
                                })?;
                            users.push(session_to_vo(&session));
                        }
                        Ok(None) => {} // 已过期
                        Err(e) => {
                            tracing::error!("Redis GET 在线用户 {} 失败: {}", key, e);
                            return Err(AppError::Internal("查询在线用户失败".into()));
                        }
                    }
                }
                Ok(users)
            }
            Self::InMemory { sessions } => {
                let s = sessions.read().await;
                Ok(s.values()
                    .filter(|session| session.tenant_id == tenant_id)
                    .map(session_to_vo)
                    .collect())
            }
        }
    }

    /// 强制下线用户
    /// 返回被下线会话，用于后续的 Token 黑名单等操作。
    pub async fn force_logout(
        &self,
        actor: &ActorContext,
        token_id: &str,
    ) -> AppResult<UserSession> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let key = online_user_key(tenant_id, token_id);
        match self {
            Self::Redis { client } => {
                let session = match client.get(&key).await {
                    Ok(Some(json)) => {
                        serde_json::from_str::<UserSession>(&json).map_err(|error| {
                            tracing::error!("反序列化在线用户失败: {}", error);
                            AppError::Internal("在线用户数据损坏".into())
                        })?
                    }
                    Ok(None) => return Err(AppError::NotFound("在线用户不存在".into())),
                    Err(error) => {
                        tracing::error!("Redis GET 强制下线失败: {}", error);
                        return Err(AppError::Internal("强制下线失败".into()));
                    }
                };
                match client.del(&key).await {
                    Ok(n) if n > 0 => Ok(session),
                    Ok(_) => Err(AppError::NotFound("在线用户不存在".into())),
                    Err(e) => {
                        tracing::error!("Redis DEL 强制下线失败: {}", e);
                        Err(AppError::Internal("强制下线失败".into()))
                    }
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                s.remove(&key)
                    .ok_or_else(|| AppError::NotFound("在线用户不存在".into()))
            }
        }
    }

    /// 更新用户最后访问时间
    pub async fn touch_user(&self, tenant_id: &str, token_id: &str) {
        let key = online_user_key(tenant_id, token_id);
        match self {
            Self::Redis { client } => {
                match client.get(&key).await {
                    Ok(Some(json)) => {
                        if let Ok(mut session) = serde_json::from_str::<UserSession>(&json) {
                            session.last_access_time = Utc::now();
                            if let Ok(new_json) = serde_json::to_string(&session) {
                                let ttl = DEFAULT_TIMEOUT_MINUTES * 60;
                                if let Err(e) = client.set_ex(&key, &new_json, ttl as u64).await {
                                    tracing::warn!("Redis SET 续期失败: {}", e);
                                }
                            }
                        }
                    }
                    Ok(None) => {} // 已过期，忽略
                    Err(e) => {
                        tracing::warn!("Redis GET touch_user 失败: {}", e);
                    }
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                if let Some(session) = s.get_mut(&key) {
                    session.last_access_time = Utc::now();
                }
            }
        }
    }

    /// 确保在线用户会话存在（touch + 自动补建）
    ///
    /// 先尝试更新 last_access_time，如果会话不存在（如服务重启后被 clear_all_on_startup 清除），
    /// 则自动重新创建会话。解决 JWT 仍有效但 Redis 会话丢失导致在线用户列表为空的问题。
    pub async fn ensure_user(&self, session: UserSession) {
        match self {
            Self::Redis { client } => {
                let key = online_user_key(&session.tenant_id, &session.token_id);
                // 先尝试 touch：如果 key 存在则更新 last_access_time
                if let Ok(Some(json)) = client.get(&key).await {
                    if let Ok(mut existing) = serde_json::from_str::<UserSession>(&json) {
                        existing.last_access_time = Utc::now();
                        if let Ok(new_json) = serde_json::to_string(&existing) {
                            let ttl = DEFAULT_TIMEOUT_MINUTES * 60;
                            if let Err(error) = client.set_ex(&key, &new_json, ttl as u64).await {
                                tracing::warn!(%error, "Redis SET ensure_user 续期失败");
                            }
                        }
                    }
                    return;
                } // key 不存在或读取失败，走下方补建逻辑
                // 会话不存在，重新创建
                self.add_user(session).await;
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                let key = online_user_key(&session.tenant_id, &session.token_id);
                if let Some(existing) = s.get_mut(&key) {
                    existing.last_access_time = Utc::now();
                } else {
                    s.insert(key, session);
                }
            }
        }
    }

    /// 清理过期的在线用户（仅内存模式有效，Redis 模式由 TTL 自动管理）
    pub async fn cleanup_expired(&self, timeout_minutes: i64) {
        if let Self::InMemory { sessions } = self {
            let now = Utc::now();
            let mut s = sessions.write().await;
            s.retain(|_, session| {
                let duration = now.signed_duration_since(session.last_access_time);
                duration.num_minutes() < timeout_minutes
            });
        }
    }

    /// 获取在线用户数量
    pub async fn count(&self, actor: &ActorContext) -> AppResult<usize> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        match self {
            Self::Redis { client } => {
                let pattern = tenant_online_user_pattern(tenant_id);
                match client.scan_keys(&pattern).await {
                    Ok(keys) => Ok(keys.len()),
                    Err(error) => {
                        tracing::error!("Redis SCAN 在线用户失败: {}", error);
                        Err(AppError::Internal("查询在线用户数量失败".into()))
                    }
                }
            }
            Self::InMemory { sessions } => {
                let s = sessions.read().await;
                Ok(s.values()
                    .filter(|session| session.tenant_id == tenant_id)
                    .count())
            }
        }
    }

    /// 启动时清理所有在线用户（Redis 模式下清除残留的旧会话）
    pub async fn clear_all_on_startup(&self) {
        match self {
            Self::Redis { client } => {
                let pattern = format!("{}*", ONLINE_USER_KEY_PREFIX);
                match client.delete_by_pattern(&pattern).await {
                    Ok(deleted) => {
                        if deleted > 0 {
                            tracing::info!("启动时清理了 {} 个残留在线用户会话", deleted);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("清理残留在线用户会话失败: {}", e);
                    }
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                s.clear();
            }
        }
    }
}

/// UserSession → OnlineUserVo
pub fn session_to_vo(s: &UserSession) -> OnlineUserVo {
    OnlineUserVo {
        token_id: s.token_id.clone(),
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
