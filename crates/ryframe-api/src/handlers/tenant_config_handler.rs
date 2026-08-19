use crate::RequestPrincipal;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header::CONTENT_LENGTH},
};
use ryframe_application::system::ApplyTenantConfigTransferCommand;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, route};

use crate::{
    dto::{
        empty_dto::EmptyRequestDto,
        public_dto::{TenantConfigBundleVo, TenantConfigTransferItemVo, TenantConfigTransferVo},
        tenant_config_dto::{
            ApplyTenantConfigTransferDto, CreateTenantConfigTransferDto,
            TenantConfigPackageUploadForm, TenantConfigPageQuery,
        },
    },
    handler_utils::{attachment_response, idempotency_key_hash},
    state::AppState,
};

const CONFIG_PACKAGE_CONTENT_TYPE: &str = "application/zip";

pub fn config_package_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(request_package_export))
        .merge(route!(list_packages))
        .merge(route!(get_package))
        .merge(route!(download_package))
        .with_state(state)
}

pub fn config_transfer_router(state: AppState) -> Router {
    Router::new()
        // 仅关闭 Multipart 提取器内置的 2 MiB 默认限制；外层统一请求体限制和
        // Handler 内 5 MiB 配置包限制仍会先后执行。
        .merge(route!(upload_transfer).layer(DefaultBodyLimit::disable()))
        .merge(route!(create_transfer_from_package))
        .merge(route!(list_transfers))
        .merge(route!(get_transfer))
        .merge(route!(list_transfer_items))
        .merge(route!(request_preview))
        .merge(route!(request_apply))
        .merge(route!(request_rollback))
        .with_state(state)
}

#[post("/")]
#[perm("system:config-package:export")]
#[utoipa::path(post, path = "/api/v1/system/config-packages", tag = "租户配置迁移",
    params(("Idempotency-Key" = String, Header, description = "配置包导出幂等键")),
    responses(
        (status = 202, description = "配置包导出任务已创建", body = ApiResponse<TenantConfigBundleVo>),
        (status = 400, description = "幂等键格式无效"),
        (status = 403, description = "没有配置包导出权限"),
        (status = 409, description = "同一幂等键对应不同请求"),
        (status = 503, description = "数据库、对象存储或后台任务服务不可用")
    ),
    security(("bearer" = [])))]
async fn request_package_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
) -> HttpResult<(StatusCode, Json<ApiResponse<TenantConfigBundleVo>>)> {
    let outcome = state
        .services
        .tenant_config_transfer
        .request_package_export(&current_user, &idempotency_key_hash(&headers)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(outcome.bundle.into())),
    ))
}

#[get("/")]
#[perm("system:config-package:list")]
#[utoipa::path(get, path = "/api/v1/system/config-packages", tag = "租户配置迁移",
    params(TenantConfigPageQuery),
    responses(
        (status = 200, description = "配置包列表", body = ApiPageResponse<TenantConfigBundleVo>),
        (status = 400, description = "分页参数无效"),
        (status = 403, description = "没有配置包列表权限")
    ),
    security(("bearer" = [])))]
