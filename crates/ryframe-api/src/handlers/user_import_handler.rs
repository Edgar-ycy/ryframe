use axum::{
    Json, Router,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_excel::ExcelImporter;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, route};
use ryframe_service::system::{RequestUserImportCommand, UserImportData, UserImportListParams};
use sha2::{Digest, Sha256};

use crate::{
    dto::{
        empty_dto::EmptyRequestDto,
        public_dto::{UserImportJobVo, UserImportRowVo},
        user_import_dto::{UserImportPageQuery, UserImportRowPageQuery, UserImportUploadForm},
    },
    handler_utils::{excel_response, idempotency_key_hash},
    state::AppState,
};

pub fn user_import_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(create))
        .merge(route!(list))
        .merge(route!(detail))
        .merge(route!(cancel))
        .merge(route!(rows))
        .merge(route!(report))
        .with_state(state)
}

#[post("/")]
#[perm("system:user-import:add")]
#[utoipa::path(post, path = "/api/v1/system/user-imports", tag = "用户导入",
    params(("Idempotency-Key" = String, Header, description = "用户导入幂等键")),
    request_body(content = UserImportUploadForm, content_type = "multipart/form-data"),
    responses((status = 202, description = "用户导入任务已创建", body = ApiResponse<UserImportJobVo>)),
    security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> HttpResult<(StatusCode, Json<ApiResponse<UserImportJobVo>>)> {
    let idempotency_hash = idempotency_key_hash(&headers)?;
    if let Some(existing) = state
        .services
        .user_import
        .find_by_idempotency(&current_user, &idempotency_hash)
        .await
        .map_err(ryframe_http::HttpAppError::from)?
    {
        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::success(existing.into())),
        ));
    }

    let mut source = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Validation(format!("读取上传表单失败: {error}")))?
    {
        if field.name() != Some("file") {
            return Err(AppError::Validation("上传表单只允许 file 字段".into()).into());
        }
        if source.is_some() {
            return Err(AppError::Validation("只能上传一个用户导入文件".into()).into());
        }
        let file_name = field
            .file_name()
            .map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| AppError::Validation("用户导入文件名不能为空".into()))?;
        if !file_name.to_ascii_lowercase().ends_with(".xlsx") {
            return Err(AppError::Validation("用户导入只接受 .xlsx 文件".into()).into());
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|error| AppError::Validation(format!("读取用户导入文件失败: {error}")))?
            .to_vec();
        if bytes.is_empty() {
            return Err(AppError::Validation("用户导入文件不能为空".into()).into());
        }
        if bytes.len() > state.config.user_import.max_file_bytes {
            return Err(AppError::PayloadTooLarge(format!(
                "用户导入文件超过 {} 字节上限",
                state.config.user_import.max_file_bytes
            ))
            .into());
        }
        let validation_bytes = bytes.clone();
        tokio::task::spawn_blocking(move || {
            ExcelImporter::validate_headers_from_bytes(
                &validation_bytes,
                None,
                UserImportData::excel_headers(),
            )
        })
        .await
        .map_err(|error| AppError::Internal(format!("XLSX 内容校验任务异常结束: {error}")))??;
        source = Some((file_name, bytes));
    }
    let (file_name, bytes) =
        source.ok_or_else(|| AppError::Validation("未找到 file 上传字段".into()))?;
    let source_sha256 = hex::encode(Sha256::digest(&bytes));
    let uploaded = state
        .services
        .user_import
        .upload_source(&current_user, file_name.clone(), bytes)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    let source_file_id = uploaded
        .file_id
        .parse::<i64>()
        .map_err(|_| AppError::Internal("用户导入源文件标识无效".into()))?;
    let outcome = match state
        .services
        .user_import
        .request(
            &current_user,
            RequestUserImportCommand {
                idempotency_key_hash: idempotency_hash,
                source_file_id,
                source_name: file_name,
                source_sha256,
            },
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            if let Err(cleanup_error) = state
                .services
                .user_import
                .schedule_unreferenced_source_cleanup(&current_user, source_file_id)
                .await
            {
                tracing::error!(
                    file_id = source_file_id,
                    %cleanup_error,
                    "用户导入任务创建失败后无法安排孤儿源文件回收"
                );
            }
            return Err(ryframe_http::HttpAppError::from(error));
        }
    };
    if !outcome.inserted
        && let Err(cleanup_error) = state
            .services
            .user_import
            .schedule_unreferenced_source_cleanup(&current_user, source_file_id)
            .await
    {
        tracing::error!(
            file_id = source_file_id,
            %cleanup_error,
            "用户导入幂等重放后无法安排未引用源文件回收"
        );
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(outcome.job.into())),
    ))
}

