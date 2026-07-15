use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Multipart, Path, Query, State},
};
use ryframe_auth::rbac;
use ryframe_common::{ApiPageResponse, ApiResponse, AppError, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{CreateUserParams, UpdateUserParams, UserDetailVo, UserVo};
use serde::Deserialize;
use validator::Validate;

use super::auth_handler::AppState;
use std::net::SocketAddr;

use crate::dto::{
    user_dto::{
        ChangeStatusDto, CreateUserDto, PasswordResetRequestDto, PasswordResetRequestResponse,
        UpdateUserDto, UserRoleAssignDto,
    },
    user_import_dto::{UserExportData, UserImportData},
};
use crate::extractors::CurrentUser;
use crate::handler_utils::{
    excel_response, parse_csv_i64, parse_i64_strings, parse_optional_i64, parse_optional_i64_str,
};
use crate::runtime::UserImportCompletedEvent;

async fn ensure_target_user_access(
    state: &AppState,
    current_user: &CurrentUser,
    target_user_id: i64,
) -> AppResult<ryframe_db::entities::user::Model> {
    let scope_ctx = current_user.to_data_scope_context();
    state
        .user_service
        .ensure_user_accessible(&state.db, target_user_id, &scope_ctx)
        .await
}

fn ensure_not_self_operation(
    current_user: &CurrentUser,
    target_user_id: i64,
    action: &str,
) -> AppResult<()> {
    if current_user.user_id == target_user_id {
        return Err(AppError::Authorization(format!("禁止{}自己", action)));
    }
    Ok(())
}

fn ensure_current_user_permission(
    current_user: &CurrentUser,
    permission: &str,
    message: &str,
) -> AppResult<()> {
    if current_user.is_super_admin || rbac::has_permission(&current_user.permissions, permission) {
        return Ok(());
    }
    Err(AppError::Authorization(message.into()))
}

async fn ensure_not_super_admin_target(state: &AppState, target_user_id: i64) -> AppResult<()> {
    if state
        .user_service
        .is_super_admin_user(&state.db, target_user_id)
        .await?
    {
        return Err(AppError::Authorization("禁止操作超级管理员".into()));
    }
    Ok(())
}

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
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .merge(route!(batch_remove))
        .merge(route!(request_password_reset))
        .merge(route!(change_status))
        .merge(route!(export_users))
        .merge(route!(import_users))
        .merge(route!(download_import_template))
        .with_state(state)
}

pub fn user_assignment_router(state: AppState) -> Router {
    Router::new().merge(route!(assign_role)).with_state(state)
}

/// 用户列表分页查询
#[get("/", "/list")]
#[perm("system:user:list")]
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
            .find_by_page_filtered_with_data_scope(
                &state.db,
                page_query,
                query.username.as_deref(),
                query.phone.as_deref(),
                query.status.as_deref(),
                query.dept_id,
                &scope_ctx,
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
#[get("/listNoPage")]
#[perm("system:user:list")]
#[utoipa::path(get, path = "/api/v1/system/users/listNoPage", tag = "用户管理",
    responses((status = 200, description = "用户列表")),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
) -> AppResult<Json<ApiResponse<Vec<UserVo>>>> {
    let page_query = PageQuery::all_records();
    let scope_ctx = current_user.to_data_scope_context();
    state
        .user_service
        .find_by_page_with_data_scope(&state.db, page_query, &scope_ctx)
        .await
        .map(|p| Json(ApiResponse::success(p.records)))
}

/// 用户详情
#[get("/{id}")]
#[perm("system:user:list")]
#[utoipa::path(get, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    responses((status = 200, description = "用户详情")),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<UserDetailVo>>> {
    let scope_ctx = current_user.to_data_scope_context();
    match state
        .user_service
        .find_by_id_with_roles_with_data_scope(&state.db, id, &scope_ctx)
        .await?
    {
        Some(user) => Ok(Json(ApiResponse::success(user))),
        None => Err(ryframe_common::AppError::NotFound("用户不存在".into())),
    }
}