async fn list_packages(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<TenantConfigPageQuery>,
) -> HttpResult<Json<ApiPageResponse<TenantConfigBundleVo>>> {
    let page = state
        .services
        .tenant_config_transfer
        .list_bundles(&current_user, query.into_page(&state.config.pagination)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok(Json(page_response(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[get("/{id}")]
#[perm("system:config-package:list")]
#[utoipa::path(get, path = "/api/v1/system/config-packages/{id}", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置包 ID")),
    responses(
        (status = 200, description = "配置包详情", body = ApiResponse<TenantConfigBundleVo>),
        (status = 400, description = "配置包 ID 无效"),
        (status = 403, description = "没有配置包列表权限"),
        (status = 404, description = "配置包不存在或不属于当前租户")
    ),
    security(("bearer" = [])))]
async fn get_package(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<TenantConfigBundleVo>>> {
    state
        .services
        .tenant_config_transfer
        .get_bundle(&current_user, parse_positive_id(&id, "配置包")?)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(TenantConfigBundleVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[get("/{id}/download")]
#[perm("system:config-package:download")]
#[utoipa::path(get, path = "/api/v1/system/config-packages/{id}/download", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置包 ID")),
    responses(
        (status = 200, description = "配置包文件", body = Vec<u8>, content_type = "application/zip"),
        (status = 400, description = "配置包 ID 无效"),
        (status = 403, description = "没有配置包下载权限"),
        (status = 404, description = "配置包或文件不存在"),
        (status = 409, description = "配置包尚未生成或文件已经过期"),
        (status = 503, description = "对象存储不可用")
    ),
    security(("bearer" = [])))]
async fn download_package(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<axum::response::Response> {
    let file = state
        .services
        .tenant_config_transfer
        .download_bundle(&current_user, parse_positive_id(&id, "配置包")?)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    attachment_response(file.data, &file.original_name, CONFIG_PACKAGE_CONTENT_TYPE)
}

#[post("/upload")]
#[perm("system:config-transfer:add")]
#[utoipa::path(post, path = "/api/v1/system/config-transfers/upload", tag = "租户配置迁移",
    params(("Idempotency-Key" = String, Header, description = "配置包上传幂等键")),
    request_body(content = TenantConfigPackageUploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 202, description = "配置迁移已创建", body = ApiResponse<TenantConfigTransferVo>),
        (status = 400, description = "上传表单、幂等键或配置包无效"),
        (status = 403, description = "没有配置迁移创建权限"),
        (status = 409, description = "幂等冲突或当前配置状态冲突"),
        (status = 413, description = "配置包压缩或解压大小超过限制"),
        (status = 503, description = "数据库、对象存储或后台任务服务不可用")
    ),
    security(("bearer" = [])))]
async fn upload_transfer(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> HttpResult<(StatusCode, Json<ApiResponse<TenantConfigTransferVo>>)> {
    let idempotency_hash = idempotency_key_hash(&headers)?;
    let mut package = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Validation(format!("读取上传表单失败: {error}")))?
    {
        if field.name() != Some("file") {
            return Err(AppError::Validation("上传表单只允许 file 字段".into()).into());
        }
        if package.is_some() {
            return Err(AppError::Validation("只能上传一个配置包".into()).into());
        }
        let file_name = field
            .file_name()
            .map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| AppError::Validation("配置包文件名不能为空".into()))?;
        if file_name.chars().count() > 255 || file_name.contains(['/', '\\', '\0', '\r', '\n']) {
            return Err(AppError::Validation(
                "配置包文件名不能超过 255 个字符，且不能包含路径或控制字符".into(),
            )
            .into());
        }
        if !file_name
            .to_ascii_lowercase()
            .ends_with(".ryframe-config.zip")
        {
            return Err(
                AppError::Validation("配置包文件名必须以 .ryframe-config.zip 结尾".into()).into(),
            );
        }
        let max_package_bytes = state.config.tenant_config_transfer.max_package_bytes;
        let declared_length = field
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        if declared_length.is_some_and(|length| length > max_package_bytes) {
            return Err(AppError::PayloadTooLarge(format!(
                "配置包超过 {max_package_bytes} 字节上限"
            ))
            .into());
        }
        let mut data = Vec::with_capacity(declared_length.unwrap_or_default());
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| AppError::Validation(format!("读取配置包失败: {error}")))?
        {
            if chunk.len() > max_package_bytes.saturating_sub(data.len()) {
                return Err(AppError::PayloadTooLarge(format!(
                    "配置包超过 {max_package_bytes} 字节上限"
                ))
                .into());
            }
            data.extend_from_slice(&chunk);
        }
        if data.is_empty() {
            return Err(AppError::Validation("配置包不能为空".into()).into());
        }
        package = Some((file_name, data));
    }
    let (file_name, data) =
        package.ok_or_else(|| AppError::Validation("未找到 file 上传字段".into()))?;
    let outcome = state
        .services
        .tenant_config_transfer
        .upload_package_and_create_transfer(&current_user, file_name, data, &idempotency_hash)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(outcome.transfer.into())),
    ))
}

