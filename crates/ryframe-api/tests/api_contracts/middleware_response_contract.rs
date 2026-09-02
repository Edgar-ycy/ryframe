use std::{collections::HashMap, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::RETRY_AFTER},
    middleware::{from_fn, from_fn_with_state},
    routing::get,
};
use ryframe_api::{
    auth_middleware::require_permission,
    middleware::rate_limit::{
        RateLimitState, api_rate_limit_middleware, rate_limit_middleware,
        user_rate_limit_middleware,
    },
    rate_limit::{HttpRateLimiter, RateLimitDecision},
    settings::RateLimitSettings,
};
use ryframe_auth::jwt::Claims;
use tower::ServiceExt;

struct FixedLimiter(Result<RateLimitDecision, String>);

#[async_trait::async_trait]
impl HttpRateLimiter for FixedLimiter {
    async fn acquire(&self, _: &str, _: u64, _: u32) -> Result<RateLimitDecision, String> {
        self.0.clone()
    }
}

fn settings(enabled: bool) -> RateLimitSettings {
    RateLimitSettings {
        enabled,
        capacity: 10,
        window_secs: 60,
        enable_user_rate_limit: true,
        user_window_secs: 60,
        user_capacity: 10,
        api_limits: HashMap::from([("GET".to_owned(), 10)]),
        api_window_secs: 60,
    }
}

fn claims() -> Claims {
    Claims {
        sub: "1".to_owned(),
        tenant_id: "system".to_owned(),
        tenant_session_version: 1,
        user_authorization_version: 1,
        username: "测试用户".to_owned(),
        token_type: "access".to_owned(),
        sid: "测试会话".to_owned(),
        jti: "测试令牌".to_owned(),
        iat: 1,
        exp: usize::MAX,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时应可用")
}

#[test]
fn rate_limit_responses_preserve_status_body_and_retry_header() {
    runtime().block_on(async {
        for kind in 0..3 {
            for (decision, expected, retry_after) in [
                (
                    Ok(RateLimitDecision {
                        allowed: true,
                        retry_after_secs: 0,
                    }),
                    StatusCode::OK,
                    None,
                ),
                (
                    Ok(RateLimitDecision {
                        allowed: false,
                        retry_after_secs: 0,
                    }),
                    StatusCode::TOO_MANY_REQUESTS,
                    Some("1"),
                ),
                (
                    Ok(RateLimitDecision {
                        allowed: false,
                        retry_after_secs: 12,
                    }),
                    StatusCode::TOO_MANY_REQUESTS,
                    Some("12"),
                ),
                (
                    Err("测试后端不可用".to_owned()),
                    StatusCode::SERVICE_UNAVAILABLE,
                    None,
                ),
            ] {
                let state = RateLimitState {
                    limiter: Arc::new(FixedLimiter(decision)),
                    config: Arc::new(settings(true)),
                };
                let app = limited_router(kind, state);
                let mut request = Request::builder().uri("/test").body(Body::empty()).unwrap();
                request.extensions_mut().insert(claims());
                let response = app.oneshot(request).await.unwrap();
                assert_eq!(response.status(), expected, "限流层 {kind}");
                assert_eq!(
                    response
                        .headers()
                        .get(RETRY_AFTER)
                        .map(|value| value.to_str().unwrap()),
                    retry_after
                );
                let body = to_bytes(response.into_body(), 1024).await.unwrap();
                let body = std::str::from_utf8(&body).unwrap();
                match expected {
                    StatusCode::OK => assert_eq!(body, "通过"),
                    StatusCode::TOO_MANY_REQUESTS => assert!(body.contains("请求过于频繁")),
                    _ => assert_eq!(body, "限流服务暂不可用，请稍后重试"),
                }
            }
        }
    });
}

fn limited_router(kind: usize, state: RateLimitState) -> Router {
    let app = Router::new().route("/test", get(|| async { "通过" }));
    match kind {
        0 => app.layer(from_fn_with_state(state, rate_limit_middleware)),
        1 => app.layer(from_fn_with_state(state, user_rate_limit_middleware)),
        _ => app.layer(from_fn_with_state(state, api_rate_limit_middleware)),
    }
}

#[test]
fn disabled_rate_limits_bypass_unavailable_backend() {
    runtime().block_on(async {
        for kind in 0..3 {
            let state = RateLimitState {
                limiter: Arc::new(FixedLimiter(Err("不应调用后端".to_owned()))),
                config: Arc::new(settings(false)),
            };
            let response = limited_router(kind, state)
                .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    });
}

#[test]
fn permission_middleware_typed_error_preserves_unauthorized_response() {
    runtime().block_on(async {
        let app = Router::new()
            .route("/test", get(protected_handler))
            .layer(from_fn(require_permission("system:post:list")));
        let response = app
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], 401);
        assert_eq!(body["error_key"], "authentication");
    });
}

async fn protected_handler() -> &'static str {
    panic!("未认证请求不得进入处理器")
}
