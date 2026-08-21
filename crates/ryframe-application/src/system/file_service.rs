use std::sync::Arc;

use chrono::{DateTime, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sha2::{Digest, Sha256};

use crate::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind, FILE_UPLOAD_STATUS_CLEANUP,
    FILE_UPLOAD_STATUS_READY, FileCleanupPersistencePort, FileContentProcessor,
    FileDownloadPersistencePort, FileUploadCommitMode, FileUploadPersistencePort, FileUploadRecord,
    ProcessedFileContent,
};

mod policy;
mod upload_reservation;

pub use policy::UploadPolicy;

use upload_reservation::{ReservationOutcome, UploadReservationGuard, storage_error_is_not_found};

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

/// 用户导入源文件与错误报告专用私有 bucket 名称。
pub const IMPORT_BUCKET: &str = "imports";

/// 租户配置包和回滚快照专用私有 bucket 名称。
pub const CONFIG_PACKAGE_BUCKET: &str = "config-packages";

/// 内部文件最终清理声明的租约时长；进程退出后由全局清理器接管。
const INTERNAL_DELETE_CLAIM_SECONDS: i64 = 300;

/// 对象存储暂时不可用时的清理重试间隔。
const INTERNAL_DELETE_RETRY_SECONDS: i64 = 60;

pub struct UploadCommand<'a> {
    pub original_name: String,
    pub data: Vec<u8>,
    pub policy: &'a UploadPolicy,
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
    processor: &dyn FileContentProcessor,
    original_name: String,
    data: Vec<u8>,
    compress: bool,
) -> AppResult<PreparedUpload> {
    let ProcessedFileContent {
        original_name,
        data: final_data,
        file_name: final_name,
        content_type,
    } = processor.process(original_name, data, compress).await?;
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
    cleanup: Arc<dyn FileCleanupPersistencePort>,
    downloads: Arc<dyn FileDownloadPersistencePort>,
    uploads: Arc<dyn FileUploadPersistencePort>,
    storage: Arc<dyn ArtifactStore>,
    content: Arc<dyn FileContentProcessor>,
}

impl FileService {
    pub fn new(
        cleanup: Arc<dyn FileCleanupPersistencePort>,
        downloads: Arc<dyn FileDownloadPersistencePort>,
        uploads: Arc<dyn FileUploadPersistencePort>,
        storage: Arc<dyn ArtifactStore>,
        content: Arc<dyn FileContentProcessor>,
    ) -> Self {
        Self {
            cleanup,
            downloads,
            uploads,
            storage,
            content,
        }
    }

    /// 校验是否可使用已配置的凭据连接存储后端。
    pub async fn check_storage(&self) -> AppResult<()> {
        for bucket in [
            UPLOAD_BUCKET,
            AVATAR_BUCKET,
            IMPORT_BUCKET,
            CONFIG_PACKAGE_BUCKET,
        ] {
            self.storage.readiness(bucket).await.map_err(|error| {
                AppError::ServiceUnavailable(format!(
                    "object storage readiness check failed: {error}"
                ))
            })?;
        }
        Ok(())
    }

