/// replay_protection 中间件测试
/// 从 crates/ryframe-middleware/src/replay_protection.rs 内联测试迁移
use axum::{
    Router, body::Body, extract::Request, middleware, response::IntoResponse, routing::get,
};
use chrono::Utc;
use ryframe_middleware::replay_protection::{ReplayProtectionState, replay_protection_middleware};
use tower::util::ServiceExt;

async fn handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({"status": "ok"}))
}

fn test_router(state: ReplayProtectionState) -> Router {
    Router::new()
        .route("/test", get(handler))
        .layer(middleware::from_fn_with_state(
            state,
            replay_protection_middleware,
        ))
}

#[tokio::test]
async fn test_no_headers_passes_through() {
    let state = ReplayProtectionState::new(None, 300);
    let req = Request::builder()
        .uri("/test")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let resp = test_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_valid_timestamp_and_nonce() {
    let state = ReplayProtectionState::new(None, 300);
    let now = Utc::now().timestamp();
    let nonce = uuid::Uuid::new_v4().to_string();

    let req = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", now.to_string())
        .header("X-Nonce", &nonce)
        .body(Body::empty())
        .unwrap();
    let resp = test_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_replay_nonce_rejected() {
    let state = ReplayProtectionState::new(None, 300);
    let now = Utc::now().timestamp();
    let nonce = "replay-nonce-001";

    // 首次请求
    let req1 = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", now.to_string())
        .header("X-Nonce", nonce)
        .body(Body::empty())
        .unwrap();
    let resp1 = test_router(state.clone()).oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), 200);

    // 重放请求
    let req2 = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", now.to_string())
        .header("X-Nonce", nonce)
        .body(Body::empty())
        .unwrap();
    let resp2 = test_router(state).oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), 409);
}

#[tokio::test]
async fn test_expired_timestamp_rejected() {
    let state = ReplayProtectionState::new(None, 300);
    // 10 分钟前的时间戳
    let old_ts = Utc::now().timestamp() - 600;
    let nonce = uuid::Uuid::new_v4().to_string();

    let req = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", old_ts.to_string())
        .header("X-Nonce", &nonce)
        .body(Body::empty())
        .unwrap();
    let resp = test_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_future_timestamp_rejected() {
    let state = ReplayProtectionState::new(None, 300);
    // 10 分钟后的时间戳
    let future_ts = Utc::now().timestamp() + 600;
    let nonce = uuid::Uuid::new_v4().to_string();

    let req = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", future_ts.to_string())
        .header("X-Nonce", &nonce)
        .body(Body::empty())
        .unwrap();
    let resp = test_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_missing_nonce_with_timestamp_rejected() {
    let state = ReplayProtectionState::new(None, 300);
    let now = Utc::now().timestamp();

    let req = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", now.to_string())
        .body(Body::empty())
        .unwrap();
    let resp = test_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_different_nonces_independent() {
    let state = ReplayProtectionState::new(None, 300);
    let now = Utc::now().timestamp();

    let req_a = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", now.to_string())
        .header("X-Nonce", "nonce-a")
        .body(Body::empty())
        .unwrap();
    let resp_a = test_router(state.clone()).oneshot(req_a).await.unwrap();
    assert_eq!(resp_a.status(), 200);

    let req_b = Request::builder()
        .uri("/test")
        .method("GET")
        .header("X-Timestamp", now.to_string())
        .header("X-Nonce", "nonce-b")
        .body(Body::empty())
        .unwrap();
    let resp_b = test_router(state).oneshot(req_b).await.unwrap();
    assert_eq!(resp_b.status(), 200);
}
