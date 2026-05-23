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
    /// 用户名
    pub username: String,
    /// 角色编码列表
    pub roles: Vec<String>,
    /// 权限码列表
    pub perms: Vec<String>,
    /// 令牌类型: "access" | "refresh"
    pub token_type: String,
    /// 令牌唯一标识（用于在线用户管理）
    pub jti: String,
    /// 签发时间 (UNIX timestamp)
    pub iat: usize,
    /// 过期时间 (UNIX timestamp)
    pub exp: usize,
}

/// 签发访问令牌
///
/// `roles` 和 `perms` 嵌入 Claims，避免每次请求都查数据库。
/// 返回 `(token_string, jti)` 元组，jti 用于在线用户管理。
pub fn encode_access(
    user_id: i64,
    username: &str,
    roles: &[String],
    perms: &[String],
    config: &AuthConfig,
) -> AppResult<(String, String)> {
    let ttl = parse_duration(&config.access_token_expire)?;
    let now = Utc::now().timestamp() as usize;
    let jti = ryframe_common::utils::snowflake::next_snowflake_id().to_string();
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        roles: roles.to_vec(),
        perms: perms.to_vec(),
        token_type: "access".into(),
        jti: jti.clone(),
        iat: now,
        exp: now + ttl,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT 编码失败: {}", e)))?;
    Ok((token, jti))
}

/// 签发刷新令牌
///
/// 刷新令牌仅包含用户身份信息，不含 roles/perms（避免权限过期问题）。
/// 刷新时重新查询数据库获取最新角色权限。
pub fn encode_refresh(user_id: i64, username: &str, config: &AuthConfig) -> AppResult<String> {
    let ttl = parse_duration(&config.refresh_token_expire)?;
    let now = Utc::now().timestamp() as usize;
    let jti = ryframe_common::utils::snowflake::next_snowflake_id().to_string();
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        roles: vec![],
        perms: vec![],
        token_type: "refresh".into(),
        jti,
        iat: now,
        exp: now + ttl,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT 编码失败: {}", e)))
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
fn parse_duration(s: &str) -> AppResult<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        assert_eq!(parse_duration("3600").unwrap(), 3600);
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let config = AuthConfig {
            jwt_secret: "test-secret".into(),
            access_token_expire: "1h".into(),
            refresh_token_expire: "168h".into(),
        };
        let user_id = 1234567890123456789i64;
        let roles = vec!["admin".to_string()];
        let perms = vec!["system:user:list".to_string()];

        let (token, jti) = encode_access(user_id, "admin", &roles, &perms, &config).unwrap();
        let claims = decode_token(&token, &config.jwt_secret).unwrap();

        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.username, "admin");
        assert_eq!(claims.roles, roles);
        assert_eq!(claims.perms, perms);
        assert_eq!(claims.token_type, "access");
        assert!(!claims.jti.is_empty());
        assert_eq!(claims.jti, jti);
    }
}
