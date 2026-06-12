use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{ConnectInfo, Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ryframe_common::{
    ApiResponse, AppError, AppResult,
    utils::captcha::{CaptchaType, generate_captcha},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::handlers::auth_handler::AppState;

/// 验证码生成查询参数
#[derive(Debug, Deserialize)]
pub struct CaptchaQuery {
    /// 验证码类型: alphanumeric（字母数字）/ math（数学计算）
    #[serde(default = "default_captcha_type")]
    pub captcha_type: String,
}

fn default_captcha_type() -> String {
    "alphanumeric".to_string()
}

/// 验证码响应
#[derive(Debug, Serialize)]
pub struct CaptchaResponse {
    /// 验证码 UUID（用于后续校验）
    pub captcha_id: String,
    /// 验证码图片（Base64 编码）
    pub image_base64: String,
}

/// 验证码校验请求
#[derive(Debug, Deserialize)]
pub struct CaptchaVerifyRequest {
    pub captcha_id: String,
    pub code: String,
}

/// 验证码路由
///
/// 不内嵌 `.with_state()`，由父路由统一注入 AppState。
pub fn captcha_router() -> Router<AppState> {
    Router::new()
        .route("/generate", get(generate_captcha_handler))
        .route("/image", get(captcha_image_handler))
        .route("/verify", post(verify_captcha_handler))
        .route("/config", get(get_captcha_config_handler))
}

/// 生成验证码
pub async fn generate_captcha_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<CaptchaQuery>,
) -> AppResult<Json<ApiResponse<CaptchaResponse>>> {
    // 验证码生成频率限制（每个 IP 每分钟最多 10 次）
    let captcha_key = format!("captcha:gen:{}", addr.ip());
    if !state.rate_limiter.try_acquire(&captcha_key).await {
        return Err(AppError::Validation(
            "验证码请求过于频繁，请稍后再试".into(),
        ));
    }

    let captcha_type = match query.captcha_type.as_str() {
        "math" => CaptchaType::Math,
        _ => CaptchaType::Alphanumeric,
    };

    let captcha = generate_captcha(captcha_type)?;

    // 输出验证码到日志（开发调试用）
    tracing::info!(
        "验证码生成: answer={}, type={}",
        captcha.answer,
        query.captcha_type
    );

    // 生成验证码 ID
    let captcha_id = Uuid::now_v7().to_string();

    // 存储验证码答案
    state
        .captcha_store
        .set(captcha_id.clone(), captcha.answer)
        .await;

    // 将图片转换为 Base64
    let image_base64 = STANDARD.encode(&captcha.image_data);

    Ok(Json(ApiResponse::success(CaptchaResponse {
        captcha_id,
        image_base64: format!("data:image/png;base64,{}", image_base64),
    })))
}

/// 校验验证码
pub async fn verify_captcha_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<CaptchaVerifyRequest>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    // 验证码校验频率限制（每个 IP 每分钟最多 5 次）
    let verify_key = format!("captcha:verify:{}", addr.ip());
    if !state.rate_limiter.try_acquire(&verify_key).await {
        return Err(AppError::Validation(
            "验证码校验过于频繁，请稍后再试".into(),
        ));
    }

    let valid = state.captcha_store.verify(&req.captcha_id, &req.code).await;

    if valid {
        Ok(Json(ApiResponse::success(
            serde_json::json!({"valid": true}),
        )))
    } else {
        Err(AppError::Validation("验证码错误或已过期".into()))
    }
}

/// 查询验证码开关状态
///
/// GET /api/v1/auth/captcha/config（公开接口，无需认证）
/// 返回 sys.account.captchaEnabled 配置值。
pub async fn get_captcha_config_handler(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {
    let enabled = state
        .config_service
        .find_by_key(&state.db, "sys.account.captchaEnabled")
        .await
        .ok()
        .flatten()
        .map(|c| c.value == "true")
        .unwrap_or(true); // 配置缺失时默认开启

    Ok(Json(ApiResponse::success(
        serde_json::json!({"captcha_enabled": enabled}),
    )))
}

/// 返回验证码图片（PNG 格式）
pub async fn captcha_image_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<CaptchaQuery>,
) -> AppResult<impl IntoResponse> {
    // 验证码生成频率限制（每个 IP 每分钟最多 10 次）
    let captcha_key = format!("captcha:gen:{}", addr.ip());
    if !state.rate_limiter.try_acquire(&captcha_key).await {
        return Err(AppError::Validation(
            "验证码请求过于频繁，请稍后再试".into(),
        ));
    }

    let captcha_type = match query.captcha_type.as_str() {
        "math" => CaptchaType::Math,
        _ => CaptchaType::Alphanumeric,
    };

    let captcha = generate_captcha(captcha_type)?;

    // 输出验证码到日志（开发调试用）
    tracing::info!(
        "验证码生成: answer={}, type={}",
        captcha.answer,
        query.captcha_type
    );

    // 生成验证码 ID 并存储
    let captcha_id = Uuid::now_v7().to_string();
    state
        .captcha_store
        .set(captcha_id.clone(), captcha.answer)
        .await;

    // 构建响应头
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "image/png"
            .parse()
            .map_err(|e| AppError::Internal(format!("设置 Content-Type 失败: {}", e)))?,
    );
    // 在响应头中返回验证码 ID
    headers.insert(
        "X-Captcha-Id",
        captcha_id
            .parse()
            .map_err(|e| AppError::Internal(format!("设置 X-Captcha-Id 失败: {}", e)))?,
    );

    Ok((headers, captcha.image_data))
}
