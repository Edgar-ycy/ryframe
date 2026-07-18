//! 对象存储配置
//!
//! 支持四种存储后端：
//! - `local`：本地文件系统
//! - `rustfs`：RustFS (S3-compatible)
//! - `minio`：MinIO (S3-compatible)
//! - `s3`：AWS S3 及其他 S3-compatible 服务

use serde::Deserialize;

/// 对象存储后端类型
#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// 本地文件系统
    Local,
    /// RustFS（S3-compatible）
    Rustfs,
    /// MinIO / S3-compatible
    Minio,
    /// S3-compatible endpoint
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
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectStorageConfig {
    /// 存储后端类型：local | rustfs | minio | s3
    #[serde(default = "default_backend")]
    pub backend: StorageBackend,

    // ---- 通用配置 ----
    /// 本地存储根目录（local 模式下使用）
    #[serde(default = "default_local_base_dir")]
    pub local_base_dir: String,

    // ---- RustFS / MinIO / S3 配置 ----
    /// 服务端点 (e.g., "http://localhost:9000")
    #[serde(default)]
    pub endpoint: String,

    /// Access Key
    #[serde(default)]
    pub access_key: String,

    /// Secret Key
    #[serde(default)]
    pub secret_key: String,

    /// 是否使用 SSL
    #[serde(default)]
    pub use_ssl: bool,

    /// AWS Region（S3 兼容后端通常使用 us-east-1）
    #[serde(default = "default_region")]
    pub region: String,
}

impl Default for ObjectStorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Local,
            local_base_dir: "uploads".to_string(),
            endpoint: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            use_ssl: false,
            region: "us-east-1".to_string(),
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
