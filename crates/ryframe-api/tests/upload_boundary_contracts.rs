use std::io;

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{DefaultBodyLimit, Multipart, multipart::MultipartRejection},
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    routing::post,
};
use futures_util::stream;
use ryframe_api::{
    http::HttpResult, middleware::body_limit::body_limit_middleware, settings::UploadSettings,
};
use tower::ServiceExt;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn limits() -> UploadSettings {
    UploadSettings {
        file_max_bytes: 4 * 1024 * 1024,
        avatar_max_bytes: 3 * 1024 * 1024,
        multipart_envelope_bytes: 1024,
        upload_timeout_seconds: 60,
        api_timeout_seconds: 30,
    }
}

fn router() -> Router {
    Router::new()
        .route("/api/v1/common/upload", post(|| async { "已读取" }))
        .route("/api/v1/auth/profile/avatar", post(|| async { "已读取" }))
        .layer(from_fn_with_state(limits(), body_limit_middleware))
}

async fn assert_error(body: Body, path: &str, status: StatusCode, key: &str) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .body(body)
        .unwrap();
    assert!(request.headers().get("content-length").is_none());
    let response = router().oneshot(request).await.unwrap();
    assert_response_error(response, status, key).await;
}

async fn assert_response_error(response: axum::response::Response, status: StatusCode, key: &str) {
    assert_eq!(response.status(), status);
    let data = to_bytes(response.into_body(), 4096).await.unwrap();
    let data: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(data["error_key"], key);
    assert_eq!(data["code"], status.as_u16());
}

async fn read_multipart(form: Result<Multipart, MultipartRejection>) -> HttpResult<String> {
    let mut form = form?;
    let mut count = 0;
    while let Some(field) = form.next_field().await? {
        count += field.bytes().await?.len();
    }
    Ok(count.to_string())
}

fn multipart_request(body: Body, media: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", media)
        .body(body)
        .unwrap()
}

#[test]
fn multipart_errors_keep_malformed_size_and_io_categories() {
    runtime().block_on(async {
        let app = Router::new().route("/", post(read_multipart));
        let header =
            b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\n";
        let malformed = [header.as_slice(), b"unfinished"].concat();
        let oversized = [
            header.as_slice(),
            &vec![b'a'; 3 * 1024 * 1024],
            b"\r\n--test--\r\n",
        ]
        .concat();
        for (body, media, status, key) in [
            (
                Body::empty(),
                "multipart/form-data",
                StatusCode::BAD_REQUEST,
                "validation",
            ),
            (
                Body::from(malformed),
                "multipart/form-data; boundary=test",
                StatusCode::BAD_REQUEST,
                "validation",
            ),
            (
                Body::from(oversized),
                "multipart/form-data; boundary=test",
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
            ),
            (
                Body::from_stream(stream::iter([
                    Ok(Bytes::from_static(header)),
                    Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "测试上传流中断",
                    )),
                ])),
                "multipart/form-data; boundary=test",
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(multipart_request(body, media))
                .await
                .unwrap();
            assert_response_error(response, status, key).await;
        }
    });
}

#[test]
fn configured_upload_envelope_allows_four_mib_without_the_extractor_default_cap() {
    runtime().block_on(async {
        let app = Router::new()
            .route("/api/v1/common/upload", post(read_multipart))
            .layer(DefaultBodyLimit::disable())
            .layer(from_fn_with_state(limits(), body_limit_middleware));
        let content = [
            b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\n"
                .as_slice(),
            &vec![b'a'; limits().file_max_bytes],
            b"\r\n--test--\r\n",
        ]
        .concat();
        let mut request =
            multipart_request(Body::from(content), "multipart/form-data; boundary=test");
        *request.uri_mut() = "/api/v1/common/upload".parse().unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 128).await.unwrap(),
            limits().file_max_bytes.to_string()
        );
    });
}

#[test]
fn chunked_body_cannot_exceed_configured_file_or_avatar_limit() {
    runtime().block_on(async {
        for (path, limit) in [
            ("/api/v1/common/upload", limits().file_max_bytes),
            ("/api/v1/auth/profile/avatar", limits().avatar_max_bytes),
        ] {
            let body = Body::from_stream(stream::iter([
                Ok::<_, io::Error>(Bytes::from(vec![b'a'; limit])),
                Ok(Bytes::from(vec![
                    b'b';
                    limits().multipart_envelope_bytes + 1
                ])),
            ]));
            assert_error(
                body,
                path,
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
            )
            .await;
        }
    });
}

#[test]
fn request_body_io_failure_is_not_reported_as_an_upload_size_error() {
    runtime().block_on(async {
        let body = Body::from_stream(stream::iter([
            Ok(Bytes::from_static(b"received chunk")),
            Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "测试连接读取中断",
            )),
        ]));
        assert_error(
            body,
            "/api/v1/common/upload",
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
        )
        .await;
    });
}
