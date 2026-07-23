use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use ryframe_common::{ActorContext, AppError, AppResult};
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

/// Redis key 前缀
const ONLINE_USER_KEY_PREFIX: &str = "ryframe:v0.5:online-user:";
const ONLINE_USER_MGET_BATCH_SIZE: usize = 256;

fn remaining_session_ttl(absolute_exp: i64) -> Option<u64> {
    let remaining = absolute_exp - Utc::now().timestamp();
    (remaining > 0).then_some(remaining as u64)
}

fn online_user_key(tenant_id: &str, sid: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:{sid}")
}

fn tenant_online_user_pattern(tenant_id: &str) -> String {
    format!("{ONLINE_USER_KEY_PREFIX}{tenant_id}:*")
}

fn decode_online_users(
    expected_tenant_id: &str,
    keys: &[String],
    values: Vec<Option<String>>,
) -> AppResult<Vec<OnlineUserVo>> {
    if keys.len() != values.len() {
        tracing::error!(
            key_count = keys.len(),
            value_count = values.len(),
            "Redis MGET 在线用户返回数量异常"
        );
        return Err(AppError::Internal("查询在线用户失败".into()));
    }

    let mut users = Vec::with_capacity(keys.len());
    for (key, value) in keys.iter().zip(values) {
        let Some(json) = value else {
            continue;
        };
        let session = serde_json::from_str::<UserSession>(&json).map_err(|error| {
            tracing::error!(%error, %key, "反序列化在线用户失败");
            AppError::Internal("在线用户数据损坏".into())
        })?;
        if session.tenant_id != expected_tenant_id
            || key != &online_user_key(expected_tenant_id, &session.sid)
        {
            tracing::warn!(
                %key,
                expected_tenant_id,
                session_tenant_id = session.tenant_id,
                "ignored an online-user index outside the requested tenant"
            );
            continue;
        }
        if remaining_session_ttl(session.absolute_exp).is_some() {
            users.push(session_to_vo(&session));
        }
    }
    Ok(users)
}

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
        let Some(ttl) = remaining_session_ttl(session.absolute_exp) else {
            return;
        };
        match self {
            Self::Redis { client } => {
                let key = online_user_key(&session.tenant_id, &session.sid);
                let json = match serde_json::to_string(&session) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!("序列化在线用户失败: {}", e);
                        return;
                    }
                };
                if let Err(e) = client.set_ex(&key, &json, ttl).await {
                    tracing::error!("Redis SET 在线用户失败: {}", e);
                }
            }
            Self::InMemory { sessions } => {
                let mut s = sessions.write().await;
                s.insert(online_user_key(&session.tenant_id, &session.sid), session);
            }
        }
    }

    /// 移除在线用户
    pub async fn remove_user(&self, tenant_id: &str, sid: &str) {
        if let Err(error) = ryframe_core::validate_tenant_identifier(tenant_id) {
            tracing::error!(tenant_id, %error, "refusing invalid tenant in online-user removal");
            return;
        }
        let key = online_user_key(tenant_id, sid);
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
                let mut users = Vec::with_capacity(keys.len());
                for key_batch in keys.chunks(ONLINE_USER_MGET_BATCH_SIZE) {
                    // MGET 与 keys 顺序一致；扫描后已过期的 key 返回 None 并被跳过。
                    let values = client.mget(key_batch).await.map_err(|error| {
                        tracing::error!("Redis MGET 在线用户失败: {}", error);
                        AppError::Internal("查询在线用户失败".into())
                    })?;
                    users.extend(decode_online_users(tenant_id, key_batch, values)?);
                }
                Ok(users)
            }
            Self::InMemory { sessions } => {
                let s = sessions.read().await;
                Ok(s.values()
                    .filter(|session| {
                        session.tenant_id == tenant_id
                            && remaining_session_ttl(session.absolute_exp).is_some()
                    })
                    .map(session_to_vo)
                    .collect())
            }
        }
    }

    /// 更新用户最后访问时间
    pub async fn touch_user(&self, tenant_id: &str, sid: &str) {
        if let Err(error) = ryframe_core::validate_tenant_identifier(tenant_id) {
            tracing::error!(tenant_id, %error, "refusing invalid tenant in online-user update");
            return;
        }
        let key = online_user_key(tenant_id, sid);
        match self {
            Self::Redis { client } => {
                match client.get(&key).await {
                    Ok(Some(json)) => {
                        if let Ok(mut session) = serde_json::from_str::<UserSession>(&json) {
                            session.last_access_time = Utc::now();
                            if let (Some(ttl), Ok(new_json)) = (
                                remaining_session_ttl(session.absolute_exp),
                                serde_json::to_string(&session),
                            ) {
                                if let Err(e) = client.set_ex(&key, &new_json, ttl).await {
                                    tracing::warn!("Redis SET 续期失败: {}", e);
                                }
                            } else if let Err(error) = client.del(&key).await {
                                tracing::warn!(%error, "删除过期在线用户索引失败");
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
                let expired = s
                    .get(&key)
                    .is_some_and(|session| remaining_session_ttl(session.absolute_exp).is_none());
                if expired {
                    s.remove(&key);
                } else if let Some(session) = s.get_mut(&key) {
                    session.last_access_time = Utc::now();
                }
            }
        }
    }

    /// 清理过期的在线用户（仅内存模式有效，Redis 模式由 TTL 自动管理）
    pub async fn cleanup_expired(&self) {
        if let Self::InMemory { sessions } = self {
            let mut s = sessions.write().await;
            s.retain(|_, session| remaining_session_ttl(session.absolute_exp).is_some());
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

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use ryframe_common::AppError;

    use super::{ONLINE_USER_MGET_BATCH_SIZE, UserSession, decode_online_users, online_user_key};

    fn session(sid: &str, username: &str) -> UserSession {
        let now = Utc::now();
        UserSession {
            sid: sid.into(),
            tenant_id: "system".into(),
            user_id: 1,
            username: username.into(),
            dept_name: None,
            ipaddr: "127.0.0.1".into(),
            login_location: None,
            browser: None,
            os: None,
            login_time: now,
            last_access_time: now,
            absolute_exp: (now + Duration::hours(1)).timestamp(),
        }
    }

    #[test]
    fn batch_decode_preserves_key_order_and_skips_expired_entries() {
        let keys = vec![
            online_user_key("system", "b"),
            online_user_key("system", "expired"),
            online_user_key("system", "a"),
        ];
        let values = vec![
            Some(serde_json::to_string(&session("b", "bob")).unwrap()),
            None,
            Some(serde_json::to_string(&session("a", "alice")).unwrap()),
        ];

        let users = decode_online_users("system", &keys, values).unwrap();

        assert_eq!(
            users
                .iter()
                .map(|user| user.username.as_str())
                .collect::<Vec<_>>(),
            ["bob", "alice"]
        );
    }

    #[test]
    fn batch_decode_rejects_corrupted_or_misaligned_data() {
        let corrupted = decode_online_users("system", &["bad".into()], vec![Some("{".into())]);
        assert!(
            matches!(corrupted, Err(AppError::Internal(message)) if message == "在线用户数据损坏")
        );

        let misaligned = decode_online_users("system", &["only-key".into()], Vec::new());
        assert!(
            matches!(misaligned, Err(AppError::Internal(message)) if message == "查询在线用户失败")
        );
    }

    #[test]
    fn batch_decode_filters_cross_tenant_and_mismatched_keys() {
        let foreign = UserSession {
            tenant_id: "tenant-b".into(),
            ..session("foreign", "mallory")
        };
        let wrong_key_session = session("actual", "eve");
        let keys = vec![
            online_user_key("system", "foreign"),
            online_user_key("system", "different"),
        ];
        let values = vec![
            Some(serde_json::to_string(&foreign).unwrap()),
            Some(serde_json::to_string(&wrong_key_session).unwrap()),
        ];

        assert!(
            decode_online_users("system", &keys, values)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn online_user_mget_batches_are_bounded() {
        let keys = (0..(ONLINE_USER_MGET_BATCH_SIZE * 2 + 1)).collect::<Vec<_>>();
        assert_eq!(
            keys.chunks(ONLINE_USER_MGET_BATCH_SIZE)
                .map(<[usize]>::len)
                .collect::<Vec<_>>(),
            [ONLINE_USER_MGET_BATCH_SIZE, ONLINE_USER_MGET_BATCH_SIZE, 1]
        );
    }
}
