//! 对象存储抽象层
//!
//! 支持本地文件系统和 MinIO (S3-compatible) 两种存储后端。
//! 通过 `ObjectStorage` trait 实现统一接口。
//!
//! ## 设计要点
//! - **Bucket 非配置项**：bucket 名称在调用时指定（`put(bucket, key, ...)`），
//!   不在配置文件中预设。一个项目可以使用多个 bucket。
//! - **自动创建**：MinIO/S3 后端在首次操作时自动确保 bucket 存在。

use std::path::PathBuf;

use sha2::Digest;

/// 对象存储操作结果
pub type StorageResult<T> = Result<T, String>;

/// 对象存储 trait
///
/// 实现统一的上传/下载/删除接口，支持本地和云端存储。
/// 每次操作都需要指定 bucket 名称。
#[async_trait::async_trait]
pub trait ObjectStorage: Send + Sync {
    /// 上传对象
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()>;

    /// 下载对象
    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>>;

    /// 删除对象
    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()>;

    /// 检查对象是否存在
    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool>;

    /// 生成可公开访问的 URL
    fn public_url(&self, bucket: &str, key: &str) -> String;

    /// 确保 bucket 存在（MinIO/S3 需调用，本地存储为 no-op）。
    /// 如果 bucket 不存在则自动创建。
    async fn ensure_bucket(&self, _bucket: &str) -> StorageResult<()> {
        Ok(())
    }
}

// ==================== 本地文件系统实现 ====================

/// 本地文件系统存储
pub struct LocalObjectStorage {
    base_dir: PathBuf,
    public_base_url: String,
}

impl LocalObjectStorage {
    /// 创建本地存储
    ///
    /// `base_dir` - 文件存储根目录
    /// `public_base_url` - 公共访问 URL 前缀
    pub fn new(base_dir: impl Into<PathBuf>, public_base_url: &str) -> Self {
        Self {
            base_dir: base_dir.into(),
            public_base_url: public_base_url.to_string(),
        }
    }

    /// 获取文件在磁盘上的完整路径
    /// bucket 作为一级子目录，key 作为后续路径
    fn file_path(&self, bucket: &str, key: &str) -> PathBuf {
        // 防止路径遍历攻击
        let safe_bucket = bucket.replace("..", "").replace('\\', "/");
        let safe_key = key.replace("..", "").replace('\\', "/");
        self.base_dir
            .join(safe_bucket.trim_start_matches('/'))
            .join(safe_key.trim_start_matches('/'))
    }
}

#[async_trait::async_trait]
impl ObjectStorage for LocalObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        let path = self.file_path(bucket, key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("写入文件失败: {}", e))?;
        Ok(())
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        let path = self.file_path(bucket, key);
        std::fs::read(&path).map_err(|e| format!("读取文件失败: {}", e))
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        let path = self.file_path(bucket, key);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("删除文件失败: {}", e))?;
        }
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        Ok(self.file_path(bucket, key).exists())
    }

    fn public_url(&self, bucket: &str, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.public_base_url.trim_end_matches('/'),
            bucket.trim_start_matches('/'),
            key.trim_start_matches('/')
        )
    }
}

// ==================== MinIO 实现 ====================

/// MinIO 配置
///
/// 仅包含连接凭据，不包含 bucket 名称。
/// Bucket 在每次调用 `put`/`get` 等方法时指定。
#[derive(Clone)]
pub struct MinioConfig {
    /// MinIO 服务端点 (e.g., "http://localhost:9000")
    pub endpoint: String,
    /// Access Key
    pub access_key: String,
    /// Secret Key
    pub secret_key: String,
    /// 是否使用 SSL
    pub use_ssl: bool,
}

/// MinIO 对象存储
///
/// 使用 S3-compatible HTTP API 与 MinIO 通信。
pub struct MinioStorage {
    config: MinioConfig,
    client: reqwest::Client,
}

impl MinioStorage {
    /// 创建 MinIO 存储
    pub fn new(config: MinioConfig) -> StorageResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

        let scheme = if config.use_ssl { "https" } else { "http" };
        let endpoint = config.endpoint.trim_end_matches('/');

