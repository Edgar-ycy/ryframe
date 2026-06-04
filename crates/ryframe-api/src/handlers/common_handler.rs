use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Multipart, Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use ryframe_common::{
    ApiResponse, AppError, AppResult,
    utils::{
        ObjectStorage,
        file_upload::{
            UploadConfig, UploadFileInfo, compress_image, generate_storage_filename,
            get_content_type, validate_extension,
        },
    },
};
use ryframe_core::repository::Repository;
use ryframe_db::{FileRepository, entities::sys_file};
use serde::{Deserialize, Serialize};

use crate::handlers::auth_handler::AppState;

/// 默认上传 bucket 名称
const UPLOAD_BUCKET: &str = "uploads";

/// Avatar 专用 bucket 名称
const AVATAR_BUCKET: &str = "avatar";

/// 文件上传响应
#[derive(Debug, Serialize)]
pub struct UploadResponse {
    /// sys_file 主键 ID
    pub file_id: i64,
    /// 文件访问 URL
    pub file_url: String,
    /// 文件详细信息
    pub file_info: UploadFileInfo,
}

/// 多文件上传响应
pub type MultiUploadResponse = Vec<UploadResponse>;

/// 文件下载查询参数
#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    /// 对象存储中的 key 路径
    pub path: String,
    /// bucket 名称（默认 uploads）
    #[serde(default = "default_bucket")]
    pub bucket: String,
}

fn default_bucket() -> String {
    UPLOAD_BUCKET.to_string()
}

/// 上传文件路由（公开）
pub fn upload_router(state: AppState) -> Router {
    Router::new()
        .route("/", post(upload_file))
        .route("/image", post(upload_image))
        .route("/avatar", post(upload_avatar))
        .with_state(state)
}

/// 下载文件路由（需认证）
pub fn download_router(state: AppState) -> Router {
    Router::new()
        .route("/download", get(download_file))
        .with_state(state)
}

// ==================== 核心上传处理 ====================

/// 通用文件上传（支持多文件、动态桶名）
///
/// 请求格式: multipart/form-data
/// - `bucket` 字段 (文本, 可选): 指定存储桶名称，默认 "uploads"
/// - `file` 字段 (文件, 可多个): 上传的文件
///
/// 响应: `ApiResponse<Vec<UploadResponse>>` — 每个文件一条记录
pub async fn upload_file(
    State(state): State<AppState>,
    multipart: Multipart,
) -> AppResult<Json<ApiResponse<MultiUploadResponse>>> {
    let config = UploadConfig::default();
    process_upload(state, multipart, config, None, false).await
}

/// 图片上传（仅允许图片类型，自动压缩）
///
/// 与 `upload_file` 相同，但仅接受图片类型且会自动压缩。
pub async fn upload_image(
    State(state): State<AppState>,
    multipart: Multipart,
) -> AppResult<Json<ApiResponse<MultiUploadResponse>>> {
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
    process_upload(state, multipart, config, None, true).await
}

/// 头像上传（仅允许图片，固定使用 `avatar` 桶）
///
/// 与 `upload_image` 相同，但 bucket 固定为 "avatar"，不可通过表单修改。
pub async fn upload_avatar(
    State(state): State<AppState>,
    multipart: Multipart,
) -> AppResult<Json<ApiResponse<MultiUploadResponse>>> {
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
    process_upload(
        state,
        multipart,
        config,
        Some(AVATAR_BUCKET.to_string()),
        true,
    )
    .await
}

