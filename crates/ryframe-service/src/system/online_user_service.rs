use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use ryframe_common::{AppError, AppResult};
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Redis key 前缀
const ONLINE_USER_KEY_PREFIX: &str = "online_user:";
/// 在线用户默认超时时间（30 分钟）
const DEFAULT_TIMEOUT_MINUTES: i64 = 30;

/// 在线用户信息（DTO）
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub enum OnlineUserServiceImpl {
    /// Redis 存储（生产推荐，支持分布式部署）
    Redis { client: Box<RedisClient> },
    /// 内存存储（开发/降级模式）
    InMemory {
        sessions: Arc<RwLock<HashMap<String, UserSession>>>,
    },
}

impl Default for OnlineUserServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl OnlineUserServiceImpl {
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

    /// 兼容旧 API
    pub fn new() -> Self {
        Self::new_in_memory()
    }

    /// 添加在线用户
    pub async fn add_user(&self, session: UserSession) {
        match self {
            Self::Redis { client } => {
                let key = format!("{}{}", ONLINE_USER_KEY_PREFIX, session.token_id);
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
                s.insert(session.token_id.clone(), session);
            }
        }
    }

    /// 移除在线用户
    pub async fn remove_user(&self, token_id: &str) {
        match self {
            Self::Redis { client } => {
                let key = format!("{}{}", ONLINE_USER_KEY_PREFIX, token_id);
                if let Err(e) = client.del(&key).await {
                    tracing::error!("Redis DEL 在线用户失败: {}", e);
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                s.remove(token_id);
            }
        }
    }

    /// 获取所有在线用户列表
    pub async fn list_online_users(&self) -> Vec<OnlineUserVo> {
        match self {
            Self::Redis { client } => {
                let pattern = format!("{}*", ONLINE_USER_KEY_PREFIX);
                let keys = match client.keys(&pattern).await {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::error!("Redis KEYS 在线用户失败: {}", e);
                        return vec![];
                    }
                };

                let mut users = Vec::new();
                for key in keys {
                    match client.get(&key).await {
                        Ok(Some(json)) => {
                            if let Ok(session) = serde_json::from_str::<UserSession>(&json) {
                                users.push(session_to_vo(&session));
                            }
                        }
                        Ok(None) => {} // 已过期
                        Err(e) => {
                            tracing::warn!("Redis GET 在线用户 {} 失败: {}", key, e);
                        }
                    }
                }
                users
            }
            Self::InMemory { sessions } => {
                let s = sessions.read().await;
                s.values().map(session_to_vo).collect()
            }
        }
    }

    /// 强制下线用户
    /// 返回被下线用户的 user_id，用于后续的 Token 黑名单等操作。
    pub async fn force_logout(&self, token_id: &str) -> AppResult<i64> {
        match self {
            Self::Redis { client } => {
                let key = format!("{}{}", ONLINE_USER_KEY_PREFIX, token_id);
                // 先读取会话获取 user_id（删除前）
                let user_id = match client.get(&key).await {
                    Ok(Some(json)) => serde_json::from_str::<UserSession>(&json)
                        .map(|s| s.user_id)
                        .unwrap_or(0),
                    _ => 0,
                };
                // 再删除
                match client.del(&key).await {
                    Ok(n) if n > 0 => Ok(user_id),
                    Ok(_) => Err(AppError::NotFound("在线用户不存在".into())),
                    Err(e) => {
                        tracing::error!("Redis DEL 强制下线失败: {}", e);
                        Err(AppError::Internal("强制下线失败".into()))
                    }
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                let user_id = s.get(token_id).map(|s| s.user_id).unwrap_or(0);
                if s.remove(token_id).is_some() {
                    Ok(user_id)
                } else {
                    Err(AppError::NotFound("在线用户不存在".into()))
                }
            }
        }
    }

    /// 更新用户最后访问时间
    pub async fn touch_user(&self, token_id: &str) {
        match self {
            Self::Redis { client } => {
                let key = format!("{}{}", ONLINE_USER_KEY_PREFIX, token_id);
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
                if let Some(session) = s.get_mut(token_id) {
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
                let key = format!("{}{}", ONLINE_USER_KEY_PREFIX, session.token_id);
                // 先尝试 touch：如果 key 存在则更新 last_access_time
                match client.get(&key).await {
                    Ok(Some(json)) => {
                        if let Ok(mut existing) = serde_json::from_str::<UserSession>(&json) {
                            existing.last_access_time = Utc::now();
                            if let Ok(new_json) = serde_json::to_string(&existing) {
                                let ttl = DEFAULT_TIMEOUT_MINUTES * 60;
                                let _ = client.set_ex(&key, &new_json, ttl as u64).await;
                            }
                        }
                        return;
                    }
                    _ => {} // key 不存在或读取失败，走下方补建逻辑
                }
                // 会话不存在，重新创建
                self.add_user(session).await;
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                if let Some(existing) = s.get_mut(&session.token_id) {
                    existing.last_access_time = Utc::now();
                } else {
                    s.insert(session.token_id.clone(), session);
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
    pub async fn count(&self) -> usize {
        match self {
            Self::Redis { client } => {
                let pattern = format!("{}*", ONLINE_USER_KEY_PREFIX);
                match client.keys(&pattern).await {
                    Ok(keys) => keys.len(),
                    Err(_) => 0,
                }
            }
            Self::InMemory { sessions } => {
                let s = sessions.read().await;
                s.len()
            }
        }
    }

    /// 启动时清理所有在线用户（Redis 模式下清除残留的旧会话）
    pub async fn clear_all_on_startup(&self) {
        match self {
            Self::Redis { client } => {
                let pattern = format!("{}*", ONLINE_USER_KEY_PREFIX);
                match client.keys(&pattern).await {
                    Ok(keys) => {
                        let count = keys.len();
                        for key in &keys {
                            let _ = client.del(key).await;
                        }
                        if count > 0 {
                            tracing::info!("启动时清理了 {} 个残留在线用户会话", count);
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
