use std::sync::Arc;

use chrono::Utc;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use ryframe_common::{
    ActorContext, AppError, AppResult,
    utils::file_upload::{
        UploadConfig, UploadFileInfo, compress_image, generate_storage_filename, get_content_type,
        validate_extension, validate_file_signature,
    },
};
use ryframe_core::repository::Repository;
use ryframe_db::DatabaseCluster;
use ryframe_db::{FileRepository, entities::sys_file};
use ryframe_storage::{ObjectStorage, StorageError};
use serde::Serialize;
use utoipa::ToSchema;

/// 文件上传响应
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadResponse {
    pub file_id: String,
    pub file_url: String,
    pub file_info: UploadFileInfo,
}

/// 默认上传 bucket 名称
pub const UPLOAD_BUCKET: &str = "uploads";

/// Avatar 专用 bucket 名称
pub const AVATAR_BUCKET: &str = "avatar";

pub struct UploadCommand<'a> {
    pub original_name: String,
    pub data: Vec<u8>,
    pub config: &'a UploadConfig,
    pub bucket: &'a str,
    pub compress: bool,
}

pub struct FileService {
    db: DatabaseCluster,
    storage: Arc<dyn ObjectStorage>,
}

impl FileService {
    pub fn new(db: DatabaseCluster, storage: Arc<dyn ObjectStorage>) -> Self {
        Self { db, storage }
    }

