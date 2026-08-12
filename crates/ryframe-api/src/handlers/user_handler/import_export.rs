use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post};
use ryframe_service::system::UserExportFilters;

use crate::{
    dto::{public_dto::ExportJobVo, user_dto::UserExportRequestDto},
    handler_utils::excel_response,
    handlers::export_handler::request_export,
    state::AppState,
};

/// 创建用户异步导出任务，实际文件由 Worker 生成并保存到对象存储。
#[post("/exports")]
#[perm("system:user:export")]
#[utoipa::path(post, path = "/api/v1/system/users/exports", tag = "用户管理",
    params(("Idempotency-Key" = String, Header, description = "幂等键")),
    request_body = UserExportRequestDto,
    responses((status = 202, description = "用户导出任务已创建", body = ApiResponse<ExportJobVo>)),
    security(("bearer" = [])))]
pub(crate) async fn request_user_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<UserExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    let dept_id = request
        .dept_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| {
            id.parse::<i64>()
                .map_err(|_| AppError::Validation(format!("无效的部门ID: {id}")))
        })
        .transpose()?;
    request_export(
        state,
        current_user,
        headers,
        "users",
        "system:user:export",
        serde_json::to_value(UserExportFilters {
            username: request.username,
            phone: request.phone,
            status: request.status,
            dept_id,
        })
        .map_err(|error| AppError::Internal(format!("用户导出筛选条件序列化失败: {error}")))?,
    )
    .await
}

#[get("/import-template")]
#[perm("system:user-import:add")]
#[utoipa::path(get, path = "/api/v1/system/users/import-template", tag = "用户管理",
    responses((status = 200, description = "下载用户导入模板", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
pub(crate) async fn download_import_template(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<axum::response::Response> {
    let bytes = state
        .services
        .user_import
        .build_template(&current_user)
        .await
        .map_err(ryframe_http::HttpAppError::from)?;
    excel_response(bytes, "user_import_template.xlsx")
}
