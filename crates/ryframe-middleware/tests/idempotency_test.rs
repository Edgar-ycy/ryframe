use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ActorContext, annotations::data_scope::DataScope};
use ryframe_config::{RedisConfig, RedisMode};
use ryframe_core::RedisClient;
use ryframe_middleware::idempotency::{IdempotencyState, idempotency_middleware};
use tower::util::ServiceExt;

#[derive(Clone)]
struct HandlerState(Arc<AtomicUsize>);

async fn handler(State(state): State<HandlerState>, body: String) -> impl IntoResponse {
    let invocation = state.0.fetch_add(1, Ordering::SeqCst) + 1;
    axum::Json(serde_json::json!({
        "body": body,
        "invocation": invocation,
    }))
}

async fn response_headers_handler(State(state): State<HandlerState>) -> Response {
    let invocation = state.0.fetch_add(1, Ordering::SeqCst) + 1;
    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/vnd.ryframe.test+json")
        .header(header::LOCATION, "/resources/42")
        .header(header::ETAG, "\"resource-v1\"")
        .header(header::SET_COOKIE, "private_session=secret; HttpOnly")
        .header(header::AUTHORIZATION, "Bearer response-secret")
        .header("x-internal-secret", "must-not-be-replayed")
        .body(Body::from(format!(r#"{{"invocation":{invocation}}}"#)))
        .unwrap()
}

async fn inject_principal(
    State(principal): State<RequestPrincipal>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().insert(principal);
    next.run(request).await
}

fn principal(tenant_id: &str, user_id: i64) -> RequestPrincipal {
    RequestPrincipal {
        actor: ActorContext {
            user_id,
            tenant_id: tenant_id.to_string(),
            username: format!("user-{user_id}"),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::All,
            custom_dept_ids: vec![],
            include_self: false,
            is_super_admin: false,
        },
        roles: vec![],
        role_ids: vec![],
        permissions: vec![],
        tenant_request_limit_per_minute: 100,
    }
}

fn test_router(
    state: IdempotencyState,
    principal: RequestPrincipal,
    calls: Arc<AtomicUsize>,
) -> Router {
    Router::new()
        .route("/test", post(handler))
        .route("/other", post(handler))
        .route("/resources/{id}", post(handler))
        .route("/response-headers", post(response_headers_handler))
        .with_state(HandlerState(calls))
        .layer(middleware::from_fn_with_state(
            state,
            idempotency_middleware,
        ))
        .layer(middleware::from_fn_with_state(principal, inject_principal))
}

fn request(key: Option<&str>, body: &str) -> Request {
    request_at("/test", key, body)
}

fn request_at(path: &str, key: Option<&str>, body: &str) -> Request {
    let mut builder = Request::builder().uri(path).method("POST");
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn multipart_request(key: &str, body: &str) -> Request {
    Request::builder()
        .uri("/test")
        .method("POST")
        .header("Idempotency-Key", key)
        .header("Content-Type", "multipart/form-data; boundary=ryframe-test")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn docker_redis() -> RedisClient {
    let port = std::env::var("RYFRAME_TEST_REDIS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(16379);
    RedisClient::connect(&RedisConfig {
        mode: RedisMode::Required,
        host: "127.0.0.1".into(),
        port,
        timeout_secs: 2,
        ..RedisConfig::default()
    })
    .await
    .expect(
        "connect Redis test service; run `docker compose -f docker-compose.test.yml up -d --wait`",
    )
}

#[tokio::test]
async fn requests_without_a_key_pass_through() {
    let calls = Arc::new(AtomicUsize::new(0));
    let response = test_router(
        IdempotencyState::new(None, 60),
        principal("tenant-a", 1),
        calls.clone(),
    )
    .oneshot(request(None, "first"))
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn multipart_uploads_are_never_cached_or_replayed() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    for _ in 0..2 {
        let response = test_router(state.clone(), principal.clone(), calls.clone())
            .oneshot(multipart_request("upload-key", "multipart payload"))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("X-Idempotency-Replay"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn completed_response_is_replayed_without_reexecution() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    let first = test_router(state.clone(), principal.clone(), calls.clone())
        .oneshot(request(Some("same-key"), "payload"))
        .await
        .unwrap();
    assert_eq!(first.status(), 200);

    let replay = test_router(state, principal, calls.clone())
        .oneshot(request(Some("same-key"), "payload"))
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.headers()["X-Idempotency-Replay"], "true");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn same_key_with_different_body_is_rejected() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    test_router(state.clone(), principal.clone(), calls.clone())
        .oneshot(request(Some("same-key"), "payload-a"))
        .await
        .unwrap();
    let conflict = test_router(state, principal, calls.clone())
        .oneshot(request(Some("same-key"), "payload-b"))
        .await
        .unwrap();

    assert_eq!(conflict.status(), 409);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tenant_and_user_scope_prevent_cross_account_replays() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));

    for principal in [
        principal("tenant-a", 1),
        principal("tenant-a", 2),
        principal("tenant-b", 1),
    ] {
        let response = test_router(state.clone(), principal, calls.clone())
            .oneshot(request(Some("shared-client-key"), "payload"))
            .await
            .unwrap();
        assert!(!response.headers().contains_key("X-Idempotency-Replay"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn different_route_templates_are_part_of_the_scope() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    for path in ["/test", "/other"] {
        let response = test_router(state.clone(), principal.clone(), calls.clone())
            .oneshot(request_at(path, Some("same-key"), "payload"))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("X-Idempotency-Replay"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn matched_route_template_uses_key_and_body_to_isolate_concrete_ids() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    let first = test_router(state.clone(), principal.clone(), calls.clone())
        .oneshot(request_at(
            "/resources/1",
            Some("resource-operation-a"),
            "payload-a",
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), 200);

    let different_key = test_router(state.clone(), principal.clone(), calls.clone())
        .oneshot(request_at(
            "/resources/2",
            Some("resource-operation-b"),
            "payload-a",
        ))
        .await
        .unwrap();
    assert_eq!(different_key.status(), 200);

    let different_body = test_router(state, principal, calls.clone())
        .oneshot(request_at(
            "/resources/3",
            Some("resource-operation-a"),
            "payload-b",
        ))
        .await
        .unwrap();
    assert_eq!(different_body.status(), StatusCode::CONFLICT);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn replay_preserves_allowlisted_headers_and_drops_sensitive_headers() {
    let state = IdempotencyState::new(None, 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    let first = test_router(state.clone(), principal.clone(), calls.clone())
        .oneshot(request_at(
            "/response-headers",
            Some("response-headers-key"),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    assert!(first.headers().contains_key(header::SET_COOKIE));
    assert!(first.headers().contains_key(header::AUTHORIZATION));

    let replay = test_router(state, principal, calls.clone())
        .oneshot(request_at(
            "/response-headers",
            Some("response-headers-key"),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(
        replay.headers()[header::CONTENT_TYPE],
        "application/vnd.ryframe.test+json"
    );
    assert_eq!(replay.headers()[header::LOCATION], "/resources/42");
    assert_eq!(replay.headers()[header::ETAG], "\"resource-v1\"");
    assert_eq!(replay.headers()["x-idempotency-replay"], "true");
    assert!(!replay.headers().contains_key(header::SET_COOKIE));
    assert!(!replay.headers().contains_key(header::AUTHORIZATION));
    assert!(!replay.headers().contains_key("x-internal-secret"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_result_executes_again() {
    let state = IdempotencyState::new(None, 1).with_processing_ttl(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let principal = principal("tenant-a", 1);

    test_router(state.clone(), principal.clone(), calls.clone())
        .oneshot(request(Some("expiring-key"), "payload"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = test_router(state, principal, calls.clone())
        .oneshot(request(Some("expiring-key"), "payload"))
        .await
        .unwrap();

    assert_eq!(second.status(), 200);
    assert!(!second.headers().contains_key("X-Idempotency-Replay"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let body = to_bytes(second.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("invocation"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redis_reservation_allows_only_one_side_effect_under_fifty_callers() {
    let state = IdempotencyState::new(Some(docker_redis().await), 60);
    let calls = Arc::new(AtomicUsize::new(0));
    let redis_principal = principal("tenant-redis", 42);
    let key = format!(
        "redis-concurrency-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );

    let mut tasks = Vec::new();
    for _ in 0..50 {
        let router = test_router(state.clone(), redis_principal.clone(), calls.clone());
        let request = request(Some(&key), "one-side-effect");
        tasks.push(tokio::spawn(async move {
            router.oneshot(request).await.unwrap()
        }));
    }

    for task in tasks {
        let status = task.await.unwrap().status();
        assert!(status == 200 || status == 409, "unexpected status {status}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let replay = test_router(state.clone(), redis_principal.clone(), calls.clone())
        .oneshot(request(Some(&key), "one-side-effect"))
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.headers()["X-Idempotency-Replay"], "true");

    let conflict = test_router(state.clone(), redis_principal, calls.clone())
        .oneshot(request(Some(&key), "different-body"))
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);

    let other_tenant = test_router(state, principal("tenant-other", 42), calls.clone())
        .oneshot(request(Some(&key), "one-side-effect"))
        .await
        .unwrap();
    assert_eq!(other_tenant.status(), 200);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
