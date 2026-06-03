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
use serde::{Deserialize, Serialize};

use crate::handlers::auth_handler::AppState;

/// 默认上传 bucket 名称
const UPLOAD_BUCKET: &str = "uploads";

/// 文件上传响应
#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub file_url: String,
    pub file_info: UploadFileInfo,
}

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
        .with_state(state)
}

/// 下载文件路由（需认证）
pub fn download_router(state: AppState) -> Router {
    Router::new()
        .route("/download", get(download_file))
        .with_state(state)
}

/// 通用文件上传
pub async fn upload_file(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<UploadResponse>>> {
    let config = UploadConfig::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("读取 multipart 失败: {}", e)))?
    {
        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(format!("读取文件数据失败: {}", e)))?;

        // 验证文件大小
        if data.len() as u64 > config.max_file_size {
            return Err(AppError::Validation(format!(
                "文件大小超过限制（最大 {} MB）",
                config.max_file_size / 1024 / 1024
            )));
        }

        // 验证文件类型
        validate_extension(&filename, &config.allowed_extensions)?;

        // 生成存储文件名 + 日期路径
        let storage_name = generate_storage_filename(&filename);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{}/{}", date_prefix, storage_name);
        let content_type = get_content_type(&filename);

        // 通过对象存储保存（自动路由到本地/MinIO/S3）
        state
            .object_storage
            .put(UPLOAD_BUCKET, &object_key, &data, &content_type)
            .await
            .map_err(|e| AppError::Internal(format!("保存文件失败: {}", e)))?;

        // 生成文件访问 URL
        let file_url = build_file_url(&state.object_storage, UPLOAD_BUCKET, &object_key);

        let file_info = UploadFileInfo {
            original_name: filename.clone(),
            storage_name,
            file_path: format!("/{}", object_key),
            file_size: data.len() as u64,
            content_type,
            upload_time: Utc::now().to_rfc3339(),
        };

        return Ok(Json(ApiResponse::success(UploadResponse {
            file_url,
            file_info,
        })));
    }

    Err(AppError::Validation("未找到上传文件".into()))
}

/// 上传图片（仅允许图片类型）
pub async fn upload_image(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<UploadResponse>>> {
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

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("读取 multipart 失败: {}", e)))?
    {
        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(format!("读取文件数据失败: {}", e)))?;

        // 验证文件大小
        if data.len() as u64 > config.max_file_size {
            return Err(AppError::Validation(format!(
                "图片大小超过限制（最大 {} MB）",
                config.max_file_size / 1024 / 1024
            )));
        }

        // 验证文件类型
        validate_extension(&filename, &config.allowed_extensions)?;

        // 图片压缩（减小存储空间和带宽消耗）
        let (compressed_data, compressed_name) =
            compress_image(&data, &filename).unwrap_or_else(|e| {
                tracing::warn!("图片压缩失败，使用原始数据: {}", e);
                (data.to_vec(), filename.clone())
            });
        let original_size = data.len() as u64;
        let compressed_size = compressed_data.len() as u64;
        if compressed_size < original_size {
            let saved_pct = (1.0 - compressed_size as f64 / original_size as f64) * 100.0;
            tracing::info!(
                "图片压缩: {} → {} ({:.1}% 减小)",
                ryframe_common::utils::file_upload::format_file_size(original_size),
                ryframe_common::utils::file_upload::format_file_size(compressed_size),
                saved_pct
            );
        }

        // 生成存储文件名 + 日期路径
        let storage_name = generate_storage_filename(&compressed_name);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{}/{}", date_prefix, storage_name);
        let content_type = get_content_type(&compressed_name);

        // 通过对象存储保存（自动路由到本地/MinIO/S3）
        state
            .object_storage
            .put(UPLOAD_BUCKET, &object_key, &compressed_data, &content_type)
            .await
            .map_err(|e| AppError::Internal(format!("保存图片失败: {}", e)))?;

        // 生成文件访问 URL
        let file_url = build_file_url(&state.object_storage, UPLOAD_BUCKET, &object_key);

        let file_info = UploadFileInfo {
            original_name: filename.clone(),
            storage_name,
            file_path: format!("/{}", object_key),
            file_size: compressed_size,
            content_type,
            upload_time: Utc::now().to_rfc3339(),
        };

        return Ok(Json(ApiResponse::success(UploadResponse {
            file_url,
            file_info,
        })));
    }

    Err(AppError::Validation("未找到上传图片".into()))
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