    /// 上传单个文件并持久化文件元数据。
    ///
    /// 包含：验证 → 压缩（可选）→ 上传对象存储 → 持久化元数据 → 返回响应。
    pub async fn upload_single(
        &self,
        actor: &ActorContext,
        command: UploadCommand<'_>,
    ) -> AppResult<UploadResponse> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.upload_for_tenant(
            tenant_id,
            &actor.username,
            command,
            FileUploadCommitMode::CurrentRequest,
        )
        .await
    }

    /// 由后台任务上传受控的内部文件，不接受客户端指定租户或 bucket。
    pub(crate) async fn upload_internal(
        &self,
        tenant_id: &str,
        uploaded_by: &str,
        command: UploadCommand<'_>,
    ) -> AppResult<UploadResponse> {
        if tenant_id.is_empty() || tenant_id.len() > 64 {
            return Err(AppError::Validation("内部文件租户标识无效".into()));
        }
        self.upload_for_tenant(
            tenant_id,
            uploaded_by,
            command,
            FileUploadCommitMode::CurrentRequest,
        )
        .await
    }

    /// 供组合业务上传中间文件，最终操作审计由组合业务的成功事务统一提交。
    pub(crate) async fn upload_internal_unbound(
        &self,
        tenant_id: &str,
        uploaded_by: &str,
        command: UploadCommand<'_>,
    ) -> AppResult<UploadResponse> {
        if tenant_id.is_empty() || tenant_id.len() > 64 {
            return Err(AppError::Validation("内部文件租户标识无效".into()));
        }
        self.upload_for_tenant(
            tenant_id,
            uploaded_by,
            command,
            FileUploadCommitMode::Unbound,
        )
        .await
    }

    /// 上传由服务端生成或已经完成格式校验的配置包，不接受客户端 bucket。
    pub async fn upload_config_package_unbound(
        &self,
        tenant_id: &str,
        uploaded_by: &str,
        original_name: String,
        data: Vec<u8>,
        max_file_size: u64,
    ) -> AppResult<UploadResponse> {
        let policy = UploadPolicy {
            allowed_extensions: vec!["zip".to_owned()],
            max_file_size,
        };
        self.upload_internal_unbound(
            tenant_id,
            uploaded_by,
            UploadCommand {
                original_name,
                data,
                policy: &policy,
                bucket: CONFIG_PACKAGE_BUCKET,
                compress: false,
            },
        )
        .await
    }

    async fn upload_for_tenant(
        &self,
        tenant_id: &str,
        uploaded_by: &str,
        command: UploadCommand<'_>,
        commit_mode: FileUploadCommitMode,
    ) -> AppResult<UploadResponse> {
        let UploadCommand {
            original_name,
            data,
            policy,
            bucket,
            compress,
        } = command;
        // 验证文件大小
        if data.len() as u64 > policy.max_file_size {
            return Err(AppError::PayloadTooLarge(format!(
                "文件大小超过限制（最大 {} MB）",
                policy.max_file_size / 1024 / 1024
            )));
        }

        // 验证文件类型
        validate_extension(&original_name, &policy.allowed_extensions)?;

        let PreparedUpload {
            original_name,
            final_data,
            final_name,
            content_type,
            file_sha256,
        } = prepare_upload_data(self.content.as_ref(), original_name, data, compress).await?;

        let storage_name = generate_storage_filename(&final_name);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{tenant_id}/{date_prefix}/{storage_name}");
        let now = Utc::now();
        let reservation_token = uuid::Uuid::new_v4().to_string();
        let file_id = crate::next_id()?;
        let model = FileUploadRecord {
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
            upload_by: Some(uploaded_by.to_owned()),
            upload_status: crate::FILE_UPLOAD_STATUS_PENDING.to_owned(),
            reservation_token: Some(reservation_token),
            // 在预留事务内、等待租户行锁结束后，使用主数据库时钟设置。
            reservation_expires_at: None,
            del_flag: crate::FILE_DEL_FLAG_NORMAL.to_owned(),
            created_at: now,
            updated_at: now,
        };

        let reservation = match self.reserve_upload(tenant_id, model).await? {
            ReservationOutcome::Ready(existing) => {
                return Ok(Self::upload_response_for_existing(existing));
            }
            ReservationOutcome::InProgress(existing) => {
                return self
                    .recover_in_progress_upload(existing, &file_sha256, commit_mode)
                    .await;
            }
            ReservationOutcome::Reserved(reservation) => reservation,
        };
        let mut guard = UploadReservationGuard::new(
            Arc::clone(&self.cleanup),
            Arc::clone(&self.storage),
            reservation,
        );

        if let Err(error) = self.put_reserved_object(&guard, &final_data).await {
            guard.compensate().await;
            return Err(error);
        }

        if let Err(error) = self.finalize_upload(&mut guard, commit_mode).await {
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

        let file = self
            .downloads
            .find_by_storage_path(tenant_id, bucket, path)
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

    /// 按稳定文件 ID 下载当前租户的受控文件，并校验 bucket 边界。
    pub async fn download_by_id(
        &self,
        actor: &ActorContext,
        file_id: i64,
        expected_bucket: &str,
    ) -> AppResult<DownloadedFile> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        self.download_by_id_for_tenant(tenant_id, file_id, expected_bucket)
            .await
    }

    /// 供后台任务按租户和稳定文件 ID 读取私有文件。
    pub(crate) async fn download_internal(
        &self,
        tenant_id: &str,
        file_id: i64,
        expected_bucket: &str,
    ) -> AppResult<DownloadedFile> {
        self.download_by_id_for_tenant(tenant_id, file_id, expected_bucket)
            .await
    }

    /// 供配置迁移服务按稳定文件 ID 读取私有配置包或回滚快照。
    pub async fn download_config_package_internal(
        &self,
        tenant_id: &str,
        file_id: i64,
    ) -> AppResult<DownloadedFile> {
        self.download_internal(tenant_id, file_id, CONFIG_PACKAGE_BUCKET)
            .await
    }

    async fn download_by_id_for_tenant(
        &self,
        tenant_id: &str,
        file_id: i64,
        expected_bucket: &str,
    ) -> AppResult<DownloadedFile> {
        let file = self
            .downloads
            .find_ready_by_id(tenant_id, file_id, expected_bucket)
            .await?
            .ok_or_else(|| AppError::NotFound("文件不存在或已过期".into()))?;
        let data = self
            .storage
            .get(&file.bucket, &file.storage_path)
            .await
            .map_err(map_storage_read_error)?;
        Ok(DownloadedFile {
            data,
            original_name: file.original_name,
            content_type: file.content_type,
        })
    }

    /// 删除已经超过保留期的导入文件对象及元数据；重复删除保持幂等。
    pub(crate) async fn delete_expired_import_artifact(
        &self,
        tenant_id: &str,
        file_id: i64,
        expired_before: DateTime<Utc>,
    ) -> AppResult<bool> {
        let expected_bucket = IMPORT_BUCKET;
        let transaction = self.cleanup.begin().await?;
        // 用户导入创建同样先锁租户再锁文件。统一锁顺序可保证资格复核与新任务引用
        // 串行发生，避免初始候选查询后文件被活动任务重新引用。
        transaction.lock_tenant(tenant_id).await?;
        let Some(file) = transaction.find_for_update(tenant_id, file_id).await? else {
            transaction.rollback().await?;
            return Ok(false);
        };
        if file.bucket != expected_bucket {
            transaction.rollback().await?;
            return Err(AppError::Authorization("内部文件存储边界不匹配".into()));
        }
        if file.upload_status != FILE_UPLOAD_STATUS_READY {
            transaction.rollback().await?;
            return Ok(false);
        }

        let claimed_at = transaction.database_now().await?;
        let claim_until = claimed_at + chrono::Duration::seconds(INTERNAL_DELETE_CLAIM_SECONDS);
        let claim_token = uuid::Uuid::new_v4().to_string();
        if !transaction
            .claim_expired_import(
                tenant_id,
                file_id,
                &claim_token,
                expired_before,
                claim_until,
            )
            .await?
        {
            transaction.rollback().await?;
            return Ok(false);
        }

        // 先持久化不可恢复的清理声明，再触碰对象存储。提交响应不明时必须从主库验证
        // 令牌，只有能够证明本实例拥有清理权才允许删除对象。
        if let Err(commit_error) = transaction.commit().await {
            let verified = self.cleanup.find(tenant_id, file_id).await;
            match verified {
                Ok(Some(current))
                    if current.bucket == expected_bucket
                        && current.upload_status == FILE_UPLOAD_STATUS_CLEANUP
                        && current.reservation_token.as_deref() == Some(claim_token.as_str()) => {}
                Ok(_) => {
                    return Err(AppError::Database(format!(
                        "文件清理声明提交结果无法确认: {commit_error}"
                    )));
                }
                Err(verify_error) => {
                    return Err(AppError::Database(format!(
                        "文件清理声明提交结果无法确认: {commit_error}; 主库核验失败: {verify_error}"
                    )));
                }
            }
        }

        let delete_result = self.storage.delete(&file.bucket, &file.storage_path).await;
        if let Err(error) = delete_result
            && !storage_error_is_not_found(&error)
        {
            let retry_at = self.cleanup.database_now().await?;
            if let Err(defer_error) = self
                .cleanup
                .defer_claim(
                    tenant_id,
                    file_id,
                    &claim_token,
                    retry_at,
                    retry_at + chrono::Duration::seconds(INTERNAL_DELETE_RETRY_SECONDS),
                )
                .await
            {
                // 清理声明本身仍然有效；即使延期失败，全局清理器也会在原租约到期后接管。
                tracing::error!(
                    file_id,
                    %defer_error,
                    "无法延期内部文件清理声明"
                );
            }
            return Err(map_storage_write_error(error));
        }

        if self
            .cleanup
            .complete_claim(tenant_id, file_id, &claim_token)
            .await?
        {
            return Ok(true);
        }

        // 元数据已被删除，或仍是不可恢复的清理墓碑，都表示业务文件已经完成删除。
        // 后一种情况由已经接管令牌的实例收尾，不允许将其误判为仍可使用的文件。
        match self.cleanup.find(tenant_id, file_id).await? {
            None => Ok(true),
            Some(current)
                if current.bucket == expected_bucket
                    && current.upload_status == FILE_UPLOAD_STATUS_CLEANUP =>
            {
                Ok(true)
            }
            Some(_) => Err(AppError::Conflict(
                "内部文件清理所有权已发生异常变化".into(),
            )),
        }
    }

    /// 将尚未被业务记录引用的配置包文件纳入可恢复的延迟清理。
    ///
    /// 调用方必须已经确认配置包、迁移和快照均未引用该文件；本方法会在租户锁下
    /// 再次锁定文件并仅允许受控 bucket，避免把客户端上传文件误作内部文件回收。
    pub async fn schedule_unreferenced_config_package_cleanup(
        &self,
        tenant_id: &str,
        file_id: i64,
    ) -> AppResult<bool> {
        const ORPHAN_GRACE_MINUTES: i64 = 15;
        let transaction = self.cleanup.begin().await?;
        transaction.lock_tenant(tenant_id).await?;
        let Some(file) = transaction.find_for_update(tenant_id, file_id).await? else {
            transaction.rollback().await?;
            return Ok(false);
        };
        if file.bucket != CONFIG_PACKAGE_BUCKET {
            transaction.rollback().await?;
            return Err(AppError::Authorization("内部文件存储边界不匹配".into()));
        }
        if file.upload_status != FILE_UPLOAD_STATUS_READY {
            transaction.rollback().await?;
            return Ok(false);
        }
        let now = transaction.database_now().await?;
        let marked = transaction
            .mark_unreferenced_config_package(
                tenant_id,
                file_id,
                now,
                now + chrono::Duration::minutes(ORPHAN_GRACE_MINUTES),
            )
            .await?;
        if marked {
            transaction.commit().await?;
        } else {
            transaction.rollback().await?;
        }
        Ok(marked)
    }

    fn upload_response_for_existing(existing: FileUploadRecord) -> UploadResponse {
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
        let policy = UploadPolicy {
            allowed_extensions: vec![
                "jpg".to_string(),
                "jpeg".to_string(),
                "png".to_string(),
                "gif".to_string(),
                "bmp".to_string(),
                "webp".to_string(),
            ],
            max_file_size,
        };

        let result = self
            .upload_single(
                actor,
                UploadCommand {
                    original_name,
                    data,
                    policy: &policy,
                    bucket: AVATAR_BUCKET,
                    compress: true,
                },
            )
            .await?;

        Ok(result)
    }
}

