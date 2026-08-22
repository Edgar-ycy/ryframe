//! 对象存储配置
//!
//! 支持四种存储后端：
//! - `local`：本地文件系统
//! - `rustfs`：RustFS（兼容 S3）
//! - `minio`：MinIO（兼容 S3）
//! - `s3`：AWS S3 及其他兼容 S3 的服务

use std::fmt;

use serde::Deserialize;

/// 对象存储后端类型
#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// 本地文件系统
    Local,
    /// RustFS（兼容 S3）
    Rustfs,
    /// MinIO / 兼容 S3 的服务。
    Minio,
    /// 兼容 S3 的端点。
    S3,
}

impl StorageBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Rustfs => "rustfs",
            Self::Minio => "minio",
            Self::S3 => "s3",
        }
    }
}

/// 对象存储配置
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectStorageConfig {
    /// 存储后端类型：local | rustfs | minio | s3
    #[serde(default = "default_backend")]
    pub backend: StorageBackend,

    // ---- 通用配置 ----
    /// 本地存储根目录（local 模式下使用）
    #[serde(default = "default_local_base_dir")]
    pub local_base_dir: String,
    /// 显式确认可在单实例或正确配置的共享卷生产环境中使用本地存储。
    #[serde(default)]
    pub allow_local_in_production: bool,

    // ---- RustFS / MinIO / S3 配置 ----
    /// 服务端点（例如 `http://localhost:9000`）。
    #[serde(default)]
    pub endpoint: String,

    /// 访问密钥。
    #[serde(default)]
    pub access_key: String,

    /// 私密访问密钥。
    #[serde(default)]
    pub secret_key: String,

    /// 是否使用 SSL
    #[serde(default)]
    pub use_ssl: bool,

    /// AWS 区域（兼容 S3 的后端通常使用 us-east-1）。
    #[serde(default = "default_region")]
    pub region: String,

    /// 单次兼容 S3 请求的总超时（秒）。
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
}

impl fmt::Debug for ObjectStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObjectStorageConfig")
            .field("backend", &self.backend)
            .field("local_base_dir", &self.local_base_dir)
            .field("allow_local_in_production", &self.allow_local_in_production)
            .field("endpoint", &self.endpoint)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("use_ssl", &self.use_ssl)
            .field("region", &self.region)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .finish()
    }
}

impl Default for ObjectStorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Local,
            local_base_dir: "uploads".to_string(),
            allow_local_in_production: false,
            endpoint: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            use_ssl: false,
            region: "us-east-1".to_string(),
            request_timeout_secs: default_request_timeout_secs(),
        }
    }
}

// ---- serde 默认值函数 ----
fn default_backend() -> StorageBackend {
    StorageBackend::Local
}

fn default_local_base_dir() -> String {
    "uploads".to_string()
}

fn default_region() -> String {
    "us-east-1".to_string()
}

const fn default_request_timeout_secs() -> u64 {
    30
}
