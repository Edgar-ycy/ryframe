use axum::{
    Extension, Json, Router,
    extract::{Multipart, Path, Query, State},
    routing::{delete, get, post, put},
};
use ryframe_auth::middleware::perm_route;
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_service::system::{CreateUserParams, UpdateUserParams, UserDetailVo, UserVo};
use serde::Deserialize;
use validator::Validate;

use super::auth_handler::AppState;
use crate::dto::{
    user_dto::{ChangeStatusDto, CreateUserDto, ResetPasswordDto, UpdateUserDto},
    user_import_dto::{UserExportData, UserImportData},
};
use crate::extractors::CurrentUser;
use crate::handler_utils::{
    excel_response, parse_csv_i64, parse_i64_strings, parse_optional_i64, parse_optional_i64_str,
};

/// 用户列表分页查询参数（支持搜索过滤）
#[derive(Debug, Deserialize)]
pub struct UserListQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(
        default = "ryframe_core::repository::default_page_size",
        alias = "pageSize"
    )]
    pub page_size: u64,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub status: Option<String>,
    pub dept_id: Option<i64>,
}

pub fn user_router(state: AppState) -> Router {
    Router::new()
        .route("/", perm_route(get(list), "system:user:list"))
        .route("/", perm_route(post(create), "system:user:add"))
        .route("/list", perm_route(get(list), "system:user:list"))
        .route(
            "/listNoPage",
            perm_route(get(list_no_page), "system:user:list"),
        )
        .route("/{id}", perm_route(get(detail), "system:user:list"))
        .route("/{id}", perm_route(put(update), "system:user:edit"))
        .route("/{id}", perm_route(delete(remove), "system:user:remove"))
        .route(
            "/batch/{ids}",
            perm_route(delete(batch_remove), "system:user:remove"),
        )
        .route(
            "/{id}/password",
            perm_route(put(reset_password), "system:user:edit"),
        )
        .route(
            "/changeStatus",
            perm_route(put(change_status), "system:user:edit"),
        )
        .route(
            "/export",
            perm_route(get(export_users), "system:user:export"),
        )
        .route("/import", perm_route(post(import_users), "system:user:add"))
        .route(
            "/import-template",
            perm_route(get(download_import_template), "system:user:add"),
        )
        .with_state(state)
}

/// 用户列表分页查询
#[utoipa::path(get, path = "/api/v1/system/users", tag = "用户管理",
    responses((status = 200, description = "用户列表")),
    security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Query(query): Query<UserListQuery>,
) -> AppResult<Json<ApiPageResponse<UserVo>>> {
    let page_query = PageQuery {
        page: query.page,
        page_size: query.page_size,
    };

    let scope_ctx = current_user.to_data_scope_context();
    let has_filter = query.username.is_some()
        || query.phone.is_some()
        || query.status.is_some()
        || query.dept_id.is_some();

    if has_filter {
        state
            .user_service
            .find_by_page_filtered(
                &state.db,
                page_query,
                query.username.as_deref(),
                query.phone.as_deref(),
                query.status.as_deref(),
                query.dept_id,
            )
            .await
            .map(|p| Json(p.to_page_response("查询成功")))
    } else {
        state
            .user_service
            .find_by_page_with_data_scope(&state.db, page_query, &scope_ctx)
            .await
            .map(|p| Json(p.to_page_response("查询成功")))
    }
}

/// 用户列表不分页查询（返回全部数据）
#[utoipa::path(get, path = "/api/v1/system/users/listNoPage", tag = "用户管理",
    responses((status = 200, description = "用户列表")),
    security(("bearer" = [])))]
async fn list_no_page(State(state): State<AppState>) -> AppResult<Json<ApiResponse<Vec<UserVo>>>> {
    let page_query = PageQuery::all_records();
    state
        .user_service
        .find_by_page(&state.db, page_query)
        .await
        .map(|p| Json(ApiResponse::success(p.records)))
}

/// 用户详情
#[utoipa::path(get, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    responses((status = 200, description = "用户详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<UserDetailVo>>> {
    match state
        .user_service
        .find_by_id_with_roles(&state.db, id)
        .await?
    {
        Some(user) => Ok(Json(ApiResponse::success(user))),
        None => Err(ryframe_common::AppError::NotFound("用户不存在".into())),
    }
}

