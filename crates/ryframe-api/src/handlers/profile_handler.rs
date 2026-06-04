use axum::{
    Json, Router,
    extract::{Multipart, State},
    routing::{get, put},
};
use chrono::Utc;
use ryframe_auth::jwt::Claims;
use ryframe_common::{
    ApiResponse, AppError, AppResult,
    utils::file_upload::{
        compress_image, generate_storage_filename, get_content_type, validate_extension,
    },
};
use ryframe_core::repository::Repository;
use ryframe_db::{FileRepository, entities::sys_file};
use ryframe_service::system::profile_service::UserProfileResponse;

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

    const BUCKET: &str = "avatar";
    let allowed_extensions: Vec<String> = vec!["jpg", "jpeg", "png", "gif", "bmp", "webp"]
        .into_iter()
        .map(|s| s.to_string())
        .collect();

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

        // 文件大小限制 5MB
        if data.len() > 5 * 1024 * 1024 {
            return Err(AppError::Validation("头像文件大小不能超过 5MB".into()));
        }

        // 验证图片类型
        validate_extension(&filename, &allowed_extensions)?;

        // 图片压缩
        let (final_data, final_name) = compress_image(&data, &filename).unwrap_or_else(|e| {
            tracing::warn!("头像压缩失败，使用原始数据: {}", e);
            (data.to_vec(), filename.clone())
        });
        let content_type = get_content_type(&final_name);

        // 生成存储路径
        let storage_name = generate_storage_filename(&final_name);
        let date_prefix = Utc::now().format("%Y/%m/%d").to_string();
        let object_key = format!("{}/{}", date_prefix, storage_name);

        // 确保 bucket 存在
        state
            .object_storage
            .ensure_bucket(BUCKET)
            .await
            .map_err(|e| AppError::Internal(format!("创建存储桶失败: {}", e)))?;

        // 上传到对象存储
        state
            .object_storage
            .put(BUCKET, &object_key, &final_data, &content_type)
            .await
            .map_err(|e| AppError::Internal(format!("保存头像失败: {}", e)))?;

        // 生成公开访问 URL（用于 sys_user.avatar）
        let public_url = state.object_storage.public_url(BUCKET, &object_key);
        avatar_url = if public_url.is_empty() || public_url == "/" {
            format!(
                "/api/v1/common/file/download?bucket={}&path={}",
                BUCKET, object_key
            )
        } else {
            public_url
        };

        // 写入 sys_file 元数据表
        let relative_file_url = format!("{}/{}", BUCKET, object_key);
        let file_id = ryframe_common::utils::snowflake::next_snowflake_id();
        let model = sys_file::Model {
            id: file_id,
            original_name: filename.clone(),
            storage_name,
            storage_path: object_key.clone(),
            bucket: BUCKET.to_string(),
            file_url: relative_file_url,
            file_size: final_data.len() as i64,
            content_type,
            file_md5: None,
            upload_by: Some(user_id.to_string()),
            del_flag: sys_file::Model::DEL_FLAG_NORMAL.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        FileRepository
            .insert(&state.db, model)
            .await
            .map_err(|e| AppError::Internal(format!("写入文件元数据失败: {}", e)))?;

        // 只处理第一个文件
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