/// 创建用户
#[post("/")]
#[perm("system:user:add")]
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
    state
        .user_service
        .create(
            &state.db,
            CreateUserParams {
                username: &dto.username,
                nickname: &dto.nickname,
                email: dto.email.as_deref().unwrap_or(""),
                phone: dto.phone.as_deref().unwrap_or(""),
                dept_id,
                role_ids: None,
            },
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新用户
#[put("/{id}")]
#[perm("system:user:edit")]
#[utoipa::path(put, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = UpdateUserDto,
    responses((status = 200, description = "更新成功")),
    security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateUserDto>,
) -> AppResult<Json<ApiResponse<UserVo>>> {
    dto.validate()?;
    ensure_target_user_access(&state, &current_user, id).await?;
    if id == current_user.user_id && dto.status != ryframe_db::entities::user::Model::STATUS_NORMAL
    {
        return Err(AppError::Authorization("禁止停用自己".into()));
    }
    ensure_not_super_admin_target(&state, id).await?;
    // 解析前端传来的 String ID 为 i64
    let dept_id = parse_optional_i64(dto.dept_id);
    let result = state
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
                role_ids: None,
            },
        )
        .await?;
    super::auth_handler::invalidate_user_tokens(&state, id).await?;
    Ok(Json(ApiResponse::success(result)))
}

/// 给用户分配角色
#[post("/assign-role")]
#[perm("system:user:edit")]
#[utoipa::path(post, path = "/api/v1/system/user/assign-role", tag = "用户管理",
    request_body = UserRoleAssignDto,
    responses((status = 200, description = "角色分配成功")),
    security(("bearer" = [])))]
async fn assign_role(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Json(dto): Json<UserRoleAssignDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    dto.validate()?;
    let user_id = dto
        .user_id
        .parse::<i64>()
        .map_err(|_| AppError::Validation("无效的用户ID".into()))?;
    let role_ids = parse_i64_strings(&dto.role_ids);
    ensure_target_user_access(&state, &current_user, user_id).await?;
    if user_id == current_user.user_id {
        ensure_current_user_permission(
            &current_user,
            "sys:user:editSelf",
            "不允许修改自身角色权限",
        )?;
    }

    let super_role = state.role_service.get_super_role(&state.db).await?;
    if role_ids.contains(&super_role.id) {
        ensure_current_user_permission(
            &current_user,
            "sys:role:editSuper",
            "无权限分配超级管理员角色",
        )?;
    }
    state
        .user_service
        .assign_roles(&state.db, user_id, role_ids)
        .await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("角色分配成功")))
}

/// 删除用户
#[delete("/{id}")]
#[perm("system:user:remove")]
#[utoipa::path(delete, path = "/api/v1/system/users/{id}", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    responses((status = 200, description = "删除成功")),
    security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    ensure_target_user_access(&state, &current_user, id).await?;
    ensure_not_self_operation(&current_user, id, "删除")?;
    ensure_not_super_admin_target(&state, id).await?;
    state.user_service.delete(&state.db, id).await?;
    super::auth_handler::invalidate_user_tokens(&state, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 批量删除用户
#[delete("/batch/{ids}")]
#[perm("system:user:remove")]
#[utoipa::path(delete, path = "/api/v1/system/users/batch/{ids}", tag = "用户管理",
    params(("ids" = String, Path, description = "用户ID列表，逗号分隔")),
    responses((status = 200, description = "批量删除成功")),
    security(("bearer" = [])))]
async fn batch_remove(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Path(ids_str): Path<String>,
) -> AppResult<Json<ApiResponse<()>>> {
    let ids = parse_csv_i64(&ids_str);

    if ids.is_empty() {
        return Err(ryframe_common::AppError::Validation(
            "请选择要删除的用户".into(),
        ));
    }

    for id in &ids {
        ensure_target_user_access(&state, &current_user, *id).await?;
        ensure_not_self_operation(&current_user, *id, "删除")?;
        ensure_not_super_admin_target(&state, *id).await?;
    }

    let count = state.user_service.delete_many(&state.db, &ids).await?;
    super::auth_handler::invalidate_users_tokens(&state, &ids).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "成功删除 {} 个用户",
        count
    ))))
}

/// 修改用户状态
#[put("/changeStatus")]
#[perm("system:user:edit")]
#[utoipa::path(put, path = "/api/v1/system/users/changeStatus", tag = "用户管理",
    request_body = ChangeStatusDto,
    responses((status = 200, description = "状态修改成功")),
    security(("bearer" = [])))]
