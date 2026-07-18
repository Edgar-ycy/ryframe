use std::convert::Infallible;

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode, header},
    middleware,
    routing::post,
};
use futures_util::stream;
use ryframe_config::UploadLimitsConfig;
use tower::util::ServiceExt;

fn router() -> Router {
    let limits = UploadLimitsConfig {
        file_max_bytes: 10,
        avatar_max_bytes: 5,
        multipart_envelope_bytes: 1,
        upload_timeout_seconds: 120,
        api_timeout_seconds: 30,
    };
    Router::new()
        .route("/api/v1/auth/profile/avatar", post(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            limits,
            ryframe_middleware::body_limit_middleware,
        ))
}

fn chunked_body(chunks: &[&'static [u8]]) -> Body {
    let chunks = chunks
        .iter()
        .copied()
        .map(|chunk| Ok::<Bytes, Infallible>(Bytes::from_static(chunk)))
        .collect::<Vec<_>>();
    Body::from_stream(stream::iter(chunks))
}

#[tokio::test]
async fn chunked_body_at_limit_is_accepted() {
    let request = Request::post("/api/v1/auth/profile/avatar")
        .body(chunked_body(&[b"123", b"456"]))
        .unwrap();
    assert_eq!(
        router().oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn chunked_body_over_limit_returns_uniform_413() {
    let request = Request::post("/api/v1/auth/profile/avatar")
        .body(chunked_body(&[b"123", b"4567"]))
        .unwrap();
    let response = router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 413);
    assert_eq!(json["msg"], "request body is too large");
}
