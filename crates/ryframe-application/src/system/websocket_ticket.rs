use std::{future::Future, pin::Pin, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 保存于 Redis 的一次性 WebSocket 票据内容。
///
/// 票据原文不会持久化；Redis 键仅使用票据的 SHA-256 摘要。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebSocketTicket {
    pub tenant_id: String,
    pub user_id: i64,
    pub session_id: String,
    pub user_authorization_version: i32,
    pub tenant_session_version: i32,
    pub locale: String,
}

/// 成功签发 WebSocket 票据后的领域结果。
#[derive(Clone, Debug)]
pub struct WebSocketTicketGrant {
    pub ticket: String,
    pub expires_in: u64,
}

pub type WebSocketTicketStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

pub trait WebSocketTicketStore: Send + Sync {
    fn put(&self, key: String, value: String, ttl_secs: u64) -> WebSocketTicketStoreFuture<'_, ()>;

    fn take<'a>(&'a self, key: &'a str) -> WebSocketTicketStoreFuture<'a, Option<String>>;
}

/// WebSocket 一次性票据的签发与原子消费服务。
#[derive(Clone)]
pub struct WebSocketTicketService {
    store: Option<Arc<dyn WebSocketTicketStore>>,
    config: crate::MessagingPolicy,
}

impl WebSocketTicketService {
    pub fn new(
        store: Option<Arc<dyn WebSocketTicketStore>>,
        config: crate::MessagingPolicy,
    ) -> Self {
        Self { store, config }
    }

    /// 按配置有效期签发只能消费一次的 WebSocket 票据。
    pub async fn issue(
        &self,
        principal: &RequestPrincipal,
        claims: &Claims,
        locale: &str,
    ) -> AppResult<WebSocketTicketGrant> {
        self.ensure_enabled()?;
        let claimed_user_id = claims
            .sub
            .parse::<i64>()
            .map_err(|_| AppError::Authentication("访问令牌中的用户标识无效".into()))?;
        if claimed_user_id != principal.user_id || claims.tenant_id != principal.tenant_id {
            return Err(AppError::Authentication(
                "访问令牌与当前认证主体不一致".into(),
            ));
        }
        if claims.sid.trim().is_empty() {
            return Err(AppError::Authentication("访问令牌缺少登录会话标识".into()));
        }

        let store = self.store.as_ref().ok_or_else(|| {
            AppError::ServiceUnavailable("WebSocket 票据服务依赖 Redis，当前不可用".into())
        })?;
        let ticket = new_ticket();
        let payload = WebSocketTicket {
            tenant_id: principal.tenant_id.clone(),
            user_id: principal.user_id,
            session_id: claims.sid.clone(),
            user_authorization_version: claims.user_authorization_version,
            tenant_session_version: claims.tenant_session_version,
            locale: normalize_locale(locale),
        };
        let encoded = serde_json::to_string(&payload)
            .map_err(|error| AppError::Internal(format!("WebSocket 票据序列化失败: {error}")))?;
        store
            .put(ticket_key(&ticket), encoded, self.config.ticket_ttl_seconds)
            .await?;

        Ok(WebSocketTicketGrant {
            ticket,
            expires_in: self.config.ticket_ttl_seconds,
        })
    }

    /// 原子消费一次性票据；缺失、过期和重放统一返回认证失败。
    pub async fn consume(&self, ticket: &str) -> AppResult<WebSocketTicket> {
        self.ensure_enabled()?;
        if ticket.len() != 43 || !ticket.bytes().all(is_base64url_byte) {
            return Err(invalid_ticket_error());
        }
        let store = self.store.as_ref().ok_or_else(|| {
            AppError::ServiceUnavailable("WebSocket 票据服务依赖 Redis，当前不可用".into())
        })?;
        let key = ticket_key(ticket);
        let value = store.take(&key).await?.ok_or_else(invalid_ticket_error)?;
        serde_json::from_str(&value).map_err(|_| invalid_ticket_error())
    }

    fn ensure_enabled(&self) -> AppResult<()> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable("消息中心已关闭".into()))
        }
    }
}

fn invalid_ticket_error() -> AppError {
    AppError::Authentication("WebSocket 票据无效或已失效".into())
}

fn new_ticket() -> String {
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn ticket_key(ticket: &str) -> String {
    let digest = Sha256::digest(ticket.as_bytes());
    format!("ws-ticket:{}", URL_SAFE_NO_PAD.encode(digest))
}

fn is_base64url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn normalize_locale(locale: &str) -> String {
    if locale.trim().to_ascii_lowercase().starts_with("en") {
        "en-US".into()
    } else {
        "zh-CN".into()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use ryframe_kernel::{ActorContext, DataScope};
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryTicketStore {
        values: Mutex<HashMap<String, String>>,
    }

    impl WebSocketTicketStore for MemoryTicketStore {
        fn put(
            &self,
            key: String,
            value: String,
            _ttl_secs: u64,
        ) -> WebSocketTicketStoreFuture<'_, ()> {
            Box::pin(async move {
                self.values.lock().await.insert(key, value);
                Ok(())
            })
        }

        fn take<'a>(&'a self, key: &'a str) -> WebSocketTicketStoreFuture<'a, Option<String>> {
            Box::pin(async move { Ok(self.values.lock().await.remove(key)) })
        }
    }

    #[tokio::test]
    async fn issued_ticket_can_only_be_consumed_once() {
        let store = Arc::new(MemoryTicketStore::default());
        let service = WebSocketTicketService::new(
            Some(store),
            crate::MessagingPolicy::new(true, 60, 7, 100).expect("策略应有效"),
        );
        let principal = RequestPrincipal {
            actor: ActorContext {
                user_id: 42,
                tenant_id: "tenant-a".into(),
                username: "tester".into(),
                dept_id: None,
                dept_path: None,
                data_scope: DataScope::SelfOnly,
                custom_dept_ids: Vec::new(),
                include_self: true,
                is_super_admin: false,
            },
            tenant_authorization_epoch: 0,
            preferred_locale: None,
            roles: Vec::new(),
            role_ids: Vec::new(),
            permissions: Vec::new(),
            tenant_request_limit_per_minute: 0,
        };
        let claims = Claims {
            sub: "42".into(),
            tenant_id: "tenant-a".into(),
            tenant_session_version: 3,
            user_authorization_version: 4,
            username: "tester".into(),
            token_type: "access".into(),
            sid: "session-a".into(),
            jti: "token-a".into(),
            iat: 1,
            exp: 2,
        };

        let grant = service
            .issue(&principal, &claims, "en-GB")
            .await
            .expect("票据应签发成功");
        let consumed = service
            .consume(&grant.ticket)
            .await
            .expect("首次消费应成功");
        assert_eq!(consumed.user_id, 42);
        assert_eq!(consumed.locale, "en-US");
        assert!(service.consume(&grant.ticket).await.is_err());
    }
}
