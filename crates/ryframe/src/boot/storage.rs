use std::sync::Arc;

use ryframe_adapters::storage::{
    LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage, ScopedObjectStorage,
};
use ryframe_application::ports::files::ArtifactStore;
use ryframe_application::system::{
    AVATAR_BUCKET, CONFIG_PACKAGE_BUCKET, EXPORT_BUCKET, IMPORT_BUCKET, UPLOAD_BUCKET,
};
use ryframe_config::{AppConfig, StorageBackend};
use ryframe_kernel::{AppError, AppResult};

/// 初始化对象存储；普通服务可建立业务桶，探活进程只执行只读就绪校验。
pub async fn init(
    config: &AppConfig,
    allows_initialization_writes: bool,
) -> AppResult<Arc<dyn ArtifactStore>> {
    let storage_config = &config.object_storage;
    let raw_storage: Arc<dyn ObjectStorage> = match storage_config.backend {
        StorageBackend::Local => Arc::new(LocalObjectStorage::new(&storage_config.local_base_dir)),
        StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => Arc::new(
            S3ObjectStorage::new(S3Config {
                endpoint: storage_config.endpoint.clone(),
                access_key: storage_config.access_key.clone(),
                secret_key: storage_config.secret_key.clone(),
                use_ssl: storage_config.use_ssl,
                region: storage_config.region.clone(),
                request_timeout_secs: storage_config.request_timeout_secs,
            })
            .map_err(|error| AppError::Config(error.to_string()))?,
        ),
    };
    let storage: Arc<dyn ObjectStorage> = Arc::new(ScopedObjectStorage::new(
        raw_storage,
        config.scope_id.as_str(),
    ));

    for bucket in [
        UPLOAD_BUCKET,
        AVATAR_BUCKET,
        EXPORT_BUCKET,
        IMPORT_BUCKET,
        CONFIG_PACKAGE_BUCKET,
    ] {
        super::startup::provision_or_verify(
            allows_initialization_writes,
            || storage.ensure_bucket(bucket),
            || storage.readiness_check(bucket),
        )
        .await
        .map_err(|error| {
            AppError::Internal(format!(
                "{} 对象存储检查失败（bucket={bucket}）: {error}",
                storage_config.backend.as_str()
            ))
        })?;
    }

    tracing::info!(
        backend = storage_config.backend.as_str(),
        endpoint = storage_config.endpoint,
        scope_id = %config.scope_id,
        "对象存储连接与业务桶检查通过"
    );

    Ok(super::artifact_store::application_store(storage))
}
