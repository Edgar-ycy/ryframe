use std::sync::Arc;

use chrono::Utc;
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
use ryframe_storage::ObjectStorage;
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

    /// 确保 bucket 存在
    pub async fn ensure_bucket(&self, bucket: &str) -> AppResult<()> {
        self.storage
            .ensure_bucket(bucket)
            .await
            .map_err(|e| AppError::Internal(format!("创建存储桶失败: {}", e)))
    }

    /// Validate that the storage backend is reachable with the configured credentials.
    pub async fn check_storage(&self) -> AppResult<()> {
        for bucket in [UPLOAD_BUCKET, AVATAR_BUCKET] {
            self.ensure_bucket(bucket).await?;
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
            return Err(AppError::Validation(format!(
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

        let public_file_url = self.build_file_url(bucket, &object_key)?;

        // 上传到对象存储
        self.storage
            .put(bucket, &object_key, &final_data, &content_type)
            .await
            .map_err(|e| AppError::Internal(format!("保存文件失败: {}", e)))?;

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
            return Err(AppError::Internal(format!("写入文件元数据失败: {error}")));
        }

        Ok(UploadResponse {
            file_id: file_id.to_string(),
            file_url: public_file_url,
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

        FileRepository
            .find_by_storage_path(self.db.read(), tenant_id, bucket, path)
            .await?
            .ok_or_else(|| AppError::NotFound("文件不存在".into()))?;

        let data = self
            .storage
            .get(bucket, path)
            .await
            .map_err(|e| AppError::NotFound(format!("文件不存在: {}", e)))?;

        let filename = path.rsplit('/').next().unwrap_or("download").to_string();

        Ok((data, filename))
    }

    /// 构建文件访问 URL
    ///
    /// - RustFS/MinIO/S3：返回对象的 public_url（直接访问对象存储）
    /// - 本地：如果 public_url 为空，返回代理下载 URL
    pub fn build_file_url(&self, bucket: &str, key: &str) -> AppResult<String> {
        match self
            .storage
            .public_url(bucket, key)
            .map_err(|error| AppError::Validation(error.to_string()))?
        {
            Some(public_url) => Ok(public_url),
            None => Ok(format!(
                "/api/v1/common/file/download?bucket={}&path={}",
                bucket, key
            )),
        }
    }

    /// 解析最终使用的 bucket 名称
    pub fn resolve_bucket(force_bucket: &Option<String>, form_bucket: &str) -> String {
        if let Some(fb) = force_bucket {
            fb.clone()
        } else if form_bucket.is_empty() {
            UPLOAD_BUCKET.to_string()
        } else {
            form_bucket.to_string()
        }
    }

    /// 上传头像（Avatar 专用便捷方法）
    ///
    /// 固定使用 `avatar` bucket、图片类型、5MB 限制、自动压缩。
    /// 返回公开访问 URL（用于更新 sys_user.avatar）。
    pub async fn upload_avatar(
        &self,
        actor: &ActorContext,
        original_name: String,
        data: Vec<u8>,
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
            max_file_size: 5 * 1024 * 1024, // 5MB
            ..Default::default()
        };

        self.ensure_bucket(AVATAR_BUCKET).await?;

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