/// 创建用户
#[utoipa::path(post, path = "/api/v1/system/users", tag = "用户管理",
    request_body = CreateUserDto,
    responses((status = 200, description = "创建成功")),
    security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateUserDto>,
) -> AppResult<Json<ApiResponse<UserVo>>> {
    dto.validate()?;
    // 解析前端传来的 String ID 为 i64
    let dept_id = parse_optional_i64(dto.dept_id);
    let role_ids = dto.role_ids.map(|ids| parse_i64_strings(&ids));
    state
        .user_service
        .create(
            &state.db,
            CreateUserParams {
                username: &dto.username,
                password: &dto.password,
                nickname: &dto.nickname,
                email: dto.email.as_deref().unwrap_or(""),
                phone: dto.phone.as_deref().unwrap_or(""),
                dept_id,
                role_ids,
                enable_pwd_complexity: state.config.auth.enable_password_complexity,
            },
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新用户
#[utoipa::path(put, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = UpdateUserDto,
    responses((status = 200, description = "更新成功")),
    security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateUserDto>,
) -> AppResult<Json<ApiResponse<UserVo>>> {
    dto.validate()?;
    // 解析前端传来的 String ID 为 i64
    let dept_id = parse_optional_i64(dto.dept_id);
    let role_ids = dto.role_ids.map(|ids| parse_i64_strings(&ids));
    state
        .user_service
        .update(
            &state.db,
            UpdateUserParams {
                id,
                nickname: &dto.nickname,
                email: dto.email.as_deref().unwrap_or(""),
                phone: dto.phone.as_deref().unwrap_or(""),
                dept_id,
                status: dto.status,
                role_ids,
            },
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除用户
#[utoipa::path(delete, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.user_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 批量删除用户
#[utoipa::path(delete, path = "/api/v1/system/users/batch/{ids}", tag = "用户管理",
    params(("ids" = String, Path, description = "用户ID列表，逗号分隔")),
    responses((status = 200, description = "批量删除成功")),
    security(("bearer" = [])))]
async fn batch_remove(
    State(state): State<AppState>,
    Path(ids_str): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    let ids = parse_csv_i64(&ids_str);

    if ids.is_empty() {
        return Err(ryframe_common::AppError::Validation(
            "请选择要删除的用户".into(),
        ));
    }

    let count = state.user_service.delete_many(&state.db, &ids).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功删除 {} 个用户",
        count
    ))))
}

/// 修改用户状态
#[utoipa::path(put, path = "/api/v1/system/users/changeStatus", tag = "用户管理",
    request_body = ChangeStatusDto,
    responses((status = 200, description = "状态修改成功")),
    security(("bearer" = [])))]
async fn change_status(
    State(state): State<AppState>,
    Json(dto): Json<ChangeStatusDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    let user_id: i64 = dto
        .user_id
        .parse()
        .map_err(|_| ryframe_common::AppError::Validation("无效的用户ID".into()))?;
    state
        .user_service
        .change_status(&state.db, user_id, dto.status)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("状态修改成功")))
}

/// 重置用户密码
#[utoipa::path(put, path = "/api/v1/system/users/{id}/password", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = ResetPasswordDto,
    responses((status = 200, description = "密码重置成功")),
    security(("bearer" = [])))]
async fn reset_password(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<ResetPasswordDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    state
        .user_service
        .reset_password(
            &state.db,
            id,
            &dto.password,
            state.config.auth.enable_password_complexity,
        )
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("密码重置成功")))
}

/// 导出用户数据为 Excel
async fn export_users(
    State(state): State<AppState>,
    Query(_query): Query<PageQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    // 查询所有用户（不分页）- 需要通过分页查询获取全部
    let query = PageQuery::all_records();
    let page_result = state.user_service.find_by_page(&state.db, query).await?;

    // 转换为导出数据
    let export_data: Vec<UserExportData> = page_result
        .records
        .into_iter()
        .map(|u| UserExportData {
            user_id: u.id,
            username: u.username,
            nickname: u.nickname,
            email: u.email,
            phone: u.phone,
            sex: "0".to_string(), // user 表没有 sex 字段，使用默认值
            dept_name: u.dept_name,
            status: u.status,
            remark: u.remark,
            created_at: u.created_at.to_rfc3339(),
        })
        .collect();

    // 生成 Excel
    let bytes =
        ExcelExporter::export_to_bytes(&export_data, "用户数据", UserExportData::excel_headers())?;

    // 返回文件
    excel_response(bytes, "users.xlsx")
}

/// 从 Excel 导入用户数据
async fn import_users(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    use ryframe_common::utils::ExcelImporter;
    use validator::Validate;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ryframe_common::AppError::Internal(format!("读取 multipart 失败: {}", e)))?
    {
        if field.name() == Some("file") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| ryframe_common::AppError::Internal(format!("读取文件失败: {}", e)))?;

            // 解析 Excel
            let import_data: Vec<UserImportData> = ExcelImporter::read_from_bytes(&bytes, None)?;

            // 验证并导入
            let mut success_count = 0;
            let mut fail_count = 0;
            let mut errors = Vec::new();

            for (index, data) in import_data.iter().enumerate() {
                // 验证数据
                if let Err(e) = data.validate() {
                    fail_count += 1;
                    errors.push(format!("第 {} 行数据验证失败: {}", index + 2, e));
                    continue;
                }

                // 创建用户
                match state
                    .user_service
                    .create(
                        &state.db,
                        CreateUserParams {
                            username: &data.username,
                            password: "123456", // 默认密码
                            nickname: &data.nickname,
                            email: &data.email,
                            phone: data.phone.as_deref().unwrap_or(""),
                            dept_id: parse_optional_i64_str(data.dept_id.as_deref()),
                            role_ids: None,
                            enable_pwd_complexity: false, // 导入时不强制密码复杂度（使用默认密码）
                        },
                    )
                    .await
                {
                    Ok(_) => success_count += 1,
                    Err(e) => {
                        fail_count += 1;
                        errors.push(format!("第 {} 行导入失败: {}", index + 2, e));
                    }
                }
            }

            return Ok(Json(ApiResponse::success_msg(
                "导入完成",
                serde_json::json!({
                    "success_count": success_count,
                    "fail_count": fail_count,
                    "errors": errors
                }),
            )));
        }
    }

    Err(ryframe_common::AppError::Validation(
        "未找到上传的文件".into(),
    ))
}

/// 下载导入模板
async fn download_import_template() -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let bytes = ExcelExporter::export_template("用户数据", UserImportData::excel_headers())?;

    excel_response(bytes, "user_template.xlsx")
}
