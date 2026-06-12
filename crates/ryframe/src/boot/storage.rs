use std::sync::Arc;

use ryframe_common::utils::object_storage::create_storage_from_config;
use ryframe_config::{AppConfig, StorageBackend};

/// 初始化对象存储（根据配置自动选择 Local / MinIO / S3）
pub fn init(config: &AppConfig) -> Arc<dyn ryframe_common::utils::ObjectStorage> {
    let storage_config = &config.object_storage;
    let backend_str = match storage_config.backend {
        StorageBackend::Local => "local",
        StorageBackend::Minio => "minio",
        StorageBackend::S3 => "s3",
    };

    let storage: Arc<dyn ryframe_common::utils::ObjectStorage> =
        Arc::from(create_storage_from_config(
            backend_str,
            &storage_config.local_base_dir,
            &storage_config.public_base_url,
            &storage_config.endpoint,
            &storage_config.access_key,
            &storage_config.secret_key,
            storage_config.use_ssl,
        ));

    tracing::info!(
        "对象存储初始化完成, 后端: {}, 本地目录: {}",
        backend_str,
        storage_config.local_base_dir
    );

    storage
}
