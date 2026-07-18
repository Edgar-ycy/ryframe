use std::sync::Arc;

use ryframe_common::{AppError, AppResult};
use ryframe_config::{AppConfig, StorageBackend};
use ryframe_service::system::{AVATAR_BUCKET, UPLOAD_BUCKET};
use ryframe_storage::{LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage};

/// 初始化对象存储，并在启动阶段验证连接、凭据和业务桶。
pub async fn init(config: &AppConfig) -> AppResult<Arc<dyn ObjectStorage>> {
    let storage_config = &config.object_storage;
    let storage: Arc<dyn ObjectStorage> = match storage_config.backend {
        StorageBackend::Local => Arc::new(LocalObjectStorage::new(&storage_config.local_base_dir)),
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => Arc::new(
            S3ObjectStorage::new(S3Config {
                endpoint: storage_config.endpoint.clone(),
                access_key: storage_config.access_key.clone(),
                secret_key: storage_config.secret_key.clone(),
                use_ssl: storage_config.use_ssl,
                region: storage_config.region.clone(),
            })
            .map_err(|error| AppError::Config(error.to_string()))?,
        ),
    };

    for bucket in [UPLOAD_BUCKET, AVATAR_BUCKET] {
        storage.ensure_bucket(bucket).await.map_err(|error| {
            AppError::Internal(format!(
                "{} 对象存储检查失败（bucket={bucket}）: {error}",
                storage_config.backend.as_str()
            ))
        })?;
    }

    tracing::info!(
        backend = storage_config.backend.as_str(),
        endpoint = storage_config.endpoint,
        "对象存储连接与业务桶检查通过"
    );

    Ok(storage)
}
