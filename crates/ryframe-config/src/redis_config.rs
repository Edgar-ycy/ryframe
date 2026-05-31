use serde::Deserialize;

/// Redis 配置（可选）
///
/// 不配置 `[redis]` section 时，框架不启用缓存。
#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
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
            format!(
                "redis://:{}@{}:{}/{}",
                self.password, self.host, self.port, self.database
            )
        }
    }
}