/// 统一上传处理函数
///
/// - 解析 multipart 中的 `bucket` 字段（如 `force_bucket` 不为 None 则忽略）
/// - 遍历所有 `file` 字段，逐个：验证 → 上传对象存储 → 写入 sys_file
/// - `compress` 参数控制是否对图片进行压缩
async fn process_upload(
    state: AppState,
    mut multipart: Multipart,
    config: UploadConfig,
    force_bucket: Option<String>,
    compress: bool,
) -> AppResult<Json<ApiResponse<MultiUploadResponse>>> {
    let mut form_bucket = String::new();
    let mut results: Vec<UploadResponse> = Vec::new();
    let mut bucket_ensured = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("读取 multipart 失败: {}", e)))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        // 提取 bucket 名称（文本字段 "bucket"）
        if field_name == "bucket" && force_bucket.is_none() {
            form_bucket = field
                .text()
                .await
                .map_err(|e| AppError::Internal(format!("读取 bucket 字段失败: {}", e)))?;
            continue;
        }

        // 处理文件字段
        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(format!("读取文件数据失败: {}", e)))?;

        // 确定最终的 bucket
        let effective_bucket = resolve_bucket(&force_bucket, &form_bucket);

        // 确保 bucket 存在（每个请求只检查一次）
        if !bucket_ensured {
            state
                .object_storage
                .ensure_bucket(&effective_bucket)
                .await
                .map_err(|e| AppError::Internal(format!("创建存储桶失败: {}", e)))?;
            bucket_ensured = true;
        }

        // 验证文件大小
        if data.len() as u64 > config.max_file_size {
            return Err(AppError::Validation(format!(
                "文件大小超过限制（最大 {} MB）",
                config.max_file_size / 1024 / 1024
            )));
        }

        // 验证文件类型
        validate_extension(&filename, &config.allowed_extensions)?;

        // 图片压缩（可选）
        let (final_data, final_name, content_type) = if compress {
            let (compressed, compressed_name) =
                compress_image(&data, &filename).unwrap_or_else(|e| {
                    tracing::warn!("图片压缩失败，使用原始数据: {}", e);
                    (data.to_vec(), filename.clone())
                });
            let original_size = data.len() as u64;
            let compressed_size = compressed.len() as u64;
            if compressed_size < original_size {
                let saved_pct = (1.0 - compressed_size as f64 / original_size as f64) * 100.0;
                tracing::info!(
                    "图片压缩: {} → {} ({:.1}% 减小)",
                    ryframe_common::utils::file_upload::format_file_size(original_size),
                    ryframe_common::utils::file_upload::format_file_size(compressed_size),
                    saved_pct
                );
            }
            let ct = get_content_type(&compressed_name);
            (compressed, compressed_name, ct)
        } else {
            let ct = get_content_type(&filename);
            (data.to_vec(), filename.clone(), ct)
        };

        // 生成存储文件名 + 日期路径
        let storage_name = generate_storage_filename(&final_name);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{}/{}", date_prefix, storage_name);

        // 通过对象存储保存
        state
            .object_storage
            .put(&effective_bucket, &object_key, &final_data, &content_type)
            .await
            .map_err(|e| AppError::Internal(format!("保存文件失败: {}", e)))?;

        // 生成文件访问 URL（动态根据配置拼接，用于 API 响应）
        let public_file_url = build_file_url(&state.object_storage, &effective_bucket, &object_key);

        // 写入文件元数据到 sys_file 表
        // file_url 仅存相对路径（bucket/date/uuid.ext），不含协议/域名/端口
        let relative_file_url = format!("{}/{}", effective_bucket, object_key);
        let file_id = ryframe_common::utils::snowflake::next_snowflake_id();
        let model = sys_file::Model {
            id: file_id,
            original_name: filename.clone(),
            storage_name: storage_name.clone(),
            storage_path: object_key.clone(),
            bucket: effective_bucket.clone(),
            file_url: relative_file_url,
            file_size: final_data.len() as i64,
            content_type: content_type.clone(),
            file_md5: None,
            upload_by: None,
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        FileRepository
            .insert(&state.db, model)
            .await
            .map_err(|e| AppError::Internal(format!("写入文件元数据失败: {}", e)))?;

        results.push(UploadResponse {
            file_id,
            file_url: public_file_url,
            file_info: UploadFileInfo {
                original_name: filename.clone(),
                storage_name,
                file_path: object_key.clone(),
                file_size: final_data.len() as u64,
                content_type,
                upload_time: Utc::now().to_rfc3339(),
            },
        });
    }

    if results.is_empty() {
        return Err(AppError::Validation("未找到上传文件".into()));
    }

    Ok(Json(ApiResponse::success(results)))
}

/// 解析最终使用的 bucket 名称
fn resolve_bucket(force_bucket: &Option<String>, form_bucket: &str) -> String {
    if let Some(fb) = force_bucket {
        fb.clone()
    } else if form_bucket.is_empty() {
        UPLOAD_BUCKET.to_string()
    } else {
        form_bucket.to_string()
    }
}

/// 下载文件
pub async fn download_file(
    State(state): State<AppState>,
    Query(query): Query<DownloadQuery>,
) -> AppResult<impl IntoResponse> {
    // 安全检查：防止路径穿越
    if query.path.contains("..") {
        return Err(AppError::Validation("非法的文件路径".into()));
    }

    let bucket = if query.bucket.is_empty() {
        UPLOAD_BUCKET.to_string()
    } else {
        query.bucket
    };

    // 通过对象存储读取（自动路由到本地/MinIO/S3）
    let data = state
        .object_storage
        .get(&bucket, &query.path)
        .await
        .map_err(|e| AppError::NotFound(format!("文件不存在: {}", e)))?;

    // 获取文件名和 MIME 类型
    let filename = query.path.rsplit('/').next().unwrap_or("download");
    let content_type = get_content_type(filename);

    // 构建响应头
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        content_type
            .parse()
            .map_err(|e| AppError::Internal(format!("设置 Content-Type 失败: {}", e)))?,
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .map_err(|e| AppError::Internal(format!("设置 Content-Disposition 失败: {}", e)))?,
    );

    Ok((headers, data))
}

/// 构建文件访问 URL
/// - S3/MinIO：返回对象的 public_url（直接访问云存储）
/// - 本地：如果 public_url 为空，返回代理下载 URL
fn build_file_url(storage: &Arc<dyn ObjectStorage>, bucket: &str, key: &str) -> String {
    let public_url = storage.public_url(bucket, key);
    // 本地模式且 public_url 为空时，使用代理下载 URL
    if public_url.is_empty() || public_url == "/" {
        format!(
            "/api/v1/common/file/download?bucket={}&path={}",
            bucket, key
        )
    } else {
        public_url
    }
}