async fn change_status(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Json(dto): Json<ChangeStatusDto>,
) -> AppResult<Json<ApiResponse<()>>> {
    let user_id: i64 = dto
        .user_id
        .parse()
        .map_err(|_| ryframe_common::AppError::Validation("无效的用户ID".into()))?;
    ensure_target_user_access(&state, &current_user, user_id).await?;
    if user_id == current_user.user_id
        && dto.status != ryframe_db::entities::user::Model::STATUS_NORMAL
    {
        return Err(AppError::Authorization("禁止停用自己".into()));
    }
    ensure_not_super_admin_target(&state, user_id).await?;
    state
        .user_service
        .change_status(&state.db, user_id, dto.status)
        .await?;
    super::auth_handler::invalidate_user_tokens(&state, user_id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("状态修改成功")))
}

/// 发起用户密码重置请求
#[post("/{id}/password-reset-requests")]
#[perm("system:user:edit")]
#[utoipa::path(post, path = "/api/v1/system/users/{id}/password-reset-requests", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = PasswordResetRequestDto,
    responses((status = 200, description = "密码重置请求已发起")),
    security(("bearer" = [])))]
async fn request_password_reset(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    Path(id): Path<i64>,
    Json(dto): Json<PasswordResetRequestDto>,
) -> AppResult<Json<ApiResponse<PasswordResetRequestResponse>>> {
    dto.validate()?;
    ensure_target_user_access(&state, &current_user, id).await?;
    ensure_not_super_admin_target(&state, id).await?;
    let outcome = state
        .user_service
        .request_password_reset(
            &state.db,
            id,
            current_user.user_id,
            &dto.reason,
            Some(remote_addr.ip().to_string()),
        )
        .await?;
    let response = PasswordResetRequestResponse {
        request_id: outcome.request.id.to_string(),
        reset_token: outcome.token.clone(),
        reset_url: format!(
            "/reset-password?requestId={}&token={}",
            outcome.request.id, outcome.token
        ),
        expires_at: outcome.request.expires_at.to_rfc3339(),
    };
    Ok(Json(ApiResponse::success_msg(
        "password reset request created",
        response,
    )))
}

/// 导出用户数据为 Excel
#[get("/export")]
#[perm("system:user:export")]
async fn export_users(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    Query(query): Query<UserListQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    // 查询所有用户（不分页）- 需要通过分页查询获取全部
    let page_query = PageQuery::all_records();
    let scope_ctx = current_user.to_data_scope_context();
    let page_result = state
        .user_service
        .find_by_page_filtered_with_data_scope(
            &state.db,
            page_query,
            query.username.as_deref(),
            query.phone.as_deref(),
            query.status.as_deref(),
            query.dept_id,
            &scope_ctx,
        )
        .await?;

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
#[post("/import")]
#[perm("system:user:add")]
async fn import_users(
    State(state): State<AppState>,
    Extension(current_user): Extension<CurrentUser>,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    use ryframe_common::utils::ExcelImporter;
    use std::time::Duration;
    use validator::Validate;

    if !state
        .runtime
        .feature_flags
        .is_enabled_or("user_import", true)
    {
        return Err(AppError::Authorization("用户导入功能已关闭".into()));
    }

    let lock_key = format!("tenant:{}:system:user:import", current_user.tenant_id);
    let _guard = state
        .runtime
        .distributed_lock
        .try_acquire(&lock_key, Duration::from_secs(300))
        .await?
        .ok_or_else(|| AppError::Conflict("当前租户正在执行用户导入，请稍后再试".into()))?;

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
                            nickname: &data.nickname,
                            email: &data.email,
                            phone: data.phone.as_deref().unwrap_or(""),
                            dept_id: parse_optional_i64_str(data.dept_id.as_deref()),
                            role_ids: None,
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

            state
                .runtime
                .emit_user_import_completed(UserImportCompletedEvent {
                    tenant_id: current_user.tenant_id,
                    operator: current_user.username,
                    success_count,
                    fail_count,
                    occurred_at: chrono::Utc::now().to_rfc3339(),
                })
                .await;

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
#[get("/import-template")]
#[perm("system:user:add")]
async fn download_import_template(
    State(_state): State<AppState>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let bytes = ExcelExporter::export_template("用户数据", UserImportData::excel_headers())?;

    excel_response(bytes, "user_template.xlsx")
}
