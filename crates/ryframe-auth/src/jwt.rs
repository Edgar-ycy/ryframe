use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use ryframe_common::{AppError, AppResult};
use ryframe_config::AuthConfig;
use serde::{Deserialize, Serialize};

/// JWT Claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 用户 UUID
    pub sub: String,
    /// Tenant identity bound when the token is issued.
    pub tenant_id: String,
    /// Tenant session generation. A tenant status transition invalidates all
    /// earlier access and refresh tokens by increasing this value.
    pub tenant_session_version: i32,
    /// Per-user authentication generation. Role, permission and credential
    /// changes increment it so existing access and refresh tokens expire.
    pub user_auth_version: i32,
    /// 用户名
    pub username: String,
    /// 令牌类型: "access" | "refresh"
    pub token_type: String,
    /// Stable login-session identifier shared by access and refresh tokens.
    #[serde(default)]
    pub sid: String,
    /// 令牌唯一标识（用于在线用户管理）
    pub jti: String,
    /// 签发时间 (UNIX timestamp)
    pub iat: usize,
    /// 过期时间 (UNIX timestamp)
    pub exp: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct TokenIdentity<'a> {
    pub user_id: i64,
    pub tenant_id: &'a str,
    pub tenant_session_version: i32,
    pub user_auth_version: i32,
    pub username: &'a str,
}

/// 签发访问令牌
///
/// 返回 `(token_string, jti)` 元组，jti 用于在线用户管理。
pub fn encode_access(
    identity: &TokenIdentity<'_>,
    config: &AuthConfig,
) -> AppResult<(String, String)> {
    let sid = new_sid();
    encode_access_for_session(identity, &sid, config)
}

pub fn encode_access_for_session(
    identity: &TokenIdentity<'_>,
    sid: &str,
    config: &AuthConfig,
) -> AppResult<(String, String)> {
    let ttl = parse_duration(&config.access_token_expire)?;
    let now = current_timestamp();
    let jti = new_jti();
    let claims = Claims {
        sub: identity.user_id.to_string(),
        tenant_id: identity.tenant_id.to_string(),
        tenant_session_version: identity.tenant_session_version,
        user_auth_version: identity.user_auth_version,
        username: identity.username.to_string(),
        token_type: "access".into(),
        sid: sid.to_owned(),
        jti: jti.clone(),
        iat: now,
        exp: now + ttl,
    };
    let token = encode_claims(&claims, config)?;
    Ok((token, jti))
}

/// 签发刷新令牌
///
/// 刷新令牌仅包含用户身份信息。
pub fn encode_refresh(identity: &TokenIdentity<'_>, config: &AuthConfig) -> AppResult<String> {
    let ttl = parse_duration(&config.refresh_token_expire)?;
    let now = current_timestamp();
    let sid = new_sid();
    encode_refresh_for_session(identity, &sid, new_jti(), now + ttl, config)
}

pub fn encode_refresh_for_session(
    identity: &TokenIdentity<'_>,
    sid: &str,
    jti: String,
    absolute_exp: usize,
    config: &AuthConfig,
) -> AppResult<String> {
    let now = current_timestamp();
    encode_refresh_for_session_at(identity, sid, jti, now, absolute_exp, config)
}

/// Encode a refresh token with an explicit issuance timestamp.
///
/// Rotation recovery uses the timestamp committed with the Redis CAS so the
/// same signed token can be reconstructed after an ambiguous/lost response.
pub fn encode_refresh_for_session_at(
    identity: &TokenIdentity<'_>,
    sid: &str,
    jti: String,
    issued_at: usize,
    absolute_exp: usize,
    config: &AuthConfig,
) -> AppResult<String> {
    if issued_at > absolute_exp {
        return Err(AppError::Authentication("refresh session expired".into()));
    }
    let claims = Claims {
        sub: identity.user_id.to_string(),
        tenant_id: identity.tenant_id.to_string(),
        tenant_session_version: identity.tenant_session_version,
        user_auth_version: identity.user_auth_version,
        username: identity.username.to_string(),
        token_type: "refresh".into(),
        sid: sid.to_owned(),
        jti,
        iat: issued_at,
        exp: absolute_exp,
    };
    encode_claims(&claims, config)
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

pub fn encode_csrf(secret: &str, sid: Option<&str>, ttl_seconds: usize) -> AppResult<String> {
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
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("CSRF encode failed: {e}")))
}

pub fn decode_csrf(token: &str, secret: &str) -> AppResult<CsrfClaims> {
    let claims = decode::<CsrfClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| AppError::Authorization("invalid or expired CSRF challenge".into()))?;
    if claims.token_type != "csrf" {
        return Err(AppError::Authorization("invalid CSRF challenge".into()));
    }
    Ok(claims)
}

fn encode_claims(claims: &Claims, config: &AuthConfig) -> AppResult<String> {
    encode(
        &Header::default(),
        claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT encode failed: {}", e)))
}

/// 验证并解码 JWT
pub fn decode_token(token: &str, secret: &str) -> AppResult<Claims> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|e| AppError::Authentication(format!("令牌无效或已过期: {}", e)))
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
