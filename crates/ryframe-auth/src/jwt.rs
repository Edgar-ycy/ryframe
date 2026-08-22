use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// JWT 签名与有效期设置。
///
/// 配置加载由组合根负责；认证核心只接收已经解析、校验完成的安全设置。
pub struct TokenSettings {
    secret: Arc<str>,
    access_token_ttl_seconds: usize,
    refresh_token_ttl_seconds: usize,
}

impl TokenSettings {
    pub fn new(
        secret: impl Into<Arc<str>>,
        access_token_expire: &str,
        refresh_token_expire: &str,
    ) -> AppResult<Self> {
        Ok(Self {
            secret: secret.into(),
            access_token_ttl_seconds: parse_duration(access_token_expire)?,
            refresh_token_ttl_seconds: parse_duration(refresh_token_expire)?,
        })
    }

    fn secret(&self) -> &str {
        &self.secret
    }

    pub const fn access_token_ttl_seconds(&self) -> usize {
        self.access_token_ttl_seconds
    }

    pub const fn refresh_token_ttl_seconds(&self) -> usize {
        self.refresh_token_ttl_seconds
    }
}

/// JWT 声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 用户 UUID
    pub sub: String,
    /// 令牌签发时绑定的租户身份。
    pub tenant_id: String,
    /// 租户会话版本。租户状态变更时递增该值，使此前的访问令牌和刷新令牌全部失效。
    pub tenant_session_version: i32,
    /// 用户认证版本。角色、权限或凭据变更时递增该值，使现有访问令牌和刷新令牌失效。
    pub user_authorization_version: i32,
    /// 用户名
    pub username: String,
    /// 令牌类型: "access" | "refresh"
    pub token_type: String,
    /// 访问令牌与刷新令牌共享的稳定登录会话标识。
    pub sid: String,
    /// 令牌唯一标识（用于在线用户管理）
    pub jti: String,
    /// 签发时间（UNIX 时间戳）
    pub iat: usize,
    /// 过期时间（UNIX 时间戳）
    pub exp: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct TokenIdentity<'a> {
    pub user_id: i64,
    pub tenant_id: &'a str,
    pub tenant_session_version: i32,
    pub user_authorization_version: i32,
    pub username: &'a str,
}

/// 签发访问令牌
///
/// 返回 `(token_string, jti)` 元组，jti 用于在线用户管理。
pub fn encode_access(
    identity: &TokenIdentity<'_>,
    settings: &TokenSettings,
) -> AppResult<(String, String)> {
    let sid = new_sid();
    encode_access_for_session(identity, &sid, settings)
}

pub fn encode_access_for_session(
    identity: &TokenIdentity<'_>,
    sid: &str,
    settings: &TokenSettings,
) -> AppResult<(String, String)> {
    let ttl = settings.access_token_ttl_seconds();
    let now = current_timestamp();
    let jti = new_jti();
    let claims = Claims {
        sub: identity.user_id.to_string(),
        tenant_id: identity.tenant_id.to_string(),
        tenant_session_version: identity.tenant_session_version,
        user_authorization_version: identity.user_authorization_version,
        username: identity.username.to_string(),
        token_type: "access".into(),
        sid: sid.to_owned(),
        jti: jti.clone(),
        iat: now,
        exp: now + ttl,
    };
    let token = encode_claims(&claims, settings)?;
    Ok((token, jti))
}

/// 签发刷新令牌
///
/// 刷新令牌仅包含用户身份信息。
pub fn encode_refresh(identity: &TokenIdentity<'_>, settings: &TokenSettings) -> AppResult<String> {
    let ttl = settings.refresh_token_ttl_seconds();
    let now = current_timestamp();
    let sid = new_sid();
    encode_refresh_for_session(identity, &sid, new_jti(), now + ttl, settings)
}