    /// Validate that the storage backend is reachable with the configured credentials.
    pub async fn check_storage(&self) -> AppResult<()> {
        for bucket in [UPLOAD_BUCKET, AVATAR_BUCKET] {
            self.storage
                .readiness_check(bucket)
                .await
                .map_err(|error| {
                    AppError::ServiceUnavailable(format!(
                        "object storage readiness check failed: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    /// 上传单个文件并写入 sys_file 元数据表
    ///
    /// 包含：验证 → 压缩（可选）→ 上传对象存储 → 写入 sys_file 表 → 返回 UploadResponse
    pub async fn upload_single(
        &self,
        actor: &ActorContext,
        command: UploadCommand<'_>,
    ) -> AppResult<UploadResponse> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let UploadCommand {
            original_name,
            data,
            config,
            bucket,
            compress,
        } = command;
        // 验证文件大小
        if data.len() as u64 > config.max_file_size {
            return Err(AppError::PayloadTooLarge(format!(
                "文件大小超过限制（最大 {} MB）",
                config.max_file_size / 1024 / 1024
            )));
        }

        // 验证文件类型
        validate_extension(&original_name, &config.allowed_extensions)?;
        validate_file_signature(&original_name, &data)?;

        // 图片压缩（可选）
        let (final_data, final_name, content_type) = if compress {
            let (compressed, compressed_name) = compress_image(&data, &original_name)
                .unwrap_or_else(|e| {
                    tracing::warn!("图片压缩失败，使用原始数据: {}", e);
                    (data.clone(), original_name.clone())
                });
            if compressed.len() < data.len() {
                let saved_pct = (1.0 - compressed.len() as f64 / data.len() as f64) * 100.0;
                tracing::info!(
                    "图片压缩: {} → {} ({:.1}% 减小)",
                    ryframe_common::utils::file_upload::format_file_size(data.len() as u64),
                    ryframe_common::utils::file_upload::format_file_size(compressed.len() as u64),
                    saved_pct
                );
            }
            let ct = get_content_type(&compressed_name);
            (compressed, compressed_name, ct)
        } else {
            let ct = get_content_type(&original_name);
            (data, original_name.clone(), ct)
        };

        // 生成存储文件名 + 日期路径
        let file_md5 = format!("{:x}", md5::compute(&final_data));
        if let Some(existing) = FileRepository
            .find_by_md5(self.db.write(), tenant_id, bucket, &file_md5)
            .await?
        {
            return Ok(UploadResponse {
                file_id: existing.id.to_string(),
                file_url: self.build_file_url(&existing.bucket, &existing.storage_path)?,
                file_info: UploadFileInfo {
                    original_name: existing.original_name,
                    storage_name: existing.storage_name,
                    file_path: existing.storage_path,
                    file_size: existing.file_size as u64,
                    content_type: existing.content_type,
                    upload_time: existing.created_at.to_rfc3339(),
                },
            });
        }

        ryframe_db::TenantRepository
            .ensure_storage_quota(self.db.write(), tenant_id, final_data.len() as u64)
            .await?;

        let storage_name = generate_storage_filename(&final_name);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{tenant_id}/{date_prefix}/{storage_name}");

        let file_url = self.build_file_url(bucket, &object_key)?;

        // 上传到对象存储
        self.storage
            .put(bucket, &object_key, &final_data, &content_type)
            .await
            .map_err(|error| {
                tracing::error!(bucket, object_key, %error, "对象存储写入失败");
                map_storage_write_error(error)
            })?;

        // 写入文件元数据到 sys_file 表
        let relative_file_url = format!("{}/{}", bucket, object_key);
        let file_id = ryframe_common::utils::snowflake::next_snowflake_id();
        let model = sys_file::Model {
            id: file_id,
            tenant_id: tenant_id.to_owned(),
            original_name: original_name.clone(),
            storage_name: storage_name.clone(),
            storage_path: object_key.clone(),
            bucket: bucket.to_string(),
            file_url: relative_file_url,
            file_size: final_data.len() as i64,
            content_type: content_type.clone(),
            file_md5: Some(file_md5),
            upload_by: Some(actor.username.clone()),
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        if let Err(error) = FileRepository
            .insert(self.db.write(), tenant_id, model)
            .await
        {
            if let Err(cleanup_error) = self.storage.delete(bucket, &object_key).await {
                tracing::error!(
                    tenant_id,
                    bucket,
                    object_key,
                    %cleanup_error,
                    "文件元数据写入失败后清理对象也失败"
                );
            }
            return Err(error);
        }

        Ok(UploadResponse {
            file_id: file_id.to_string(),
            file_url,
            file_info: UploadFileInfo {
                original_name,
                storage_name,
                file_path: object_key.clone(),
                file_size: final_data.len() as u64,
                content_type,
                upload_time: Utc::now().to_rfc3339(),
            },
        })
    }

    /// 下载文件：从对象存储读取数据，返回 (data, filename)
    pub async fn download(
        &self,
        actor: &ActorContext,
        bucket: &str,
        path: &str,
    ) -> AppResult<(Vec<u8>, String)> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        // 安全检查：防止路径穿越
        if path.contains("..") {
            return Err(AppError::Validation("非法的文件路径".into()));
        }

        // 文件元数据紧跟对象上传写入主库；下载必须从主库读取，避免从库延迟导致刚上传文件返回 404。
        FileRepository
            .find_by_storage_path(self.db.write(), tenant_id, bucket, path)
            .await?
            .ok_or_else(|| AppError::NotFound("文件不存在".into()))?;

        let data = self.storage.get(bucket, path).await.map_err(|error| {
            tracing::error!(bucket, path, %error, "对象存储读取失败");
            map_storage_read_error(error)
        })?;

        let filename = path.rsplit('/').next().unwrap_or("download").to_string();

        Ok((data, filename))
    }

    /// 构建只能通过认证后端访问的私有文件地址。
    pub fn build_file_url(&self, bucket: &str, key: &str) -> AppResult<String> {
        Ok(format!(
            "/api/v1/common/file/download?bucket={}&path={}",
            utf8_percent_encode(bucket, NON_ALPHANUMERIC),
            utf8_percent_encode(key, NON_ALPHANUMERIC),
        ))
    }

    /// 上传头像（Avatar 专用便捷方法）
    ///
    /// 固定使用 `avatar` bucket、图片类型、5MB 限制、自动压缩。
    /// 返回稳定访问地址（用于更新 sys_user.avatar）。
    pub async fn upload_avatar(
        &self,
        actor: &ActorContext,
        original_name: String,
        data: Vec<u8>,
        max_file_size: u64,
    ) -> AppResult<String> {
        let config = UploadConfig {
            allowed_extensions: vec![
                "jpg".to_string(),
                "jpeg".to_string(),
                "png".to_string(),
                "gif".to_string(),
                "bmp".to_string(),
                "webp".to_string(),
            ],
            max_file_size,
            ..Default::default()
        };

        let result = self
            .upload_single(
                actor,
                UploadCommand {
                    original_name,
                    data,
                    config: &config,
                    bucket: AVATAR_BUCKET,
                    compress: true,
                },
            )
            .await?;

        Ok(result.file_url)
    }
}

fn map_storage_write_error(error: StorageError) -> AppError {
    match error {
        StorageError::InvalidLocation(_) => AppError::Validation("非法的对象存储路径".into()),
        StorageError::Configuration(_) | StorageError::Signing(_) => {
            AppError::Internal("对象存储配置错误".into())
        }
        StorageError::Service { status, .. } if status == 429 || status >= 500 => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
        StorageError::Service { .. } => AppError::Internal("对象存储拒绝写入请求".into()),
        StorageError::Transport(_) | StorageError::Io { .. } | StorageError::Readiness(_) => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
    }
}

fn map_storage_read_error(error: StorageError) -> AppError {
    match error {
        StorageError::Service { status: 404, .. } => AppError::NotFound("文件不存在".into()),
        StorageError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            AppError::NotFound("文件不存在".into())
        }
        StorageError::InvalidLocation(_) => AppError::Validation("非法的对象存储路径".into()),
        StorageError::Configuration(_) | StorageError::Signing(_) => {
            AppError::Internal("对象存储配置错误".into())
        }
        StorageError::Service { status, .. } if status == 429 || status >= 500 => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
        StorageError::Service { .. } => AppError::Internal("对象存储拒绝读取请求".into()),
        StorageError::Transport(_) | StorageError::Io { .. } | StorageError::Readiness(_) => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
    }
}

#[cfg(test)]
mod storage_error_tests {
    use super::{map_storage_read_error, map_storage_write_error};
    use ryframe_common::AppError;
    use ryframe_storage::StorageError;

    #[test]
    fn maps_only_real_missing_objects_to_not_found() {
        let remote_missing = StorageError::Service {
            operation: "get",
            status: 404,
            message: "missing".into(),
        };
        let local_missing = StorageError::Io {
            operation: "read",
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert!(matches!(
            map_storage_read_error(remote_missing),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            map_storage_read_error(local_missing),
            AppError::NotFound(_)
        ));
    }

    #[test]
    fn maps_runtime_storage_failures_to_service_unavailable() {
        for error in [
            StorageError::Service {
                operation: "put",
                status: 500,
                message: "upstream".into(),
            },
            StorageError::Service {
                operation: "put",
                status: 429,
                message: "busy".into(),
            },
            StorageError::Io {
                operation: "write",
                source: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
            },
        ] {
            assert!(matches!(
                map_storage_write_error(error),
                AppError::ServiceUnavailable(_)
            ));
        }
    }

    #[test]
    fn keeps_configuration_and_location_errors_distinct() {
        assert!(matches!(
            map_storage_write_error(StorageError::InvalidLocation("unsafe".into())),
            AppError::Validation(_)
        ));
        assert!(matches!(
            map_storage_write_error(StorageError::Configuration("missing key".into())),
            AppError::Internal(_)
        ));
    }
}
