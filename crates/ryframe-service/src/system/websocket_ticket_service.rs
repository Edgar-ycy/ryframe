use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_config::MessagingConfig;
use ryframe_core::RedisClient;
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

/// WebSocket 一次性票据的签发与原子消费服务。
#[derive(Clone)]
pub struct WebSocketTicketService {
    redis: Option<RedisClient>,
    config: MessagingConfig,
}

impl WebSocketTicketService {
    pub fn new(redis: Option<RedisClient>, config: MessagingConfig) -> Self {
        Self { redis, config }
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

        let redis = self.redis.as_ref().ok_or_else(|| {
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
        redis
            .set_ex(ticket_key(&ticket), encoded, self.config.ticket_ttl_seconds)
            .await
            .map_err(|error| {
                AppError::ServiceUnavailable(format!("WebSocket 票据写入失败: {error}"))
            })?;

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
        let redis = self.redis.as_ref().ok_or_else(|| {
            AppError::ServiceUnavailable("WebSocket 票据服务依赖 Redis，当前不可用".into())
        })?;
        let value = redis
            .get_and_del(ticket_key(ticket))
            .await
            .map_err(|error| {
                AppError::ServiceUnavailable(format!("WebSocket 票据校验失败: {error}"))
            })?
            .ok_or_else(invalid_ticket_error)?;
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
    use ryframe_config::MessagingConfig;
    use ryframe_kernel::AppError;

    use super::{WebSocketTicketService, is_base64url_byte, normalize_locale, ticket_key};

    #[test]
    fn ticket_key_does_not_include_the_ticket_value() {
        let ticket = "A".repeat(43);
        let key = ticket_key(&ticket);
        assert!(key.starts_with("ws-ticket:"));
        assert!(!key.contains(&ticket));
    }

    #[test]
    fn ticket_charset_only_accepts_base64url_bytes() {
        assert!(is_base64url_byte(b'A'));
        assert!(is_base64url_byte(b'_'));
        assert!(!is_base64url_byte(b'='));
        assert!(!is_base64url_byte(b'/'));
    }

    #[test]
    fn locale_normalization_uses_supported_defaults() {
        assert_eq!(normalize_locale("en-GB"), "en-US");
        assert_eq!(normalize_locale("zh"), "zh-CN");
    }

    #[test]
    fn disabled_messaging_rejects_ticket_operations_before_redis_access() {
        let service = WebSocketTicketService::new(
            None,
            MessagingConfig {
                enabled: false,
                ..MessagingConfig::default()
            },
        );

        assert!(matches!(
            service.ensure_enabled(),
            Err(AppError::ServiceUnavailable(_))
        ));
    }
}
