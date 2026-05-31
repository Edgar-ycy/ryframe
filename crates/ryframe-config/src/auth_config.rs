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
    /// 最大登录失败次数（连续失败后锁定，默认 5）
    #[serde(default = "default_max_login_attempts")]
    pub max_login_attempts: u32,
    /// 登录锁定时间（分钟，默认 30）
    #[serde(default = "default_lockout_duration_minutes")]
    pub lockout_duration_minutes: u32,
    /// 是否启用密码复杂度校验（默认 true）
    #[serde(default = "default_enable_password_complexity")]
    pub enable_password_complexity: bool,
}

fn default_max_login_attempts() -> u32 {
    5
}

fn default_lockout_duration_minutes() -> u32 {
    30
}

fn default_enable_password_complexity() -> bool {
    true
}
