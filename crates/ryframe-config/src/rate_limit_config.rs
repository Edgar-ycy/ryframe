use std::collections::HashMap;

use serde::Deserialize;

fn default_enabled() -> bool {
    true
}

fn default_capacity() -> u32 {
    100
}

fn default_window() -> u64 {
    60
}

fn default_user_capacity() -> u32 {
    500
}

/// 限流配置
///
/// 支持三级限流：全局（IP）、用户级、接口级。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    /// 是否启用限流
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    // ========== 全局限流（IP 维度） ==========
    /// 每个窗口允许的最大请求数
    #[serde(default = "default_capacity")]
    pub capacity: u32,

    // ========== 全局固定窗口 ==========
    /// 固定窗口时长（秒）
    #[serde(default = "default_window")]
    pub window_secs: u64,

    // ========== 用户级限流 ==========
    /// 是否启用用户级限流
    #[serde(default)]
    pub enable_user_rate_limit: bool,
    /// 用户级窗口时长（秒）
    #[serde(default = "default_window")]
    pub user_window_secs: u64,
    /// 每个用户每窗口最大请求数
    #[serde(default = "default_user_capacity")]
    pub user_capacity: u32,

    // ========== 接口级限流 ==========
    /// 敏感接口限流规则（路径 → 每窗口最大请求数）
    ///
    /// 例如：`{"POST /api/v1/auth/login": "5"}` 表示登录接口每分钟最多 5 次。
    /// 路径格式：`METHOD /path`，METHOD 省略表示所有方法。
    #[serde(default)]
    pub api_limits: HashMap<String, u32>,
    /// 敏感接口窗口时长（秒）
    #[serde(default = "default_window")]
    pub api_window_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 100,
            window_secs: 60,
            enable_user_rate_limit: false,
            user_window_secs: 60,
            user_capacity: 500,
            api_limits: HashMap::new(),
            api_window_secs: 60,
        }
    }
}

impl RateLimitConfig {
    /// 校验所有固定窗口限流规则。
    pub fn validate(&self) -> Result<(), String> {
        if self.capacity == 0 || self.window_secs == 0 {
            return Err("rate_limit.capacity 和 rate_limit.window_secs 必须大于 0".into());
        }
        if self.user_capacity == 0 || self.user_window_secs == 0 {
            return Err(
                "rate_limit.user_capacity 和 rate_limit.user_window_secs 必须大于 0".into(),
            );
        }
        if self.api_window_secs == 0 {
            return Err("rate_limit.api_window_secs 必须大于 0".into());
        }
        for (rule, limit) in &self.api_limits {
            if rule.trim().is_empty() {
                return Err("rate_limit.api_limits 的规则名称不能为空".into());
            }
            if *limit == 0 {
                return Err(format!("rate_limit.api_limits[{rule}] 必须大于 0"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::RateLimitConfig;

    #[test]
    fn default_fixed_window_config_is_valid() {
        RateLimitConfig::default().validate().unwrap();
    }

    #[test]
    fn removed_refill_parameter_is_rejected() {
        let error = toml::from_str::<RateLimitConfig>(
            r#"
            capacity = 100
            window_secs = 60
            refill_per_sec = 20
            "#,
        )
        .expect_err("旧参数必须被拒绝");

        assert!(error.to_string().contains("refill_per_sec"));
    }

    #[test]
    fn zero_windows_and_limits_are_rejected() {
        let config = RateLimitConfig {
            window_secs: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let mut config = RateLimitConfig::default();
        config
            .api_limits
            .insert("POST /api/v1/auth/login".into(), 0);
        assert!(config.validate().is_err());
    }
}
