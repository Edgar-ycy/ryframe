use axum::{
    Json, Router,
    extract::{Multipart, Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use ryframe_common::utils::file_upload::{
    UploadConfig, UploadFileInfo, generate_storage_filename, get_content_type, get_upload_dir,
    validate_extension,
};
use ryframe_common::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::handlers::auth_handler::AppState;

/// 文件上传响应
#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub file_url: String,
    pub file_info: UploadFileInfo,
}

/// 文件下载查询参数
#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    pub path: String,
}

/// 上传文件路由
pub fn upload_router(state: AppState) -> Router {
    Router::new()
        .route("/", post(upload_file))
        .route("/image", post(upload_image))
        .route("/download", get(download_file))
        .with_state(state)
}

/// 通用文件上传
pub async fn upload_file(
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<UploadResponse>> {
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

        // 生成存储文件名
        let storage_name = generate_storage_filename(&filename);

        // 创建上传目录
        let upload_dir = get_upload_dir(&config.upload_dir);
        tokio::fs::create_dir_all(&upload_dir)
            .await
            .map_err(|e| AppError::Internal(format!("创建上传目录失败: {}", e)))?;

        // 保存文件
        let file_path = upload_dir.join(&storage_name);
        tokio::fs::write(&file_path, &data)
            .await
            .map_err(|e| AppError::Internal(format!("保存文件失败: {}", e)))?;

        // 构建响应
        let relative_path = file_path
            .strip_prefix(config.upload_dir)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();

        let file_info = UploadFileInfo {
            original_name: filename.clone(),
            storage_name: storage_name.clone(),
            file_path: format!("/{}", relative_path.replace('\\', "/")),
            file_size: data.len() as u64,
            content_type: get_content_type(&filename),
            upload_time: Utc::now().to_rfc3339(),
        };

        let file_url = format!(
            "/api/v1/common/file/download?path={}",
            relative_path.replace('\\', "/")
        );

        return Ok(Json(UploadResponse {
            file_url,
            file_info,
        }));
    }

    Err(AppError::Validation("未找到上传文件".into()))
}

/// 上传图片（仅允许图片类型）
pub async fn upload_image(
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<UploadResponse>> {
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

        // 生成存储文件名
        let storage_name = generate_storage_filename(&filename);

        // 创建上传目录
        let upload_dir = get_upload_dir(&config.upload_dir);
        tokio::fs::create_dir_all(&upload_dir)
            .await
            .map_err(|e| AppError::Internal(format!("创建上传目录失败: {}", e)))?;

        // 保存文件
        let file_path = upload_dir.join(&storage_name);
        tokio::fs::write(&file_path, &data)
            .await
            .map_err(|e| AppError::Internal(format!("保存文件失败: {}", e)))?;

        // 构建响应
        let relative_path = file_path
            .strip_prefix(config.upload_dir)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();

        let file_info = UploadFileInfo {
            original_name: filename.clone(),
            storage_name: storage_name.clone(),
            file_path: format!("/{}", relative_path.replace('\\', "/")),
            file_size: data.len() as u64,
            content_type: get_content_type(&filename),
            upload_time: Utc::now().to_rfc3339(),
        };

        let file_url = format!(
            "/api/v1/common/file/download?path={}",
            relative_path.replace('\\', "/")
        );

        return Ok(Json(UploadResponse {
            file_url,
            file_info,
        }));
    }

    Err(AppError::Validation("未找到上传图片".into()))
}

/// 下载文件
pub async fn download_file(
    State(_state): State<AppState>,
    Query(query): Query<DownloadQuery>,
) -> AppResult<impl IntoResponse> {
    let config = UploadConfig::default();

    // 安全检查：防止路径穿越
    if query.path.contains("..") {
        return Err(AppError::Validation("非法的文件路径".into()));
    }

    let file_path = PathBuf::from(&config.upload_dir).join(&query.path);

    // 检查文件是否存在
    if !file_path.exists() {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    // 读取文件
    let data = tokio::fs::read(&file_path)
        .await
        .map_err(|e| AppError::Internal(format!("读取文件失败: {}", e)))?;

    // 获取 MIME 类型
    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
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