#[post("/from-package")]
#[perm("system:config-transfer:add")]
#[utoipa::path(post, path = "/api/v1/system/config-transfers/from-package", tag = "租户配置迁移",
    params(("Idempotency-Key" = String, Header, description = "配置迁移创建幂等键")),
    request_body = CreateTenantConfigTransferDto,
    responses(
        (status = 202, description = "配置迁移已创建", body = ApiResponse<TenantConfigTransferVo>),
        (status = 400, description = "配置包 ID 或幂等键无效"),
        (status = 403, description = "没有配置迁移创建权限"),
        (status = 404, description = "配置包不存在或不属于当前租户"),
        (status = 409, description = "配置包状态或幂等结果冲突"),
        (status = 503, description = "数据库或后台任务服务不可用")
    ),
    security(("bearer" = [])))]
async fn create_transfer_from_package(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<CreateTenantConfigTransferDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<TenantConfigTransferVo>>)> {
    let outcome = state
        .services
        .tenant_config_transfer
        .create_transfer_from_package(
            &current_user,
            parse_positive_id(&request.bundle_id, "配置包")?,
            &idempotency_key_hash(&headers)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(outcome.transfer.into())),
    ))
}

#[get("/")]
#[perm("system:config-transfer:list")]
#[utoipa::path(get, path = "/api/v1/system/config-transfers", tag = "租户配置迁移",
    params(TenantConfigPageQuery),
    responses(
        (status = 200, description = "配置迁移列表", body = ApiPageResponse<TenantConfigTransferVo>),
        (status = 400, description = "分页参数无效"),
        (status = 403, description = "没有配置迁移列表权限")
    ),
    security(("bearer" = [])))]
async fn list_transfers(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<TenantConfigPageQuery>,
) -> HttpResult<Json<ApiPageResponse<TenantConfigTransferVo>>> {
    let page = state
        .services
        .tenant_config_transfer
        .list_transfers(&current_user, query.into_page(&state.config.pagination)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok(Json(page_response(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[get("/{id}")]
#[perm("system:config-transfer:list")]
#[utoipa::path(get, path = "/api/v1/system/config-transfers/{id}", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置迁移 ID")),
    responses(
        (status = 200, description = "配置迁移详情", body = ApiResponse<TenantConfigTransferVo>),
        (status = 400, description = "配置迁移 ID 无效"),
        (status = 403, description = "没有配置迁移列表权限"),
        (status = 404, description = "配置迁移不存在或不属于当前租户")
    ),
    security(("bearer" = [])))]
async fn get_transfer(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<TenantConfigTransferVo>>> {
    state
        .services
        .tenant_config_transfer
        .get_transfer(&current_user, parse_positive_id(&id, "配置迁移")?)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(TenantConfigTransferVo::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[get("/{id}/items")]
#[perm("system:config-transfer:list")]
#[utoipa::path(get, path = "/api/v1/system/config-transfers/{id}/items", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置迁移 ID"), TenantConfigPageQuery),
    responses(
        (status = 200, description = "配置迁移明细", body = ApiPageResponse<TenantConfigTransferItemVo>),
        (status = 400, description = "配置迁移 ID 或分页参数无效"),
        (status = 403, description = "没有配置迁移列表权限"),
        (status = 404, description = "配置迁移不存在或不属于当前租户")
    ),
    security(("bearer" = [])))]
async fn list_transfer_items(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Query(query): Query<TenantConfigPageQuery>,
) -> HttpResult<Json<ApiPageResponse<TenantConfigTransferItemVo>>> {
    let page = state
        .services
        .tenant_config_transfer
        .list_transfer_items(
            &current_user,
            parse_positive_id(&id, "配置迁移")?,
            query.into_page(&state.config.pagination)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok(Json(page_response(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}

#[post("/{id}/preview")]
#[perm("system:config-transfer:preview")]
#[utoipa::path(post, path = "/api/v1/system/config-transfers/{id}/preview", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置迁移 ID"), ("Idempotency-Key" = String, Header, description = "配置预览幂等键")),
    request_body = EmptyRequestDto,
    responses(
        (status = 202, description = "配置预览任务已创建", body = ApiResponse<TenantConfigTransferVo>),
        (status = 400, description = "配置迁移 ID 或幂等键无效"),
        (status = 403, description = "没有配置迁移预览权限"),
        (status = 404, description = "配置迁移不存在或不属于当前租户"),
        (status = 409, description = "预览任务、配置版本或迁移状态冲突"),
        (status = 503, description = "数据库或后台任务服务不可用")
    ),
    security(("bearer" = [])))]
async fn request_preview(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(_request): Json<EmptyRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<TenantConfigTransferVo>>)> {
    let transfer = state
        .services
        .tenant_config_transfer
        .request_preview(
            &current_user,
            parse_positive_id(&id, "配置迁移")?,
            &idempotency_key_hash(&headers)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(transfer.into())),
    ))
}

#[post("/{id}/apply")]
#[perm("system:config-transfer:apply")]
#[utoipa::path(post, path = "/api/v1/system/config-transfers/{id}/apply", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置迁移 ID"), ("Idempotency-Key" = String, Header, description = "配置应用幂等键")),
    request_body = ApplyTenantConfigTransferDto,
    responses(
        (status = 202, description = "配置应用任务已创建", body = ApiResponse<TenantConfigTransferVo>),
        (status = 400, description = "请求参数、计划哈希或幂等键无效"),
        (status = 403, description = "没有配置迁移应用权限"),
        (status = 404, description = "配置迁移不存在或不属于当前租户"),
        (status = 409, description = "预览、目标版本、租约或迁移状态冲突"),
        (status = 503, description = "数据库、对象存储或后台任务服务不可用")
    ),
    security(("bearer" = [])))]
async fn request_apply(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ApplyTenantConfigTransferDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<TenantConfigTransferVo>>)> {
    let transfer = state
        .services
        .tenant_config_transfer
        .request_apply(
            &current_user,
            parse_positive_id(&id, "配置迁移")?,
            ApplyTenantConfigTransferCommand {
                plan_hash: request.plan_hash,
                target_configuration_version: request.target_configuration_version,
                target_authorization_epoch: parse_authorization_epoch(
                    &request.target_authorization_epoch,
                )?,
                idempotency_key_hash: idempotency_key_hash(&headers)?,
            },
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(transfer.into())),
    ))
}