pub fn encode_refresh_for_session(
    identity: &TokenIdentity<'_>,
    sid: &str,
    jti: String,
    absolute_exp: usize,
    settings: &TokenSettings,
) -> AppResult<String> {
    let now = current_timestamp();
    encode_refresh_for_session_at(identity, sid, jti, now, absolute_exp, settings)
}

/// 使用明确的签发时间戳编码刷新令牌。
///
/// 轮换恢复使用 Redis CAS 已提交的时间戳，以便在响应不明确或丢失后重建相同的已签名令牌。
pub fn encode_refresh_for_session_at(
    identity: &TokenIdentity<'_>,
    sid: &str,
    jti: String,
    issued_at: usize,
    absolute_exp: usize,
    settings: &TokenSettings,
) -> AppResult<String> {
    if issued_at > absolute_exp {
        return Err(AppError::Authentication("refresh session expired".into()));
    }
    let claims = Claims {
        sub: identity.user_id.to_string(),
        tenant_id: identity.tenant_id.to_string(),
        tenant_session_version: identity.tenant_session_version,
        user_authorization_version: identity.user_authorization_version,
        username: identity.username.to_string(),
        token_type: "refresh".into(),
        sid: sid.to_owned(),
        jti,
        iat: issued_at,
        exp: absolute_exp,
    };
    encode_claims(&claims, settings)
}

fn current_timestamp() -> usize {
    Utc::now().timestamp() as usize
}

fn new_jti() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn generate_jti() -> String {
    new_jti()
}

pub fn new_sid() -> String {
    format!("s-{}", uuid::Uuid::new_v4())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrfClaims {
    pub token_type: String,
    pub sid: Option<String>,
    pub jti: String,
    pub iat: usize,
    pub exp: usize,
}

pub fn encode_csrf(
    settings: &TokenSettings,
    sid: Option<&str>,
    ttl_seconds: usize,
) -> AppResult<String> {
    let now = current_timestamp();
    let claims = CsrfClaims {
        token_type: "csrf".into(),
        sid: sid.map(str::to_owned),
        jti: new_jti(),
        iat: now,
        exp: now + ttl_seconds,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(settings.secret().as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("CSRF encode failed: {e}")))
}

pub fn decode_csrf(token: &str, settings: &TokenSettings) -> AppResult<CsrfClaims> {
    let claims = decode::<CsrfClaims>(
        token,
        &DecodingKey::from_secret(settings.secret().as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::Authorization("invalid or expired CSRF challenge".into()))?;
    if claims.token_type != "csrf" {
        return Err(AppError::Authorization("invalid CSRF challenge".into()));
    }
    Ok(claims)
}

fn encode_claims(claims: &Claims, settings: &TokenSettings) -> AppResult<String> {
    encode(
        &Header::default(),
        claims,
        &EncodingKey::from_secret(settings.secret().as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT encode failed: {}", e)))
}

/// 验证并解码 JWT
pub fn decode_token(token: &str, settings: &TokenSettings) -> AppResult<Claims> {
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(settings.secret().as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|e| AppError::Authentication(format!("令牌无效或已过期: {}", e)))?;
    if claims.sid.is_empty() {
        return Err(AppError::Authentication("令牌会话标识无效".into()));
    }
    Ok(claims)
}

/// 解析 duration 字符串为秒数
///
/// 支持格式：`1h`（小时）、`30m`（分钟）、`3600`（纯数字秒）
pub fn parse_duration(s: &str) -> AppResult<usize> {
    let s = s.trim();
    if let Some(hours) = s.strip_suffix('h') {
        hours
            .trim()
            .parse::<usize>()
            .map(|v| v * 3600)
            .map_err(|_| AppError::Config(format!("无效的 duration: {}", s)))
    } else if let Some(minutes) = s.strip_suffix('m') {
        minutes
            .trim()
            .parse::<usize>()
            .map(|v| v * 60)
            .map_err(|_| AppError::Config(format!("无效的 duration: {}", s)))
    } else {
        s.parse::<usize>()
            .map_err(|_| AppError::Config(format!("无效的 duration: {}", s)))
    }
}
