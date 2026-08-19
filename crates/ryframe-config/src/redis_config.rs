use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::ResourceScopeId;

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
    /// 由顶层 `scope_id` 注入，不允许在 `[redis]` 中单独覆盖。
    #[serde(skip)]
    pub(crate) scope_id: Option<ResourceScopeId>,
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
    /// 启用证书校验的 TLS（`rediss://`）。
    #[serde(default)]
    pub tls: bool,
    /// 当服务端 CA 不在系统信任库中时使用的可选 PEM CA 证书。
    #[serde(default)]
    pub tls_ca: Option<String>,
    /// 可选的 PEM 格式 mTLS 客户端证书。
    #[serde(default)]
    pub tls_client_cert: Option<String>,
    /// 可选的 PEM 格式 mTLS 客户端私钥。
    #[serde(default)]
    pub tls_client_key: Option<String>,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            scope_id: None,
            mode: RedisMode::Optional,
            host: default_redis_host(),
            port: default_redis_port(),
            password: String::new(),
            database: 0,
            max_pool_size: default_redis_pool_size(),
            timeout_secs: default_redis_timeout(),
            tls: false,
            tls_ca: None,
            tls_client_cert: None,
            tls_client_key: None,
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
    pub(crate) fn set_scope_id(&mut self, scope_id: ResourceScopeId) {
        self.scope_id = Some(scope_id);
    }

    pub(crate) fn has_scope_id(&self, scope_id: &ResourceScopeId) -> bool {
        self.scope_id.as_ref() == Some(scope_id)
    }

    pub fn scope_id(&self) -> &ResourceScopeId {
        self.scope_id
            .as_ref()
            .expect("AppConfig 加载后 Redis 必须具有资源作用域")
    }

    pub fn namespace(&self) -> String {
        self.scope_id().redis_namespace()
    }

    /// 生成 Redis 连接字符串
    ///
    /// 示例："redis://:password@127.0.0.1:6379/0"
    pub fn connection_url(&self) -> String {
        let scheme = if self.tls { "rediss" } else { "redis" };
        if self.password.is_empty() {
            format!("{scheme}://{}:{}/{}", self.host, self.port, self.database)
        } else {
            let password = utf8_percent_encode(&self.password, NON_ALPHANUMERIC);
            format!(
                "{scheme}://:{}@{}:{}/{}",
                password, self.host, self.port, self.database
            )
        }
    }
}
