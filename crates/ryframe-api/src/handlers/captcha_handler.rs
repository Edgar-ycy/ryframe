use axum::{
    Json, Router,
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ryframe_common::utils::captcha::{CaptchaType, generate_captcha};
use ryframe_common::{AppError, AppResult, CAPTCHA_KEY_PREFIX};
use ryframe_core::RedisClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
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

/// 验证码条目（含过期时间，仅内存模式使用）
pub(crate) struct CaptchaEntry {
    answer: String,
    created_at: Instant,
}

/// 验证码存储（支持 Redis / 内存双模式）
///
/// - Redis 模式：使用 Redis SET EX，自动过期，无需后台 GC
/// - 内存模式：HashMap + TTL，后台每 2 分钟自动清理
#[derive(Clone)]
#[allow(private_interfaces)]
pub enum CaptchaStore {
    /// Redis 存储（生产推荐）
    Redis {
        client: Box<RedisClient>,
        ttl_secs: u64,
    },
    /// 内存存储（开发/降级模式）
    InMemory {
        inner: Arc<Mutex<HashMap<String, CaptchaEntry>>>,
        ttl: Duration,
    },
}

impl CaptchaStore {
    /// 创建 Redis 模式的验证码存储
    pub fn new_redis(client: RedisClient, ttl_secs: u64) -> Self {
        Self::Redis {
            client: Box::new(client),
            ttl_secs,
        }
    }

    /// 创建内存模式的验证码存储，指定 TTL（秒）
    pub fn new_in_memory(ttl_secs: u64) -> Self {
        Self::InMemory {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// 存储验证码
    pub async fn set(&self, id: String, answer: String) {
        match self {
            Self::Redis { client, ttl_secs } => {
                let key = format!("{}{}", CAPTCHA_KEY_PREFIX, id);
                if let Err(e) = client.set_ex(&key, &answer, *ttl_secs).await {
                    tracing::error!("Redis SET 验证码失败: {}", e);
                }
            }
            Self::InMemory { inner, ttl: _ } => {
                let mut store = inner.lock().await;
                store.insert(
                    id,
                    CaptchaEntry {
                        answer,
                        created_at: Instant::now(),
                    },
                );
            }
        }
    }

    /// 校验验证码（一次性使用，校验后自动删除）
    pub async fn verify(&self, id: &str, code: &str) -> bool {
        match self {
            Self::Redis { client, .. } => {
                let key = format!("{}{}", CAPTCHA_KEY_PREFIX, id);
                match client.get_and_del(&key).await {
                    Ok(Some(stored)) => stored.to_lowercase() == code.to_lowercase(),
                    Ok(None) => false,
                    Err(e) => {
                        tracing::error!("Redis GET 验证码失败: {}", e);
                        false
                    }
                }
            }
            Self::InMemory { inner, ttl } => {
                let mut store = inner.lock().await;
                if let Some(entry) = store.remove(id) {
                    if entry.created_at.elapsed() > *ttl {
                        return false;
                    }
                    entry.answer.to_lowercase() == code.to_lowercase()
                } else {
                    false
                }
            }
        }
    }

    /// 启动后台定时清理任务（仅内存模式有效）
    pub fn spawn_gc(&self) {
        if let Self::InMemory { inner, ttl } = self {
            let inner = inner.clone();
            let ttl = *ttl;
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(120));
                loop {
                    interval.tick().await;
                    let mut store = inner.lock().await;
                    store.retain(|_, entry| entry.created_at.elapsed() <= ttl);
                }
            });
        }
        // Redis 模式由 Redis 自身 TTL 机制管理，无需后台 GC
    }
}

/// 验证码路由
pub fn captcha_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/generate", get(generate_captcha_handler))
        .route("/verify", post(verify_captcha_handler))
        .with_state(state)
}

/// 生成验证码
pub async fn generate_captcha_handler(
    State(state): State<AppState>,
    Query(query): Query<CaptchaQuery>,
) -> AppResult<Json<CaptchaResponse>> {
    let captcha_type = match query.captcha_type.as_str() {
        "math" => CaptchaType::Math,
        _ => CaptchaType::Alphanumeric,
    };

    let captcha = generate_captcha(captcha_type)?;

    // 生成验证码 ID
    let captcha_id = Uuid::now_v7().to_string();

    // 存储验证码答案
    state
        .captcha_store
        .set(captcha_id.clone(), captcha.answer)
        .await;

    // 将图片转换为 Base64
    let image_base64 = STANDARD.encode(&captcha.image_data);

    Ok(Json(CaptchaResponse {
        captcha_id,
        image_base64: format!("data:image/png;base64,{}", image_base64),
    }))
}

/// 校验验证码
pub async fn verify_captcha_handler(
    State(state): State<AppState>,
    Json(req): Json<CaptchaVerifyRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let valid = state.captcha_store.verify(&req.captcha_id, &req.code).await;

    if valid {
        Ok(Json(serde_json::json!({
            "valid": true,
            "message": "验证码正确"
        })))
    } else {
        Err(AppError::Validation("验证码错误或已过期".into()))
    }
}

/// 返回验证码图片（PNG 格式）
pub async fn captcha_image_handler(
    State(state): State<AppState>,
    Query(query): Query<CaptchaQuery>,
) -> AppResult<impl IntoResponse> {
    let captcha_type = match query.captcha_type.as_str() {
        "math" => CaptchaType::Math,
        _ => CaptchaType::Alphanumeric,
    };

    let captcha = generate_captcha(captcha_type)?;

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