#[get("/")]
#[perm("system:user-import:list")]
#[utoipa::path(get, path = "/api/v1/system/user-imports", tag = "用户导入",
    params(UserImportPageQuery),
    responses((status = 200, description = "用户导入任务列表", body = ApiPageResponse<UserImportJobVo>)),
    security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<UserImportPageQuery>,
) -> HttpResult<Json<ApiPageResponse<UserImportJobVo>>> {
    let status = query.status.clone();
    let page = state
        .services
        .user_import
        .list(
            &current_user,
            UserImportListParams {
                page: query.into_page(&state.config.pagination)?,
                status,
            },
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[get("/{id}")]
#[perm("system:user-import:list")]
#[utoipa::path(get, path = "/api/v1/system/user-imports/{id}", tag = "用户导入",
    params(("id" = String, Path, description = "用户导入任务 ID")),
    responses((status = 200, description = "用户导入任务详情", body = ApiResponse<UserImportJobVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<UserImportJobVo>>> {
    state
        .services
        .user_import
        .get(&current_user, parse_import_id(&id)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(UserImportJobVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[post("/{id}/cancel")]
#[perm("system:user-import:cancel")]
#[utoipa::path(post, path = "/api/v1/system/user-imports/{id}/cancel", tag = "用户导入",
    params(("id" = String, Path, description = "用户导入任务 ID")),
    request_body = EmptyRequestDto,
    responses((status = 200, description = "已申请取消用户导入", body = ApiResponse<UserImportJobVo>)),
    security(("bearer" = [])))]
async fn cancel(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(_request): Json<EmptyRequestDto>,
) -> HttpResult<Json<ApiResponse<UserImportJobVo>>> {
    state
        .services
        .user_import
        .cancel(&current_user, parse_import_id(&id)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(UserImportJobVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[get("/{id}/rows")]
#[perm("system:user-import:list")]
#[utoipa::path(get, path = "/api/v1/system/user-imports/{id}/rows", tag = "用户导入",
    params(("id" = String, Path, description = "用户导入任务 ID"), UserImportRowPageQuery),
    responses((status = 200, description = "用户导入异常行", body = ApiPageResponse<UserImportRowVo>)),
    security(("bearer" = [])))]
async fn rows(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Query(query): Query<UserImportRowPageQuery>,
) -> HttpResult<Json<ApiPageResponse<UserImportRowVo>>> {
    let page = state
        .services
        .user_import
        .rows(
            &current_user,
            parse_import_id(&id)?,
            query.into_page(&state.config.pagination)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[get("/{id}/report")]
#[perm("system:user-import:list")]
#[utoipa::path(get, path = "/api/v1/system/user-imports/{id}/report", tag = "用户导入",
    params(("id" = String, Path, description = "用户导入任务 ID")),
    responses((status = 200, description = "用户导入错误报告", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")),
    security(("bearer" = [])))]
async fn report(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<axum::response::Response> {
    let file = state
        .services
        .user_import
        .download_report(&current_user, parse_import_id(&id)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    excel_response(file.data, &file.original_name)
}

fn parse_import_id(value: &str) -> HttpResult<i64> {
    Ok(value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("用户导入任务 ID 必须是正整数".into()))?)
}
