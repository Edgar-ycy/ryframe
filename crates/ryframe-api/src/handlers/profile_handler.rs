use axum::{
    Json, Router,
    extract::State,
    routing::{get, put},
};
use ryframe_auth::jwt::Claims;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_service::system::profile_service::UserProfileResponse;
use uuid::Uuid;

use crate::{
    dto::profile_dto::{ChangePasswordRequest, UpdateProfileRequest},
    handlers::auth_handler::AppState,
};

/// 个人中心路由
pub fn profile_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", get(get_profile).put(update_profile))
        .route("/password", put(change_password))
        .route("/avatar", put(update_avatar))
        .with_state(state)
}

/// 获取个人信息
/// 获取个人信息
#[utoipa::path(get, path = "/api/v1/auth/profile", tag = "个人中心",
    responses((status = 200, description = "个人信息")), security(("bearer" = [])))]
pub async fn get_profile(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
) -> AppResult<Json<ApiResponse<UserProfileResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| AppError::Authentication("令牌无效".into()))?;

    // 注意：这里需要将 UUID 转换为 i64
    // 实际项目中应该统一 ID 类型
    let user_id_i64 = user_id.as_u128() as i64;

    let profile = state
        .profile_service
        .get_profile(&state.db, user_id_i64)
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
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| AppError::Authentication("令牌无效".into()))?;

    let user_id_i64 = user_id.as_u128() as i64;

    state
        .profile_service
        .update_profile(
            &state.db,
            user_id_i64,
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

    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| AppError::Authentication("令牌无效".into()))?;

    let user_id_i64 = user_id.as_u128() as i64;

    state
        .profile_service
        .change_password(&state.db, user_id_i64, &req.old_password, &req.new_password)
        .await?;

    Ok(Json(ApiResponse::success_no_data_with_msg("密码修改成功")))
}

/// 更新头像
pub async fn update_avatar(
    State(state): State<AppState>,
    claims: axum::Extension<Claims>,
    Json(req): Json<serde_json::Value>,
) -> AppResult<Json<ApiResponse<()>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| AppError::Authentication("令牌无效".into()))?;

    let user_id_i64 = user_id.as_u128() as i64;

    let avatar_url = req["avatar_url"]
        .as_str()
        .ok_or_else(|| AppError::Validation("头像URL不能为空".into()))?
        .to_string();

    state
        .profile_service
        .update_avatar(&state.db, user_id_i64, avatar_url)
        .await?;

    Ok(Json(ApiResponse::success_no_data_with_msg("头像更新成功")))
}
