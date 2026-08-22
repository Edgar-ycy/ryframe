use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Multipart, Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use ryframe_application::system::file::{
    AVATAR_BUCKET, DownloadedFile, UPLOAD_BUCKET, UploadPolicy,
};
use ryframe_kernel::{AppError, AppResult as KernelAppResult};
use ryframe_macro::{get, post, route};
use serde::Deserialize;

use crate::RequestPrincipal;
use crate::dto::{multipart_dto::FileUploadForm, public_dto::UploadResponse};
use crate::{
    handler_utils::attachment_content_disposition, runtime::UploadCircuitBreaker, state::AppState,
};

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

fn upload_failure_affects_circuit_breaker(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Database(_) | AppError::ServiceUnavailable(_)
    )
}

pub fn record_upload_result<T>(
    circuit_breaker: &dyn UploadCircuitBreaker,
    result: &KernelAppResult<T>,
) {
    match result {
        Ok(_) => circuit_breaker.record_success(),
        Err(error) if upload_failure_affects_circuit_breaker(error) => {
            circuit_breaker.record_failure();
        }
        Err(_) => {}
    }
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

/// 通用文件上传（固定私有 `uploads` 桶）
#[post("/")]
#[utoipa::path(post, path = "/api/v1/common/upload", tag = "通用",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "上传成功", body = ApiResponse<Vec<UploadResponse>>),
        (status = 413, description = "上传内容超过 10 MiB 限制"),
        (status = 503, description = "对象存储暂不可用")
    ), security(("bearer" = [])))]
pub async fn upload_file(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    multipart: Multipart,
) -> HttpResult<Json<ApiResponse<MultiUploadResponse>>> {
    let policy = UploadPolicy {
        max_file_size: state.settings.upload.file_max_bytes as u64,
        ..Default::default()
    };
    process_multipart_upload(
        state,
        multipart,
        &policy,
        UPLOAD_BUCKET,
        false,
        current_user,
    )
    .await
}

/// 图片上传（仅允许图片类型，自动压缩）
#[post("/image")]
#[utoipa::path(post, path = "/api/v1/common/upload/image", tag = "通用",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "图片上传成功", body = ApiResponse<Vec<UploadResponse>>),
        (status = 413, description = "上传内容超过 10 MiB 限制"),
        (status = 503, description = "对象存储暂不可用")
    ), security(("bearer" = [])))]
pub async fn upload_image(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    multipart: Multipart,
) -> HttpResult<Json<ApiResponse<MultiUploadResponse>>> {
    let policy = UploadPolicy {
        allowed_extensions: vec![
            "jpg".to_string(),
            "jpeg".to_string(),
            "png".to_string(),
            "gif".to_string(),
            "bmp".to_string(),
            "webp".to_string(),
        ],
        max_file_size: state.settings.upload.file_max_bytes as u64,
    };
    process_multipart_upload(state, multipart, &policy, UPLOAD_BUCKET, true, current_user).await
}

/// 头像上传（固定使用 `avatar` 桶）
#[post("/avatar")]
#[utoipa::path(post, path = "/api/v1/common/upload/avatar", tag = "通用",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "头像上传成功", body = ApiResponse<Vec<UploadResponse>>),
        (status = 413, description = "上传内容超过 5 MiB 限制"),
        (status = 503, description = "对象存储暂不可用")
    ), security(("bearer" = [])))]
pub async fn upload_avatar(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    multipart: Multipart,
) -> HttpResult<Json<ApiResponse<MultiUploadResponse>>> {
    let policy = UploadPolicy {
        allowed_extensions: vec![
            "jpg".to_string(),
            "jpeg".to_string(),
            "png".to_string(),
            "gif".to_string(),
            "bmp".to_string(),
            "webp".to_string(),
        ],
        max_file_size: state.settings.upload.avatar_max_bytes as u64,
    };
    process_multipart_upload(state, multipart, &policy, AVATAR_BUCKET, true, current_user).await
}

/// 解析 multipart 中的文件并逐文件委托 FileService 处理
async fn process_multipart_upload(
    state: AppState,
    mut multipart: Multipart,
    policy: &UploadPolicy,
    bucket: &'static str,
    compress: bool,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<MultiUploadResponse>>> {
    if !state.runtime.upload_circuit_breaker.allow_request() {
        return Err(
            AppError::ServiceUnavailable("文件上传服务暂时不可用，请稍后再试".into()).into(),
        );
    }

    let mut results: MultiUploadResponse = Vec::new();
    let mut total_file_bytes = 0_u64;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("读取 multipart 失败: {}", e)))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        if field_name == "bucket" {
            return Err(AppError::Validation("v0.5 不允许客户端选择对象存储 bucket".into()).into());
        }

        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(format!("读取文件数据失败: {}", e)))?;

        total_file_bytes = total_file_bytes.saturating_add(data.len() as u64);
        if total_file_bytes > policy.max_file_size {
            return Err(AppError::PayloadTooLarge(format!(
                "单次上传文件总大小超过限制（最大 {} MiB）",
                policy.max_file_size / 1024 / 1024
            ))
            .into());
        }

        // 委托 FileService 处理业务逻辑
        let result = state
            .services
            .file
            .upload_single(
                &current_user,
                ryframe_application::system::UploadCommand {
                    original_name: filename,
                    data: data.to_vec(),
                    policy,
                    bucket,
                    compress,
                },
            )
            .await;
        record_upload_result(state.runtime.upload_circuit_breaker.as_ref(), &result);
        let result = result?;

        results.push(result.into());
    }

    if results.is_empty() {
        return Err(AppError::Validation("未找到上传文件".into()).into());
    }

    Ok(Json(ApiResponse::success(results)))
}

/// 下载文件（薄层：HTTP 参数提取 + 构建响应头，业务委托 FileService）
#[get("/download")]
#[utoipa::path(get, path = "/api/v1/common/file/download", tag = "通用",
    params(DownloadQuery),
    responses(
        (status = 200, description = "文件下载", body = Vec<u8>, content_type = "application/octet-stream"),
        (status = 404, description = "文件或对象不存在"),
        (status = 503, description = "对象存储暂不可用")
    ), security(("bearer" = [])))]
pub async fn download_file(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<DownloadQuery>,
) -> HttpResult<impl IntoResponse> {
    let bucket = if query.bucket.is_empty() {
        UPLOAD_BUCKET
    } else if matches!(query.bucket.as_str(), UPLOAD_BUCKET | AVATAR_BUCKET) {
        query.bucket.as_str()
    } else {
        return Err(AppError::Validation("不允许访问未知文件 bucket".into()).into());
    };

    let file = state
        .services
        .file
        .download(&current_user, bucket, &query.path)
        .await?;

    downloaded_file_response(file)
}

fn downloaded_file_response(file: DownloadedFile) -> HttpResult<(HeaderMap, Vec<u8>)> {
    // 内容类型和原始名称均来自数据库元数据，不能根据对象键或扩展名重新推断。
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        file.content_type
            .parse()
            .map_err(|e| AppError::Internal(format!("设置 Content-Type 失败: {}", e)))?,
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        attachment_content_disposition(&file.original_name)?,
    );

    Ok((headers, file.data))
}