fn validate_extension(filename: &str, allowed: &[String]) -> AppResult<()> {
    let extension = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if allowed.is_empty() || allowed.contains(&extension) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "不支持的文件类型: .{extension}"
        )))
    }
}

fn generate_storage_filename(original_name: &str) -> String {
    let extension = original_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let id = uuid::Uuid::new_v4();
    if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    }
}

fn map_storage_write_error(error: ArtifactStoreError) -> AppError {
    match error.kind() {
        ArtifactStoreErrorKind::InvalidLocation => {
            AppError::Validation("非法的对象存储路径".into())
        }
        ArtifactStoreErrorKind::Misconfigured => AppError::Internal("对象存储配置错误".into()),
        ArtifactStoreErrorKind::Rejected | ArtifactStoreErrorKind::NotFound => {
            AppError::Internal("对象存储拒绝写入请求".into())
        }
        ArtifactStoreErrorKind::Unavailable => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
    }
}

fn map_storage_read_error(error: ArtifactStoreError) -> AppError {
    match error.kind() {
        ArtifactStoreErrorKind::NotFound => AppError::NotFound("文件不存在".into()),
        ArtifactStoreErrorKind::InvalidLocation => {
            AppError::Validation("非法的对象存储路径".into())
        }
        ArtifactStoreErrorKind::Misconfigured => AppError::Internal("对象存储配置错误".into()),
        ArtifactStoreErrorKind::Rejected => AppError::Internal("对象存储拒绝读取请求".into()),
        ArtifactStoreErrorKind::Unavailable => {
            AppError::ServiceUnavailable("对象存储暂不可用".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{map_storage_read_error, map_storage_write_error};
    use crate::{ArtifactStoreError, ArtifactStoreErrorKind};
    use ryframe_kernel::AppError;

    #[test]
    fn maps_unsupported_storage_operation_to_internal_error() {
        assert!(matches!(
            map_storage_write_error(ArtifactStoreError::new(
                ArtifactStoreErrorKind::Misconfigured,
                "list",
            )),
            AppError::Internal(_)
        ));
        assert!(matches!(
            map_storage_read_error(ArtifactStoreError::new(
                ArtifactStoreErrorKind::Misconfigured,
                "list",
            )),
            AppError::Internal(_)
        ));
    }

    #[test]
    fn maps_invalid_storage_response_to_service_unavailable() {
        assert!(matches!(
            map_storage_write_error(ArtifactStoreError::new(
                ArtifactStoreErrorKind::Unavailable,
                "truncated",
            )),
            AppError::ServiceUnavailable(_)
        ));
        assert!(matches!(
            map_storage_read_error(ArtifactStoreError::new(
                ArtifactStoreErrorKind::Unavailable,
                "truncated",
            )),
            AppError::ServiceUnavailable(_)
        ));
    }
}
