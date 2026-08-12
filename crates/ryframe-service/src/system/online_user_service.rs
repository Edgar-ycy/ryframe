use std::{cmp::Reverse, collections::HashMap, sync::Arc};

use chrono::Utc;
use ryframe_core::{RedisClient, RefreshSessionStore, ValidatedPageQuery};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

mod keyspace;
mod memory_backend;
mod redis_backend;
mod session_codec;

use session_codec::remaining_ttl;

/// 在线用户信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineUserVo {
    /// 稳定的刷新令牌族会话标识，而非访问令牌 JTI。
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

/// 在线用户会话元数据。
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
    /// 刷新令牌族的绝对过期时间。展示元数据不能比权威会话存活更久。
    pub absolute_exp: i64,
}

/// 在线用户管理服务（支持 Redis / 内存双模式）。
#[derive(Clone)]
pub enum OnlineUserService {
    /// Redis 存储用于多实例部署。
    Redis {
        client: Box<RedisClient>,
        refresh_sessions: RefreshSessionStore,
    },
    /// 内存存储仅保证单进程一致性。
    InMemory {
        sessions: Arc<RwLock<HashMap<String, UserSession>>>,
        refresh_sessions: RefreshSessionStore,
    },
}

impl OnlineUserService {
    pub fn new_redis(client: RedisClient, refresh_sessions: RefreshSessionStore) -> Self {
        Self::Redis {
            client: Box::new(client),
            refresh_sessions,
        }
    }

    pub fn new_in_memory(refresh_sessions: RefreshSessionStore) -> Self {
        Self::InMemory {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            refresh_sessions,
        }
    }

    fn refresh_sessions(&self) -> &RefreshSessionStore {
        match self {
            Self::Redis {
                refresh_sessions, ..
            }
            | Self::InMemory {
                refresh_sessions, ..
            } => refresh_sessions,
        }
    }

    /// 添加会话元数据。调用方只有在此操作成功后才可以向客户端发放令牌。
    pub async fn add_user(&self, session: UserSession) -> AppResult<()> {
        ryframe_core::validate_tenant_identifier(&session.tenant_id)?;
        let identity = self
            .refresh_sessions()
            .identity(&session.sid)
            .await?
            .filter(|identity| {
                identity.tenant_id == session.tenant_id
                    && identity.user_id == session.user_id
                    && identity.absolute_exp == session.absolute_exp
            })
            .ok_or_else(|| AppError::ServiceUnavailable("无法登记不完整的登录设备会话".into()))?;
        let ttl = remaining_ttl(identity.absolute_exp)
            .ok_or_else(|| AppError::Authentication("登录设备会话已过期".into()))?;
        match self {
            Self::Redis { client, .. } => redis_backend::add(client, &session, ttl).await,
            Self::InMemory { sessions, .. } => {
                memory_backend::add(sessions, session).await;
                Ok(())
            }
        }
    }

    /// 清理展示元数据和索引。权威会话撤销必须由调用方先完成。
    pub async fn remove_user(&self, tenant_id: &str, sid: &str) -> AppResult<()> {
        ryframe_core::validate_tenant_identifier(tenant_id)?;
        match self {
            Self::Redis { client, .. } => redis_backend::remove(client, tenant_id, sid).await,
            Self::InMemory { sessions, .. } => {
                memory_backend::remove(sessions, tenant_id, sid).await;
                Ok(())
            }
        }
    }

    async fn authoritative_sessions(
        &self,
        tenant_id: &str,
        metadata: Vec<UserSession>,
        expected_user_id: Option<i64>,
    ) -> AppResult<Vec<UserSession>> {
        let mut sessions = Vec::with_capacity(metadata.len());
        for mut session in metadata {
            let identity = self.refresh_sessions().identity(&session.sid).await?;
            let Some(identity) = identity.filter(|identity| {
                identity.tenant_id == tenant_id
                    && identity.user_id == session.user_id
                    && expected_user_id.is_none_or(|user_id| identity.user_id == user_id)
            }) else {
                // 旧索引可能在升级或故障恢复后残留；列表读取负责安全收敛。
                if let Err(error) = self.remove_user(tenant_id, &session.sid).await {
                    tracing::warn!(%error, sid = %session.sid, "清理失效登录设备元数据失败");
                }
                continue;
            };
            session.absolute_exp = identity.absolute_exp;
            sessions.push(session);
        }
        sessions.sort_by_key(|session| Reverse(session.last_access_time));
        Ok(sessions)
    }

