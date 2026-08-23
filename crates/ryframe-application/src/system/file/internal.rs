use super::*;

impl FileService {
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
            transaction
                .commit(crate::TransactionAuditMode::Skip)
                .await?;
        } else {
            transaction.rollback().await?;
        }
        Ok(marked)
    }

    pub(super) fn upload_response_for_existing(existing: FileUploadRecord) -> UploadResponse {
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
