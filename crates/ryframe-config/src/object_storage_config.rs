//! 对象存储配置
//!
//! 支持三种存储后端：
//! - `local`：本地文件系统
//! - `minio`：MinIO (S3-compatible)
//! - `oss`：阿里云 OSS / 腾讯云 COS 等 S3-compatible 服务

use serde::Deserialize;

/// 对象存储后端类型
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    /// 本地文件系统（开发默认）
    Local,
    /// MinIO / S3-compatible
    Minio,
    /// 阿里云 OSS / 腾讯云 COS（S3-compatible 模式）
    S3,
}

/// 对象存储配置
#[derive(Debug, Clone, Deserialize)]
pub struct ObjectStorageConfig {
    /// 存储后端类型：local | minio | s3
    #[serde(default = "default_backend")]
    pub backend: StorageBackend,

    // ---- 通用配置 ----
    /// 本地存储根目录（local 模式下使用）
    #[serde(default = "default_local_base_dir")]
    pub local_base_dir: String,

    /// 公共访问基础 URL（用于生成可访问的文件链接）
    #[serde(default)]
    pub public_base_url: String,

    // ---- MinIO / S3 配置 ----
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

    /// AWS Region（MinIO 默认 us-east-1）
    #[serde(default = "default_region")]
    pub region: String,
}

impl Default for ObjectStorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Local,
            local_base_dir: "uploads".to_string(),
            public_base_url: String::new(),
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
