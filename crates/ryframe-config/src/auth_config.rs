use serde::Deserialize;

/// 认证配置
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// JWT 签名密钥（生产环境务必修改）
    pub jwt_secret: String,
    /// 访问令牌过期时间（如 "1h", "30m"）
    pub access_token_expire: String,
    /// 刷新令牌过期时间（如 "168h" 即 7 天）
    pub refresh_token_expire: String,
}
