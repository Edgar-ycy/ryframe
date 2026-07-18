use std::time::Duration;

use axum::{
    Json,
    extract::{Multipart, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{get, post};
use ryframe_service::system::CreateUserParams;
use validator::Validate;

use super::UserFilterQuery;
use crate::{
    dto::{
        multipart_dto::FileUploadForm,
        user_import_dto::{UserExportData, UserImportData, UserImportResult},
    },
    handler_utils::{excel_response, parse_optional_i64_str},
    state::AppState,
};

#[get("/export")]
#[perm("system:user:export")]
#[utoipa::path(get, path = "/api/v1/system/users/export", tag = "用户管理",
    params(UserFilterQuery),
    responses((status = 200, description = "导出用户 Excel", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
pub(crate) async fn export_users(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<UserFilterQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let params = query.into_service_params(PageQuery::all_records())?;
    let page = state
        .services
        .user
        .find_by_page(&current_user, params)
        .await?;
    let users = page
        .records
        .into_iter()
        .map(|user| UserExportData {
            user_id: user.id,
            username: user.username,
            nickname: user.nickname,
            email: user.email,
            phone: user.phone,
            dept_name: user.dept_name,
            status: user.status,
            remark: user.remark,
            created_at: user.created_at.to_rfc3339(),
        })
        .collect::<Vec<_>>();
    let bytes =
        ExcelExporter::export_to_bytes(&users, "用户数据", UserExportData::excel_headers())?;
    excel_response(bytes, "users.xlsx")
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
) -> AppResult<Json<ApiResponse<UserImportResult>>> {
    use ryframe_common::utils::ExcelImporter;

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

        return Ok(Json(ApiResponse::success_msg(
            "导入完成",
            UserImportResult {
                success_count,
                fail_count: errors.len(),
                errors,
            },
        )));
    }
    Err(AppError::Validation("未找到上传的文件".into()))
}

#[get("/import-template")]
#[perm("system:user:add")]
#[utoipa::path(get, path = "/api/v1/system/users/import-template", tag = "用户管理",
    responses((status = 200, description = "下载用户导入模板", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
pub(crate) async fn download_import_template(
    State(_state): State<AppState>,
    _current_user: RequestPrincipal,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let bytes = ExcelExporter::export_template("用户数据", UserImportData::excel_headers())?;
    excel_response(bytes, "user_template.xlsx")
}
