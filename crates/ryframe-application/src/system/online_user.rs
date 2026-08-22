use std::{cmp::Reverse, future::Future, pin::Pin, sync::Arc};

use chrono::Utc;
use ryframe_kernel::{ActorContext, AppError, AppResult, ValidatedPageQuery};
use serde::{Deserialize, Serialize};

mod keyspace;
mod memory_backend;

pub use memory_backend::InMemoryOnlineSessionMetadata;

use crate::ports::auth::RefreshSessionPort;

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

pub type OnlineSessionMetadataFuture<'a, T> =
    Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// 在线设备展示元数据的出站端口。
pub trait OnlineSessionMetadataStore: Send + Sync {
    fn add(&self, session: UserSession, ttl_seconds: u64) -> OnlineSessionMetadataFuture<'_, ()>;

    fn remove<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> OnlineSessionMetadataFuture<'a, ()>;

    fn list<'a>(&'a self, tenant_id: &'a str) -> OnlineSessionMetadataFuture<'a, Vec<UserSession>>;

    fn list_for_user<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: i64,
    ) -> OnlineSessionMetadataFuture<'a, Vec<UserSession>>;

    fn touch<'a>(
        &'a self,
        tenant_id: &'a str,
        sid: &'a str,
    ) -> OnlineSessionMetadataFuture<'a, bool>;

    fn cleanup_expired(&self) -> OnlineSessionMetadataFuture<'_, ()>;
}

/// 在线用户管理服务。
#[derive(Clone)]
pub struct OnlineUserService {
    metadata: Arc<dyn OnlineSessionMetadataStore>,
    refresh_sessions: Arc<dyn RefreshSessionPort>,
}

impl OnlineUserService {
    pub fn new(
        metadata: Arc<dyn OnlineSessionMetadataStore>,
        refresh_sessions: Arc<dyn RefreshSessionPort>,
    ) -> Self {
        Self {
            metadata,
            refresh_sessions,
        }
    }

    pub fn new_in_memory(refresh_sessions: Arc<dyn RefreshSessionPort>) -> Self {
        Self {
            metadata: Arc::new(InMemoryOnlineSessionMetadata::default()),
            refresh_sessions,
        }
    }

    /// 添加会话元数据。调用方只有在此操作成功后才可以向客户端发放令牌。
    pub async fn add_user(&self, session: UserSession) -> AppResult<()> {
        ryframe_kernel::TenantId::parse(&session.tenant_id)?;
        let identity = self
            .refresh_sessions
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
        self.metadata.add(session, ttl).await
    }

    /// 清理展示元数据和索引。权威会话撤销必须由调用方先完成。
    pub async fn remove_user(&self, tenant_id: &str, sid: &str) -> AppResult<()> {
        ryframe_kernel::TenantId::parse(tenant_id)?;
        self.metadata.remove(tenant_id, sid).await
    }

    async fn authoritative_sessions(
        &self,
        tenant_id: &str,
        metadata: Vec<UserSession>,
        expected_user_id: Option<i64>,
    ) -> AppResult<Vec<UserSession>> {
        let mut sessions = Vec::with_capacity(metadata.len());
        for mut session in metadata {
            let identity = self.refresh_sessions.identity(&session.sid).await?;
            let Some(identity) = identity.filter(|identity| {
                identity.tenant_id == tenant_id
                    && identity.user_id == session.user_id
                    && expected_user_id.is_none_or(|user_id| identity.user_id == user_id)
            }) else {
                // 展示元数据失去权威会话后必须立即安全收敛。
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
        ryframe_kernel::TenantId::parse(tenant_id)?;
        let metadata = self.metadata.list_for_user(tenant_id, user_id).await?;
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
        let metadata = self.metadata.list(tenant_id).await?;
        Ok(self
            .authoritative_sessions(tenant_id, metadata, None)
            .await?
            .into_iter()
            .map(session_into_vo)
            .collect())
    }

    /// 严格更新已经存在的设备元数据，不会根据访问令牌重新创建缺失记录。
    pub async fn touch_user_strict(&self, tenant_id: &str, sid: &str) -> AppResult<()> {
        ryframe_kernel::TenantId::parse(tenant_id)?;
        self.refresh_sessions
            .identity(sid)
            .await?
            .filter(|identity| identity.tenant_id == tenant_id)
            .ok_or_else(|| AppError::Authentication("登录设备会话已失效".into()))?;
        if self.metadata.touch(tenant_id, sid).await? {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable(
                "登录设备元数据暂不可用".into(),
            ))
        }
    }

    /// 普通业务请求尽力更新时间；失败只记录诊断，不影响业务响应。
    pub async fn touch_user(&self, tenant_id: &str, sid: &str) {
        if let Err(error) = self.touch_user_strict(tenant_id, sid).await {
            tracing::warn!(%error, sid, "更新登录设备最近活动时间失败");
        }
    }

    pub async fn cleanup_expired(&self) {
        if let Err(error) = self.metadata.cleanup_expired().await {
            tracing::warn!(%error, "清理过期登录设备元数据失败");
        }
    }

    pub async fn count(&self, actor: &ActorContext) -> AppResult<usize> {
        Ok(self.list_online_users(actor).await?.len())
    }
}

fn remaining_ttl(absolute_exp: i64) -> Option<u64> {
    let remaining = absolute_exp - Utc::now().timestamp();
    (remaining > 0).then_some(remaining as u64)
}

fn session_into_vo(session: UserSession) -> OnlineUserVo {
    OnlineUserVo {
        sid: session.sid,
        username: session.username,
        dept_name: session.dept_name,
        ipaddr: session.ipaddr,
        login_location: session.login_location,
        browser: session.browser,
        os: session.os,
        login_time: session.login_time.to_rfc3339(),
        last_access_time: session.last_access_time.to_rfc3339(),
    }
}
