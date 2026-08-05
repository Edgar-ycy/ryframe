use std::sync::Arc;

use chrono::Utc;
use ryframe_db::{DatabaseCluster, ReadConsistency};
use ryframe_db::{FileRepository, entities::sys_file};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use ryframe_storage::{ObjectStorage, StorageError};
use ryframe_utils::file_upload::{
    UploadConfig, compress_image, generate_storage_filename, get_content_type, validate_extension,
    validate_file_signature,
};
use sha2::{Digest, Sha256};

mod upload_reservation;

use upload_reservation::{ReservationOutcome, UploadReservationGuard};

/// 文件上传响应
#[derive(Debug)]
pub struct UploadResponse {
    pub file_id: String,
    pub bucket: String,
    pub file_name: String,
    pub file_path: String,
}

/// 下载文件及其持久化元数据。
#[derive(Debug, PartialEq, Eq)]
pub struct DownloadedFile {
    pub data: Vec<u8>,
    pub original_name: String,
    pub content_type: String,
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

struct PreparedUpload {
    original_name: String,
    final_data: Vec<u8>,
    final_name: String,
    content_type: String,
    file_sha256: String,
}

async fn prepare_upload_data(
    original_name: String,
    data: Vec<u8>,
    compress: bool,
) -> AppResult<PreparedUpload> {
    run_blocking_task("file upload processing", move || {
        prepare_upload_data_blocking(original_name, data, compress)
    })
    .await?
}

fn prepare_upload_data_blocking(
    original_name: String,
    data: Vec<u8>,
    compress: bool,
) -> AppResult<PreparedUpload> {
    validate_file_signature(&original_name, &data)?;

    let original_size = data.len();
    let (final_data, final_name) = if compress {
        match compress_image(&data, &original_name) {
            Ok((compressed, compressed_name)) => {
                if compressed.len() < original_size {
                    let saved_pct = (1.0 - compressed.len() as f64 / original_size as f64) * 100.0;
                    tracing::info!(
                        original_size,
                        compressed_size = compressed.len(),
                        saved_pct,
                        "image compressed"
                    );
                }
                (compressed, compressed_name)
            }
            Err(error) => {
                tracing::warn!(%error, "image compression failed; using original data");
                (data, original_name.clone())
            }
        }
    } else {
        (data, original_name.clone())
    };

    let content_type = get_content_type(&final_name);
    let file_sha256 = hex::encode(Sha256::digest(&final_data));

    Ok(PreparedUpload {
        original_name,
        final_data,
        final_name,
        content_type,
        file_sha256,
    })
}

async fn run_blocking_task<T, F>(operation: &'static str, task: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(task).await.map_err(|error| {
        tracing::error!(operation, %error, "blocking task failed");
        AppError::Internal(format!("{operation} task failed"))
    })
}

pub struct FileService {
    db: DatabaseCluster,
    storage: Arc<dyn ObjectStorage>,
}

impl FileService {
    pub fn new(db: DatabaseCluster, storage: Arc<dyn ObjectStorage>) -> Self {
        Self { db, storage }
    }

    /// 校验是否可使用已配置的凭据连接存储后端。
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

        let PreparedUpload {
            original_name,
            final_data,
            final_name,
            content_type,
            file_sha256,
        } = prepare_upload_data(original_name, data, compress).await?;

        if let Some(existing) = FileRepository
            .find_by_sha256(self.db.write(), tenant_id, bucket, &file_sha256)
            .await?
        {
            return Ok(Self::upload_response_for_existing(existing));
        }

        let storage_name = generate_storage_filename(&final_name);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{tenant_id}/{date_prefix}/{storage_name}");
        let now = Utc::now();
        let reservation_token = uuid::Uuid::new_v4().to_string();
        let file_id = ryframe_utils::snowflake::try_next_snowflake_id()?;
        let model = sys_file::Model {
            id: file_id,
            tenant_id: tenant_id.to_owned(),
            original_name: original_name.clone(),
            storage_name: storage_name.clone(),
            storage_path: object_key.clone(),
            bucket: bucket.to_owned(),
            file_url: format!("{bucket}/{object_key}"),
            file_size: i64::try_from(final_data.len())
                .map_err(|_| AppError::PayloadTooLarge("文件大小超出数据库范围".into()))?,
            content_type: content_type.clone(),
            file_sha256: file_sha256.clone(),
            upload_by: Some(actor.username.clone()),
            upload_status: sys_file::Model::UPLOAD_STATUS_PENDING.to_owned(),
            reservation_token: Some(reservation_token),
            // 在预留事务内、等待租户行锁结束后，使用主数据库时钟设置。
            reservation_expires_at: None,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.to_owned(),
            created_at: now,
            updated_at: now,
        };

        let reservation = match self.reserve_upload(tenant_id, model).await? {
            ReservationOutcome::Ready(existing) => {
                return Ok(Self::upload_response_for_existing(existing));
            }
            ReservationOutcome::InProgress(existing) => {
                return self
                    .recover_in_progress_upload(existing, &file_sha256)
                    .await;
            }
            ReservationOutcome::Reserved(reservation) => reservation,
        };
        let mut guard =
            UploadReservationGuard::new(self.db.clone(), self.storage.clone(), reservation);

        if let Err(error) = self.put_reserved_object(&guard, &final_data).await {
            guard.compensate().await;
            return Err(error);
        }

        if let Err(error) = self.finalize_upload(&mut guard).await {
            guard.compensate().await;
            return Err(error);
        }
        guard.disarm();

        Ok(UploadResponse {
            file_id: file_id.to_string(),
            bucket: bucket.to_owned(),
            file_name: original_name,
            file_path: object_key,
        })
    }

    /// 下载文件：从对象存储读取数据，并返回数据库持久化的原始文件名与内容类型。
    pub async fn download(
        &self,
        actor: &ActorContext,
        bucket: &str,
        path: &str,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        // 安全检查：防止路径穿越
        if path.contains("..") {
            return Err(AppError::Validation("非法的文件路径".into()));
        }

        // 文件元数据紧跟对象上传写入主库；下载必须强一致读取，避免副本延迟导致刚上传文件返回 404。
        let db = self.db.select_read(ReadConsistency::Strong).connection;
        let file = FileRepository
            .find_by_storage_path(&db, tenant_id, bucket, path)
            .await?
            .ok_or_else(|| AppError::NotFound("文件不存在".into()))?;

        let data = self.storage.get(bucket, path).await.map_err(|error| {
            tracing::error!(bucket, path, %error, "对象存储读取失败");
            map_storage_read_error(error)
        })?;

        // 下载名称属于持久化元数据。为保证存储安全而生成的不透明对象键，绝不能
        // 作为面向用户的文件名泄露。
        Ok(DownloadedFile {
            data,
            original_name: file.original_name,
            content_type: file.content_type,
        })
    }

    fn upload_response_for_existing(existing: sys_file::Model) -> UploadResponse {
        UploadResponse {
            file_id: existing.id.to_string(),
            bucket: existing.bucket,
            file_name: existing.original_name,
            file_path: existing.storage_path,
        }
    }

    /// 上传头像（Avatar 专用便捷方法）
    ///
    /// 固定使用 `avatar` bucket、图片类型、5MB 限制、自动压缩。
    /// 返回上传元数据，调用方需同时保存稳定文件 ID 和访问地址。
    pub async fn upload_avatar(
        &self,
        actor: &ActorContext,
        original_name: String,
        data: Vec<u8>,
        max_file_size: u64,
    ) -> AppResult<UploadResponse> {
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

        Ok(result)
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
