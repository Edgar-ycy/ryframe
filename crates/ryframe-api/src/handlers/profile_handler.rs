use axum::{
    Json, Router,
    extract::{Multipart, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiResponse, AppError, AppResult};
use ryframe_macro::{get, put, route};
use ryframe_service::system::profile_service::UserProfileResponse;
use validator::Validate;

use crate::{
    dto::{
        multipart_dto::FileUploadForm,
        profile_dto::{AvatarResponse, ChangePasswordRequest, UpdateProfileRequest},
    },
    state::AppState,
};

/// 个人中心路由
///
/// 不内嵌 `.with_state()`，由父路由统一注入 AppState。
/// 认证和操作日志中间件在父路由 (auth_router) 中统一注册。
pub fn profile_router() -> Router<AppState> {
    Router::new()
        .merge(route!(get_profile))
        .merge(route!(update_profile))
        .merge(route!(change_password))
        .merge(route!(update_avatar))
}

/// 获取个人信息
#[get("/")]
#[utoipa::path(get, path = "/api/v1/auth/profile", tag = "个人中心",
    responses((status = 200, description = "个人信息", body = ApiResponse<UserProfileResponse>)), security(("bearer" = [])))]
pub async fn get_profile(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<UserProfileResponse>>> {
    let profile = state.services.profile.get_profile(&current_user).await?;

    Ok(Json(ApiResponse::success(profile)))
}

/// 更新个人信息
#[put("/")]
#[utoipa::path(put, path = "/api/v1/auth/profile", tag = "个人中心",
    request_body = UpdateProfileRequest, responses((status = 200, description = "更新成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
pub async fn update_profile(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(req): Json<UpdateProfileRequest>,
) -> AppResult<Json<ApiResponse<()>>> {
    req.validate()?;
    state
        .services
        .profile
        .update_profile(
            &current_user,
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
#[put("/password")]
#[utoipa::path(put, path = "/api/v1/auth/profile/password", tag = "个人中心",
    request_body = ChangePasswordRequest, responses((status = 200, description = "修改成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
pub async fn change_password(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(req): Json<ChangePasswordRequest>,
) -> AppResult<Json<ApiResponse<()>>> {
    req.validate()?;

    state
        .services
        .profile
        .change_password(&current_user, &req.old_password, &req.new_password)
        .await?;

    Ok(Json(ApiResponse::success_no_data_with_msg("密码修改成功")))
}

/// 更新头像（直接接受文件上传）
///
/// 请求格式: multipart/form-data，字段名 `file`。
/// 上传后自动写入 sys_file 元数据表并更新 sys_user.avatar。
#[put("/avatar")]
#[utoipa::path(put, path = "/api/v1/auth/profile/avatar", tag = "个人中心",
    request_body(content = FileUploadForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "头像更新成功", body = ApiResponse<AvatarResponse>)),
    security(("bearer" = [])))]
pub async fn update_avatar(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<AvatarResponse>>> {
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
        avatar_url = state
            .services
            .file
            .upload_avatar(&current_user, filename, data.to_vec())
            .await?;
        break;
    }

    if avatar_url.is_empty() {
        return Err(AppError::Validation("未找到上传的头像文件".into()));
    }

    state
        .services
        .profile
        .update_avatar(&current_user, avatar_url.clone())
        .await?;

    Ok(Json(ApiResponse::success(AvatarResponse { avatar_url })))
}
