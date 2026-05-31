use std::time::Duration;

/// idempotency 中间件测试
/// 从 crates/ryframe-middleware/src/idempotency.rs 内联测试迁移
use axum::{
    Router, body::Body, extract::Request, middleware, response::IntoResponse, routing::post,
};
use ryframe_middleware::idempotency::{IdempotencyState, idempotency_middleware};
use tower::util::ServiceExt;

async fn handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({"result": "created", "id": 1}))
}

fn test_router(state: IdempotencyState) -> Router {
    Router::new()
        .route("/test", post(handler))
        .layer(middleware::from_fn_with_state(
            state,
            idempotency_middleware,
        ))
}

#[tokio::test]
async fn test_no_idempotency_key_passes_through() {
    let state = IdempotencyState::new(None, 60);
    let router = test_router(state);

    let req = Request::builder()
        .uri("/test")
        .method("POST")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(!resp.headers().contains_key("X-Idempotency-Replay"));
}

#[tokio::test]
async fn test_idempotency_first_request() {
    let state = IdempotencyState::new(None, 60);
    let req = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-001")
        .body(Body::empty())
        .unwrap();
    let resp = test_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(!resp.headers().contains_key("X-Idempotency-Replay"));
}

#[tokio::test]
async fn test_idempotency_replay() {
    let state = IdempotencyState::new(None, 60);

    // 首次请求
    let req1 = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-002")
        .body(Body::empty())
        .unwrap();
    let resp1 = test_router(state.clone()).oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), 200);

    // 重复请求 - 应返回缓存结果
    let req2 = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-002")
        .body(Body::empty())
        .unwrap();
    let resp2 = test_router(state).oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), 200);
    assert_eq!(resp2.headers().get("X-Idempotency-Replay").unwrap(), "true");
}

#[tokio::test]
async fn test_different_keys_independent() {
    let state = IdempotencyState::new(None, 60);

    let req_a = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-a")
        .body(Body::empty())
        .unwrap();
    let resp_a = test_router(state.clone()).oneshot(req_a).await.unwrap();
    assert!(!resp_a.headers().contains_key("X-Idempotency-Replay"));

    let req_b = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-b")
        .body(Body::empty())
        .unwrap();
    let resp_b = test_router(state).oneshot(req_b).await.unwrap();
    assert!(!resp_b.headers().contains_key("X-Idempotency-Replay"));
}

#[tokio::test]
async fn test_idempotency_expiry() {
    let state = IdempotencyState::new(None, 1); // 1秒 TTL

    // 首次请求
    let req1 = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-expire")
        .body(Body::empty())
        .unwrap();
    let _ = test_router(state.clone()).oneshot(req1).await.unwrap();

    // 等待过期
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 过期后再次请求，应视为新请求
    let req2 = Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", "key-expire")
        .body(Body::empty())
        .unwrap();
    let resp2 = test_router(state).oneshot(req2).await.unwrap();
    assert!(!resp2.headers().contains_key("X-Idempotency-Replay"));
}
