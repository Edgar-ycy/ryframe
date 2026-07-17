use axum::{
    Json, Router,
    extract::{Multipart, Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use ryframe_common::{
    ApiResponse, AppError, AppResult,
    utils::file_upload::{UploadConfig, get_content_type},
};
use ryframe_macro::{get, post, route};
use ryframe_service::system::file_service::{
    AVATAR_BUCKET, FileService, UPLOAD_BUCKET, UploadResponse,
};
use serde::Deserialize;

use crate::dto::multipart_dto::FileUploadForm;
use crate::state::AppState;
use ryframe_auth::RequestPrincipal;

/// 多文件上传响应
pub type MultiUploadResponse = Vec<UploadResponse>;

/// 文件下载查询参数
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
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
        .merge(route!(upload_file))
        .merge(route!(upload_image))
        .merge(route!(upload_avatar))
        .with_state(state)
}

/// 下载文件路由（需认证）
pub fn download_router(state: AppState) -> Router {
    Router::new().merge(route!(download_file)).with_state(state)
}

// ==================== 上传接口（薄层：仅解析 HTTP 参数，委托 Service） ====================

/// 通用文件上传（支持多文件、动态桶名）
#[post("/")]
#[utoipa::path(post, path = "/api/v1/common/upload", tag = "通用",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "上传成功", body = ApiResponse<Vec<UploadResponse>>)), security(("bearer" = [])))]
pub async fn upload_file(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    multipart: Multipart,
) -> AppResult<Json<ApiResponse<MultiUploadResponse>>> {
    let config = UploadConfig::default();
    process_multipart_upload(state, multipart, &config, None, false, current_user).await
}

/// 图片上传（仅允许图片类型，自动压缩）
#[post("/image")]
#[utoipa::path(post, path = "/api/v1/common/upload/image", tag = "通用",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "图片上传成功", body = ApiResponse<Vec<UploadResponse>>)), security(("bearer" = [])))]
pub async fn upload_image(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
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
    process_multipart_upload(state, multipart, &config, None, true, current_user).await
}

/// 头像上传（固定使用 `avatar` 桶）
#[post("/avatar")]
#[utoipa::path(post, path = "/api/v1/common/upload/avatar", tag = "通用",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "头像上传成功", body = ApiResponse<Vec<UploadResponse>>)), security(("bearer" = [])))]
pub async fn upload_avatar(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
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
        max_file_size: 5 * 1024 * 1024,
        ..Default::default()
    };
    process_multipart_upload(
        state,
        multipart,
        &config,
        Some(AVATAR_BUCKET.to_string()),
        true,
        current_user,
    )
    .await
}

/// 解析 multipart 中的文件并逐文件委托 FileService 处理
async fn process_multipart_upload(
    state: AppState,
    mut multipart: Multipart,
    config: &UploadConfig,
    force_bucket: Option<String>,
    compress: bool,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<MultiUploadResponse>>> {
    if !state.runtime.upload_circuit_breaker.allow_request() {
        return Err(AppError::Conflict(
            "文件上传服务暂时不可用，请稍后再试".into(),
        ));
    }

    let mut form_bucket = String::new();
    let mut results: MultiUploadResponse = Vec::new();
    let mut bucket_ensured = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("读取 multipart 失败: {}", e)))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        // 提取 bucket 名称
        if field_name == "bucket" && force_bucket.is_none() {
            form_bucket = field
                .text()
                .await
                .map_err(|e| AppError::Internal(format!("读取 bucket 字段失败: {}", e)))?;
            continue;
        }

        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(format!("读取文件数据失败: {}", e)))?;

        let effective_bucket = FileService::resolve_bucket(&force_bucket, &form_bucket);

        // 确保 bucket 存在（每请求一次）
        if !bucket_ensured {
            state.services.file.ensure_bucket(&effective_bucket).await?;
            bucket_ensured = true;
        }

        // 委托 FileService 处理业务逻辑
        let result = match state
            .services
            .file
            .upload_single(
                &current_user,
                ryframe_service::system::UploadCommand {
                    original_name: filename,
                    data: data.to_vec(),
                    config,
                    bucket: &effective_bucket,
                    compress,
                },
            )
            .await
        {
            Ok(result) => {
                state.runtime.upload_circuit_breaker.record_success();
                result
            }
            Err(err) => {
                state.runtime.upload_circuit_breaker.record_failure();
                return Err(err);
            }
        };

        results.push(result);
    }

    if results.is_empty() {
        return Err(AppError::Validation("未找到上传文件".into()));
    }

    Ok(Json(ApiResponse::success(results)))
}

/// 下载文件（薄层：HTTP 参数提取 + 构建响应头，业务委托 FileService）
#[get("/download")]
#[utoipa::path(get, path = "/api/v1/common/file/download", tag = "通用",
    params(DownloadQuery),
    responses((status = 200, description = "文件下载", body = Vec<u8>, content_type = "application/octet-stream")), security(("bearer" = [])))]
pub async fn download_file(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<DownloadQuery>,
) -> AppResult<impl IntoResponse> {
    let bucket = if query.bucket.is_empty() {
        UPLOAD_BUCKET.to_string()
    } else {
        query.bucket
    };

    let (data, filename) = state
        .services
        .file
        .download(&current_user, &bucket, &query.path)
        .await?;

    let content_type = get_content_type(&filename);

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