        Ok(Self {
            config: MinioConfig {
                endpoint: format!(
                    "{}://{}",
                    scheme,
                    endpoint
                        .trim_start_matches("http://")
                        .trim_start_matches("https://")
                ),
                access_key: config.access_key,
                secret_key: config.secret_key,
                use_ssl: config.use_ssl,
            },
            client,
        })
    }

    /// 获取 MinIO 端点 URL
    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    /// 获取 Bucket 基础 URL（不含对象 key）
    fn bucket_url(&self, bucket: &str) -> String {
        format!("{}/{}", self.config.endpoint, bucket)
    }

    /// 检查 bucket 是否存在
    pub async fn bucket_exists(&self, bucket: &str) -> StorageResult<bool> {
        let url = self.bucket_url(bucket);
        let payload_hash = hex::encode(sha2::Sha256::digest(b""));

        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization = self.sign_bucket_request(bucket, "HEAD", &payload_hash, &amz_date);

        let response = self
            .client
            .head(&url)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization)
            .send()
            .await
            .map_err(|e| format!("检查 bucket 请求失败: {}", e))?;

        let status = response.status();
        Ok(status.is_success())
    }

    /// 创建 bucket
    pub async fn create_bucket(&self, bucket: &str) -> StorageResult<()> {
        let url = self.bucket_url(bucket);
        let payload_hash = hex::encode(sha2::Sha256::digest(b""));

        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization = self.sign_bucket_request(bucket, "PUT", &payload_hash, &amz_date);

        let response = self
            .client
            .put(&url)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization)
            .send()
            .await
            .map_err(|e| format!("创建 bucket 请求失败: {}", e))?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(format!("创建 bucket 失败 ({}): {}", status.as_u16(), body))
        }
    }

    /// 生成 Bucket 级别 AWS Signature V4 签名（不涉及对象 key）
    fn sign_bucket_request(
        &self,
        bucket: &str,
        method: &str,
        payload_hash: &str,
        amz_date: &str,
    ) -> String {
        use chrono::Utc;

        let now = Utc::now();
        let date_stamp = now.format("%Y%m%d").to_string();
        let region = "us-east-1";
        let service = "s3";

        let canonical_uri = format!("/{}", bucket);
        let canonical_querystring = "";
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            self.config
                .endpoint
                .trim_start_matches("http://")
                .trim_start_matches("https://"),
            payload_hash,
            amz_date
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            canonical_uri,
            canonical_querystring,
            canonical_headers,
            signed_headers,
            payload_hash
        );

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}/{}/{}/aws4_request\n{}",
            amz_date,
            date_stamp,
            region,
            service,
            hex::encode(sha2::Sha256::digest(canonical_request.as_bytes()))
        );

        let date_key = hmac_sign(
            format!("AWS4{}", self.config.secret_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let date_region_key = hmac_sign(&date_key, region.as_bytes());
        let date_region_service_key = hmac_sign(&date_region_key, service.as_bytes());
        let signing_key = hmac_sign(&date_region_service_key, b"aws4_request");

        let signature = hex::encode(hmac_sign(&signing_key, string_to_sign.as_bytes()));

        format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/{}/aws4_request,SignedHeaders={},Signature={}",
            self.config.access_key, date_stamp, region, service, signed_headers, signature
        )
    }

    /// 构造对象 URL
    fn object_url(&self, bucket: &str, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.config.endpoint,
            bucket,
            key.trim_start_matches('/')
        )
    }

    /// 生成 AWS Signature V4 签名
    fn sign_request(
        &self,
        bucket: &str,
        method: &str,
        key: &str,
        _headers: &std::collections::HashMap<String, String>,
        payload_hash: &str,
    ) -> String {
        use chrono::Utc;

        let now = Utc::now();
        let date_stamp = now.format("%Y%m%d").to_string();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let region = "us-east-1"; // MinIO 默认 region
        let service = "s3";

        let canonical_uri = format!("/{}/{}", bucket, key.trim_start_matches('/'));
        let canonical_querystring = "";
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            self.config
                .endpoint
                .trim_start_matches("http://")
                .trim_start_matches("https://"),
            payload_hash,
            amz_date
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            canonical_uri,
            canonical_querystring,
            canonical_headers,
            signed_headers,
            payload_hash
        );

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}/{}/{}/aws4_request\n{}",
            amz_date,
            date_stamp,
            region,
            service,
            hex::encode(sha2::Sha256::digest(canonical_request.as_bytes()))
        );

        let date_key = hmac_sign(
            format!("AWS4{}", self.config.secret_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let date_region_key = hmac_sign(&date_key, region.as_bytes());
        let date_region_service_key = hmac_sign(&date_region_key, service.as_bytes());
        let signing_key = hmac_sign(&date_region_service_key, b"aws4_request");

        let signature = hex::encode(hmac_sign(&signing_key, string_to_sign.as_bytes()));

        format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/{}/aws4_request,SignedHeaders={},Signature={}",
            self.config.access_key, date_stamp, region, service, signed_headers, signature
        )
    }
}