#[post("/{id}/rollback")]
#[perm("system:config-transfer:rollback")]
#[utoipa::path(post, path = "/api/v1/system/config-transfers/{id}/rollback", tag = "租户配置迁移",
    params(("id" = String, Path, description = "配置迁移 ID"), ("Idempotency-Key" = String, Header, description = "配置回滚幂等键")),
    request_body = EmptyRequestDto,
    responses(
        (status = 202, description = "配置回滚任务已创建", body = ApiResponse<TenantConfigTransferVo>),
        (status = 400, description = "配置迁移 ID 或幂等键无效"),
        (status = 403, description = "没有配置迁移回滚权限"),
        (status = 404, description = "配置迁移或回滚快照不存在"),
        (status = 409, description = "回滚窗口、引用、版本、租约或迁移状态冲突"),
        (status = 503, description = "数据库、对象存储或后台任务服务不可用")
    ),
    security(("bearer" = [])))]
async fn request_rollback(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(_request): Json<EmptyRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<TenantConfigTransferVo>>)> {
    let transfer = state
        .services
        .tenant_config_transfer
        .request_rollback(
            &current_user,
            parse_positive_id(&id, "配置迁移")?,
            &idempotency_key_hash(&headers)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(transfer.into())),
    ))
}

fn parse_positive_id(value: &str, label: &str) -> HttpResult<i64> {
    Ok(value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation(format!("{label} ID 必须是正整数")))?)
}

fn parse_authorization_epoch(value: &str) -> HttpResult<i32> {
    Ok(value
        .parse::<i32>()
        .ok()
        .filter(|epoch| *epoch >= 0)
        .ok_or_else(|| AppError::Validation("target_authorization_epoch 必须是非负整数".into()))?)
}

fn page_response<T: serde::Serialize>(
    items: Vec<T>,
    total: u64,
    page: u64,
    page_size: u64,
    max_page_size: u64,
) -> ApiPageResponse<T> {
    ApiPageResponse::page(items, total, page, page_size, max_page_size)
}
