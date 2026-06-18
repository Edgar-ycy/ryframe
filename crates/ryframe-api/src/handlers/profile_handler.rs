use axum::{
    Json, Router,
    extract::{Multipart, State},
    routing::{get, put},
};
use ryframe_auth::jwt::Claims;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_service::system::{file_service::FileService, profile_service::UserProfileResponse};
use validator::Validate;

use crate::{
    dto::profile_dto::{ChangePasswordRequest, UpdateProfileRequest},
    handlers::auth_handler::AppState,
};

/// 个人中心路由
///
/// 不内嵌 `.with_state()`，由父路由统一注入 AppState。
/// 认证和操作日志中间件在父路由 (auth_router) 中统一注册。
pub fn profile_router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_profile).put(update_profile))
        .route("/password", put(change_password))
        .route("/avatar", put(update_avatar))
}

/// 获取个人信息
/// 获取个人信息
#[utoipa::path(get, path = "/api/v1/auth/profile", tag = "个人中心",
    responses((status = 200, description = "个人信息")), security(("bearer" = [])))]
pub async fn get_profile(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
) -> AppResult<Json<ApiResponse<UserProfileResponse>>> {
    let user_id = claims
        .sub
        .parse::<i64>()
        .map_err(|_| AppError::Authentication("令牌无效".into()))?;

    let profile = state
        .profile_service
        .get_profile(&state.db, user_id)
        .await?;

    Ok(Json(ApiResponse::success(profile)))
}

/// 更新个人信息
/// 更新个人信息
#[utoipa::path(put, path = "/api/v1/auth/profile", tag = "个人中心",
    request_body = UpdateProfileRequest, responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
pub async fn update_profile(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
    Json(req): Json<UpdateProfileRequest>,
) -> AppResult<Json<ApiResponse<()>>> {
    req.validate()?;
    let user_id = claims
        .sub
        .parse::<i64>()
        .map_err(|_| AppError::Authentication("令牌无效".into()))?;

    state
        .profile_service
        .update_profile(
            &state.db,
            user_id,
            req.nickname,
            req.email.unwrap_or_default(),
            req.phone.unwrap_or_default(),
        )
        .await?;

    Ok(Json(ApiResponse::success_no_data_with_msg(
        "个人信息更新成功",
    )))
}

/// 修改密码
/// 修改密码
#[utoipa::path(put, path = "/api/v1/auth/profile/password", tag = "个人中心",
    request_body = ChangePasswordRequest, responses((status = 200, description = "修改成功")), security(("bearer" = [])))]
pub async fn change_password(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
    Json(req): Json<ChangePasswordRequest>,
) -> AppResult<Json<ApiResponse<()>>> {
    req.validate_passwords().map_err(AppError::Validation)?;

    let user_id = claims
        .sub
        .parse::<i64>()
        .map_err(|_| AppError::Authentication("令牌无效".into()))?;

    state
        .profile_service
        .change_password(&state.db, user_id, &req.old_password, &req.new_password)
        .await?;

    Ok(Json(ApiResponse::success_no_data_with_msg("密码修改成功")))
}

/// 更新头像（直接接受文件上传）
///
/// 请求格式: multipart/form-data，字段名 `file`。
/// 上传后自动写入 sys_file 元数据表并更新 sys_user.avatar。
#[utoipa::path(put, path = "/api/v1/auth/profile/avatar", tag = "个人中心",
    responses((status = 200, description = "头像更新成功")),
    security(("bearer" = [])))]
pub async fn update_avatar(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    let user_id = claims
        .sub
        .parse::<i64>()
        .map_err(|_| AppError::Authentication("令牌无效".into()))?;

    let mut avatar_url = String::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("读取上传数据失败: {}", e)))?
    {
        let filename = match field.file_name() {
            Some(name) => name.to_string(),
            None => continue,
        };

        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::Internal(format!("读取文件数据失败: {}", e)))?;

        // 委托 FileService 处理上传逻辑
        avatar_url = FileService::upload_avatar(
            &state.db,
            &state.object_storage,
            filename,
            data.to_vec(),
            Some(user_id.to_string()),
        )
        .await?;
        break;
    }

    if avatar_url.is_empty() {
        return Err(AppError::Validation("未找到上传的头像文件".into()));
    }

    // 更新 sys_user.avatar
    tracing::info!(
        "[update_avatar] 准备更新用户头像: user_id={}, avatar_url={}",
        user_id,
        avatar_url
    );
    state
        .profile_service
        .update_avatar(&state.db, user_id, avatar_url.clone())
        .await?;
    tracing::info!("[update_avatar] 用户头像更新成功: user_id={}", user_id);

    Ok(Json(ApiResponse::success(serde_json::json!({
        "avatar_url": avatar_url
    }))))
}
