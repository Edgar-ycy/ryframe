use std::time::Duration;

use axum::{
    Json,
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post};
use ryframe_service::system::{CreateUserParams, UserExportFilters};
use validator::Validate;

use crate::{
    dto::{
        multipart_dto::FileUploadForm,
        public_dto::ExportJobVo,
        user_dto::UserExportRequestDto,
        user_import_dto::{UserImportData, UserImportResult},
    },
    handler_utils::{excel_response, parse_optional_i64_str},
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

#[post("/import")]
#[perm("system:user:add")]
#[utoipa::path(post, path = "/api/v1/system/users/import", tag = "用户管理",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "导入用户", body = ApiResponse<UserImportResult>)), security(("bearer" = [])))]
pub(crate) async fn import_users(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    mut multipart: Multipart,
) -> HttpResult<Json<ApiResponse<UserImportResult>>> {
    use ryframe_excel::ExcelImporter;

    let lock_key = format!("tenant:{}:system:user:import", current_user.tenant_id);
    let _guard = state
        .runtime
        .distributed_lock
        .try_acquire(&lock_key, Duration::from_secs(300))
        .await
        .map_err(|error| {
            if matches!(error, AppError::ServiceUnavailable(_)) {
                ryframe_middleware::metrics::record_redis_degraded("distributed_lock");
            }
            error
        })?
        .ok_or_else(|| AppError::Conflict("当前租户正在执行用户导入，请稍后再试".into()))?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Internal(format!("读取 multipart 失败: {error}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|error| AppError::Internal(format!("读取文件失败: {error}")))?;
        let users = ExcelImporter::read_from_bytes::<UserImportData>(&bytes, None)?;
        let mut success_count = 0;
        let mut errors = Vec::new();

        for (index, user) in users.iter().enumerate() {
            if let Err(error) = user.validate() {
                errors.push(format!("第 {} 行数据验证失败: {error}", index + 2));
                continue;
            }
            let dept_id = match parse_optional_i64_str(user.dept_id.as_deref()) {
                Ok(dept_id) => dept_id,
                Err(error) => {
                    errors.push(format!("第 {} 行导入失败: {error}", index + 2));
                    continue;
                }
            };
            match state
                .services
                .user
                .create(
                    &current_user,
                    CreateUserParams {
                        username: &user.username,
                        nickname: &user.nickname,
                        email: &user.email,
                        phone: user.phone.as_deref().unwrap_or(""),
                        dept_id,
                        role_ids: Vec::new(),
                    },
                )
                .await
            {
                Ok(_) => success_count += 1,
                Err(error) => {
                    errors.push(format!("第 {} 行导入失败: {error}", index + 2));
                }
            }
        }

        return Ok(Json(ApiResponse::success(UserImportResult {
            success_count,
            fail_count: errors.len(),
            errors,
        })));
    }
    Err(AppError::Validation("未找到上传的文件".into()).into())
}

#[get("/import-template")]
#[perm("system:user:add")]
#[utoipa::path(get, path = "/api/v1/system/users/import-template", tag = "用户管理",
    responses((status = 200, description = "下载用户导入模板", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
pub(crate) async fn download_import_template(
    State(_state): State<AppState>,
    _current_user: RequestPrincipal,
) -> HttpResult<axum::response::Response> {
    use ryframe_excel::ExcelExporter;

    let bytes = ExcelExporter::export_template("用户数据", UserImportData::excel_headers())?;
    excel_response(bytes, "user_template.xlsx")
}
