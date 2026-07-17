use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ryframe_common::{
    ApiResponse, AppError, AppResult,
    utils::captcha::{CaptchaType, generate_captcha},
};
use ryframe_macro::{get, post, route};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{handler_utils::tenant_id_from_headers, state::AppState};

/// 验证码生成查询参数
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptchaKind {
    #[default]
    Alphanumeric,
    Math,
}

impl From<CaptchaKind> for CaptchaType {
    fn from(value: CaptchaKind) -> Self {
        match value {
            CaptchaKind::Alphanumeric => Self::Alphanumeric,
            CaptchaKind::Math => Self::Math,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct CaptchaQuery {
    /// 验证码类型: alphanumeric（字母数字）/ math（数学计算）
    #[serde(default)]
    pub captcha_type: CaptchaKind,
}

/// 验证码响应
#[derive(Debug, Serialize, ToSchema)]
pub struct CaptchaResponse {
    /// 验证码 UUID（用于后续校验）
    pub captcha_id: String,
    /// 验证码图片（Base64 编码）
    pub image_base64: String,
}

/// 验证码校验请求
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptchaVerifyRequest {
    pub captcha_id: String,
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CaptchaVerifyResponse {
    pub valid: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CaptchaConfigResponse {
    pub captcha_enabled: bool,
}

/// 验证码路由
///
/// 不内嵌 `.with_state()`，由父路由统一注入 AppState。
pub fn captcha_router() -> Router<AppState> {
    Router::new()
        .merge(route!(generate_captcha_handler))
        .merge(route!(captcha_image_handler))
        .merge(route!(verify_captcha_handler))
        .merge(route!(get_captcha_config_handler))
}

/// 生成验证码
#[get("/generate")]
#[utoipa::path(get, path = "/api/v1/auth/captcha/generate", tag = "认证",
    params(CaptchaQuery),
    responses((status = 200, description = "生成验证码", body = ApiResponse<CaptchaResponse>)))]
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

    let (captcha_id, image_data) = issue_captcha(&state, query.captcha_type).await?;

    // 将图片转换为 Base64
    let image_base64 = STANDARD.encode(image_data);

    Ok(Json(ApiResponse::success(CaptchaResponse {
        captcha_id,
        image_base64: format!("data:image/png;base64,{}", image_base64),
    })))
}

/// 校验验证码
#[post("/verify")]
#[utoipa::path(post, path = "/api/v1/auth/captcha/verify", tag = "认证",
    responses((status = 200, description = "验证码校验结果", body = ApiResponse<CaptchaVerifyResponse>)))]
pub async fn verify_captcha_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<CaptchaVerifyRequest>,
) -> AppResult<Json<ApiResponse<CaptchaVerifyResponse>>> {
    // 验证码校验频率限制（每个 IP 每分钟最多 5 次）
    let verify_key = format!("captcha:verify:{}", addr.ip());
    if !state.rate_limiter.try_acquire(&verify_key).await {
        return Err(AppError::Validation(
            "验证码校验过于频繁，请稍后再试".into(),
        ));
    }

    let valid = state
        .services
        .captcha
        .verify(&req.captcha_id, &req.code)
        .await;

    if valid {
        Ok(Json(ApiResponse::success(CaptchaVerifyResponse {
            valid: true,
        })))
    } else {
        Err(AppError::Validation("验证码错误或已过期".into()))
    }
}

/// 查询验证码开关状态
///
/// GET /api/v1/auth/captcha/config（公开接口，无需认证）
/// 返回 sys.account.captchaEnabled 配置值。
#[get("/config")]
#[utoipa::path(get, path = "/api/v1/auth/captcha/config", tag = "认证",
    responses((status = 200, description = "验证码开关", body = ApiResponse<CaptchaConfigResponse>)))]
pub async fn get_captcha_config_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<ApiResponse<CaptchaConfigResponse>>> {
    let tenant_id = tenant_id_from_headers(&headers)?;
    let enabled = state
        .services
        .config
        .find_public_value(&tenant_id, "sys.account.captchaEnabled")
        .await
        .ok()
        .flatten()
        .map(|value| value == "true")
        .unwrap_or(true); // 配置缺失时默认开启

    Ok(Json(ApiResponse::success(CaptchaConfigResponse {
        captcha_enabled: enabled,
    })))
}

/// 返回验证码图片（PNG 格式）
#[get("/image")]
#[utoipa::path(get, path = "/api/v1/auth/captcha/image", tag = "认证",
    params(CaptchaQuery),
    responses((status = 200, description = "验证码 PNG 图片", body = Vec<u8>, content_type = "image/png")))]
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

    let (captcha_id, image_data) = issue_captcha(&state, query.captcha_type).await?;

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

    Ok((headers, image_data))
}

async fn issue_captcha(
    state: &AppState,
    captcha_kind: CaptchaKind,
) -> AppResult<(String, Vec<u8>)> {
    let (answer, image_data) = generate_captcha(captcha_kind.into())?.into_parts();
    let captcha_id = Uuid::now_v7().to_string();
    state.services.captcha.set(captcha_id.clone(), answer).await;
    Ok((captcha_id, image_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captcha_query_defaults_and_rejects_unknown_kinds() {
        let default: CaptchaQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(default.captcha_type, CaptchaKind::Alphanumeric);

        let math: CaptchaQuery = serde_json::from_str(r#"{"captcha_type":"math"}"#).unwrap();
        assert_eq!(math.captcha_type, CaptchaKind::Math);

        assert!(serde_json::from_str::<CaptchaQuery>(r#"{"captcha_type":"unsupported"}"#).is_err());
    }
}