    /// 读取指定用户自己的有效设备会话。
    pub async fn list_user_sessions(
        &self,
        tenant_id: &str,
        user_id: i64,
    ) -> AppResult<Vec<UserSession>> {
        ryframe_core::validate_tenant_identifier(tenant_id)?;
        let metadata = match self {
            Self::Redis { client, .. } => {
                redis_backend::list_for_user(client, tenant_id, user_id).await?
            }
            Self::InMemory { sessions, .. } => {
                memory_backend::list_for_user(sessions, tenant_id, user_id).await
            }
        };
        // 新核心索引覆盖当前版本会话；旧 metadata SCAN 覆盖升级前最多七天的会话。
        // 缺少 metadata 的 SID 不可安全展示，也不得根据令牌族合成浏览器、IP 等信息。
        let indexed_sids = self
            .refresh_sessions()
            .session_sids_for_user(tenant_id, user_id)
            .await?;
        tracing::trace!(
            indexed_session_count = indexed_sids.len(),
            legacy_metadata_count = metadata.len(),
            "合并登录设备新索引与兼容元数据"
        );
        self.authoritative_sessions(tenant_id, metadata, Some(user_id))
            .await
    }

    pub async fn list_filtered(
        &self,
        actor: &ActorContext,
        username: Option<&str>,
        ipaddr: Option<&str>,
    ) -> AppResult<Vec<OnlineUserVo>> {
        let users = self.list_online_users(actor).await?;
        Ok(users
            .into_iter()
            .filter(|user| {
                username.is_none_or(|value| user.username.contains(value))
                    && ipaddr.is_none_or(|value| user.ipaddr.contains(value))
            })
            .collect())
    }

    pub async fn list_filtered_page(
        &self,
        actor: &ActorContext,
        username: Option<&str>,
        ipaddr: Option<&str>,
        page: ValidatedPageQuery,
    ) -> AppResult<(Vec<OnlineUserVo>, u64)> {
        let filtered = self.list_filtered(actor, username, ipaddr).await?;
        let total = filtered.len() as u64;
        let offset = usize::try_from(page.offset())
            .map_err(|_| AppError::Validation("分页偏移量超出当前平台范围".into()))?;
        let page_size = usize::try_from(page.page_size())
            .map_err(|_| AppError::Validation("分页大小超出当前平台范围".into()))?;
        let rows = filtered.into_iter().skip(offset).take(page_size).collect();
        Ok((rows, total))
    }

    pub async fn list_online_users(&self, actor: &ActorContext) -> AppResult<Vec<OnlineUserVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let metadata = match self {
            Self::Redis { client, .. } => redis_backend::list(client, tenant_id).await?,
            Self::InMemory { sessions, .. } => memory_backend::list(sessions, tenant_id).await,
        };
        Ok(self
            .authoritative_sessions(tenant_id, metadata, None)
            .await?
            .iter()
            .map(session_to_vo)
            .collect())
    }

    /// 严格更新已经存在的设备元数据，不会根据访问令牌重新创建缺失记录。
    pub async fn touch_user_strict(&self, tenant_id: &str, sid: &str) -> AppResult<()> {
        ryframe_core::validate_tenant_identifier(tenant_id)?;
        self.refresh_sessions()
            .identity(sid)
            .await?
            .filter(|identity| identity.tenant_id == tenant_id)
            .ok_or_else(|| AppError::Authentication("登录设备会话已失效".into()))?;
        match self {
            Self::Redis { client, .. } => redis_backend::touch(client, tenant_id, sid).await,
            Self::InMemory { sessions, .. } => {
                if memory_backend::touch(sessions, tenant_id, sid).await {
                    Ok(())
                } else {
                    Err(AppError::ServiceUnavailable(
                        "登录设备元数据暂不可用".into(),
                    ))
                }
            }
        }
    }

    /// 普通业务请求尽力更新时间；失败只记录诊断，不影响业务响应。
    pub async fn touch_user(&self, tenant_id: &str, sid: &str) {
        if let Err(error) = self.touch_user_strict(tenant_id, sid).await {
            tracing::warn!(%error, sid, "更新登录设备最近活动时间失败");
        }
    }

    pub async fn cleanup_expired(&self) {
        if let Self::InMemory { sessions, .. } = self {
            memory_backend::cleanup_expired(sessions).await;
        }
    }

    pub async fn count(&self, actor: &ActorContext) -> AppResult<usize> {
        Ok(self.list_online_users(actor).await?.len())
    }
}

pub fn session_to_vo(session: &UserSession) -> OnlineUserVo {
    OnlineUserVo {
        sid: session.sid.clone(),
        username: session.username.clone(),
        dept_name: session.dept_name.clone(),
        ipaddr: session.ipaddr.clone(),
        login_location: session.login_location.clone(),
        browser: session.browser.clone(),
        os: session.os.clone(),
        login_time: session.login_time.to_rfc3339(),
        last_access_time: session.last_access_time.to_rfc3339(),
    }
}
