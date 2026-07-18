use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RedisMode {
    Required,
    #[default]
    Optional,
    Disabled,
}

impl RedisMode {
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }
}

/// Redis 配置（可选）
///
/// 不配置 `[redis]` section 时，框架不启用缓存。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedisConfig {
    #[serde(default)]
    pub mode: RedisMode,
    /// Redis 主机地址
    #[serde(default = "default_redis_host")]
    pub host: String,
    /// Redis 端口
    #[serde(default = "default_redis_port")]
    pub port: u16,
    /// 密码（无密码时为空字符串）
    #[serde(default)]
    pub password: String,
    /// 数据库索引（0-15）
    #[serde(default)]
    pub database: u8,
    /// 连接池最大连接数
    #[serde(default = "default_redis_pool_size")]
    pub max_pool_size: u32,
    /// 连接超时（秒）
    #[serde(default = "default_redis_timeout")]
    pub timeout_secs: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            mode: RedisMode::Optional,
            host: default_redis_host(),
            port: default_redis_port(),
            password: String::new(),
            database: 0,
            max_pool_size: default_redis_pool_size(),
            timeout_secs: default_redis_timeout(),
        }
    }
}

fn default_redis_host() -> String {
    "127.0.0.1".into()
}

fn default_redis_port() -> u16 {
    6379
}

fn default_redis_pool_size() -> u32 {
    16
}

fn default_redis_timeout() -> u64 {
    3
}

impl RedisConfig {
    /// 生成 Redis 连接字符串
    ///
    /// 示例："redis://:password@127.0.0.1:6379/0"
    pub fn connection_url(&self) -> String {
        if self.password.is_empty() {
            format!("redis://{}:{}/{}", self.host, self.port, self.database)
        } else {
            let password = utf8_percent_encode(&self.password, NON_ALPHANUMERIC);
            format!(
                "redis://:{}@{}:{}/{}",
                password, self.host, self.port, self.database
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_url_percent_encodes_password() {
        let config = RedisConfig {
            password: "p:a/s#%@".into(),
            ..RedisConfig::default()
        };
        assert_eq!(
            config.connection_url(),
            "redis://:p%3Aa%2Fs%23%25%40@127.0.0.1:6379/0"
        );
    }
}