/// HMAC-SHA256 签名辅助函数
fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{KeyInit, Mac, SimpleHmac};
    use sha2::Sha256;
    let mut mac = SimpleHmac::<Sha256>::new_from_slice(key).expect("HMAC key size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// 根据配置创建对象存储实例
///
/// 根据 `ObjectStorageConfig` 创建对应的存储后端实例。
/// MinIO/S3 初始化失败时自动降级为本地存储。
pub fn create_storage_from_config(
    backend: &str,
    local_base_dir: &str,
    public_base_url: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    use_ssl: bool,
) -> Box<dyn ObjectStorage> {
    match backend {
        "minio" | "s3" => {
            match MinioStorage::new(MinioConfig {
                endpoint: endpoint.to_string(),
                access_key: access_key.to_string(),
                secret_key: secret_key.to_string(),
                use_ssl,
            }) {
                Ok(storage) => Box::new(storage),
                Err(e) => {
                    tracing::warn!(
                        "MinIO/S3 存储初始化失败({}), 降级为本地存储: {}",
                        endpoint,
                        e
                    );
                    Box::new(LocalObjectStorage::new(local_base_dir, public_base_url))
                }
            }
        }
        _ => Box::new(LocalObjectStorage::new(local_base_dir, public_base_url)),
    }
}

#[async_trait::async_trait]
impl ObjectStorage for MinioStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        let payload_hash = hex::encode(sha2::Sha256::digest(data));
        let url = self.object_url(bucket, key);

        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization =
            self.sign_request(bucket, "PUT", key, &Default::default(), &payload_hash);

        let response = self
            .client
            .put(&url)
            .header("Content-Type", content_type)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization)
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| format!("上传请求失败: {}", e))?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(format!("上传失败 ({}): {}", status.as_u16(), body))
        }
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        let payload_hash = "UNSIGNED-PAYLOAD".to_string();
        let url = self.object_url(bucket, key);

        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization =
            self.sign_request(bucket, "GET", key, &Default::default(), &payload_hash);

        let response = self
            .client
            .get(&url)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization)
            .send()
            .await
            .map_err(|e| format!("下载请求失败: {}", e))?;

        let status = response.status();
        if status.is_success() {
            response
                .bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| format!("读取响应失败: {}", e))
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(format!("下载失败 ({}): {}", status.as_u16(), body))
        }
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        let payload_hash = hex::encode(sha2::Sha256::digest(b""));
        let url = self.object_url(bucket, key);

        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization =
            self.sign_request(bucket, "DELETE", key, &Default::default(), &payload_hash);

        let response = self
            .client
            .delete(&url)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization)
            .send()
            .await
            .map_err(|e| format!("删除请求失败: {}", e))?;

        let status = response.status();
        if status.is_success() || status.as_u16() == 204 {
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(format!("删除失败 ({}): {}", status.as_u16(), body))
        }
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        let payload_hash = hex::encode(sha2::Sha256::digest(b""));
        let url = self.object_url(bucket, key);

        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization =
            self.sign_request(bucket, "HEAD", key, &Default::default(), &payload_hash);

        let response = self
            .client
            .head(&url)
            .header("x-amz-content-sha256", &payload_hash)
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization)
            .send()
            .await
            .map_err(|e| format!("HEAD 请求失败: {}", e))?;

        let status = response.status();
        Ok(status.is_success())
    }

    fn public_url(&self, bucket: &str, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.config.endpoint,
            bucket,
            key.trim_start_matches('/')
        )
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        match self.bucket_exists(bucket).await {
            Ok(true) => Ok(()),
            Ok(false) => self.create_bucket(bucket).await,
            Err(e) => Err(e),
        }
    }
}
