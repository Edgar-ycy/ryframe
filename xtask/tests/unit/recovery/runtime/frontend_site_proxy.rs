use std::{convert::Infallible, sync::Mutex};

use axum::{
    body::Body,
    http::{HeaderValue, Request, Response, StatusCode, Uri, header},
};
use hyper::{body::Incoming, server::conn::http1 as server, service::service_fn};
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

use super::*;

#[test]
fn proxy_target_requires_the_literal_api_namespace_and_safe_origin_form() {
    for (method, uri, status) in [
        (Method::GET, "/api/v10/posts", StatusCode::NOT_FOUND),
        (Method::GET, "/login", StatusCode::NOT_FOUND),
        (
            Method::GET,
            "http://127.0.0.1/api/v1",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::CONNECT,
            "localhost:80",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::GET,
            "/api/v1/%2e%2E/readyz",
            StatusCode::BAD_REQUEST,
        ),
        (Method::GET, "/api/v1/a%2Fb", StatusCode::BAD_REQUEST),
        (Method::GET, "/api/v1/a%5Cb", StatusCode::BAD_REQUEST),
        (Method::GET, "/api/v1/a//b", StatusCode::BAD_REQUEST),
        (Method::GET, "/api/v1/a%00", StatusCode::BAD_REQUEST),
        (Method::GET, "/api/v1/a%", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            validate_target(&method, &uri.parse::<Uri>().unwrap()),
            Err(status)
        );
    }
    for uri in ["/api/v1", "/api/v1/files/%E6%B5%8B%E8%AF%95?path=a%2Fb"] {
        assert!(validate_target(&Method::POST, &uri.parse::<Uri>().unwrap()).is_ok());
    }
}

#[test]
fn proxy_header_boundaries_preserve_end_to_end_fields_and_distinct_cookies() {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/posts?limit=1")
        .header(header::ORIGIN, "http://127.0.0.1:15174")
        .header(header::AUTHORIZATION, "Bearer fixture")
        .header(header::COOKIE, "one=1; two=2")
        .header(header::CONNECTION, "keep-alive, x-fixture-hop")
        .header("x-fixture-hop", "drop")
        .header("x-forwarded-for", "198.19.11.4")
        .header("x-forwarded-host", "invalid")
        .header("x-ryframe-test-gate", "fixture-control")
        .body(Body::from("fixture body"))
        .unwrap();
    assert!(headers::request(&mut request).unwrap().is_none());
    assert_eq!(request.method(), Method::POST);
    assert_eq!(
        request.uri().path_and_query().unwrap().as_str(),
        "/api/v1/posts?limit=1"
    );
    let headers = request.headers_mut();
    assert_eq!(headers[header::ORIGIN], "http://127.0.0.1:15174");
    assert_eq!(headers[header::AUTHORIZATION], "Bearer fixture");
    assert_eq!(headers[header::COOKIE], "one=1; two=2");
    assert_eq!(headers["x-forwarded-for"], "198.19.11.4");
    for name in [
        "connection",
        "x-fixture-hop",
        "x-forwarded-host",
        "x-ryframe-test-gate",
    ] {
        assert!(!headers.contains_key(name));
    }
    headers.append("x-forwarded-for", HeaderValue::from_static("198.18.1.9"));
    assert!(headers::request(&mut request).unwrap().is_none());
    assert!(!request.headers().contains_key("x-forwarded-for"));

    let mut response = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, "/same-origin")
        .header(header::SET_COOKIE, "one=1; HttpOnly")
        .header(header::SET_COOKIE, "two=2; HttpOnly")
        .header(header::CONNECTION, "x-fixture-hop")
        .header("x-fixture-hop", "drop")
        .header("transfer-encoding", "chunked")
        .body(Body::empty())
        .unwrap();
    assert!(headers::response(&mut response, None).unwrap().is_none());
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/same-origin");
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cookies, ["one=1; HttpOnly", "two=2; HttpOnly"]);
    for name in ["connection", "x-fixture-hop", "transfer-encoding"] {
        assert!(!response.headers().contains_key(name));
    }
}

fn upgrade_request(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:15174")
        .header(header::ORIGIN, "http://127.0.0.1:15174")
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap()
}

#[test]
fn websocket_upgrade_requires_exact_endpoint_and_a_hyper_upgrade_handle() {
    for path in ["/api/v1/not-ws", "/api/v1/ws"] {
        let mut request = upgrade_request(path);
        assert!(matches!(
            headers::request(&mut request),
            Err(StatusCode::BAD_REQUEST)
        ));
    }
    let mut response = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .body(Body::empty())
        .unwrap();
    assert!(matches!(
        headers::response(&mut response, None),
        Err(StatusCode::BAD_GATEWAY)
    ));
}

async fn ws_fixture(
    mut request: Request<Incoming>,
) -> std::result::Result<Response<Body>, Infallible> {
    assert_eq!(
        request.uri().path_and_query().unwrap().as_str(),
        "/api/v1/ws?ticket=fixture"
    );
    assert_eq!(request.headers()[header::ORIGIN], "http://127.0.0.1:15174");
    assert_eq!(request.headers()[header::UPGRADE], "websocket");
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let mut stream = TokioIo::new(upgrade.await.unwrap());
        let mut received = [0_u8; 5];
        stream.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"hello");
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    Ok(Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        .body(Body::empty())
        .unwrap())
}

#[tokio::test]
async fn valid_websocket_handshake_and_duplex_bytes_use_the_same_stream() {
    timeout(Duration::from_secs(5), async {
        let (proxy_out, api) = duplex(4096);
        tokio::spawn(async move {
            server::Builder::new()
                .serve_connection(TokioIo::new(api), service_fn(ws_fixture))
                .with_upgrades()
                .await
                .unwrap();
        });

        let (browser, proxy_in) = duplex(4096);
        let proxy_out = Mutex::new(Some(proxy_out));
        tokio::spawn(async move {
            let route = service_fn(|request: Request<Incoming>| {
                let stream = proxy_out.lock().unwrap().take().unwrap();
                async move {
                    let mut request = request.map(Body::new);
                    validate_target(request.method(), request.uri()).unwrap();
                    let upgrade = headers::request(&mut request).unwrap();
                    let response = relay(request, upgrade, stream).await.unwrap();
                    Ok::<_, Infallible>(response)
                }
            });
            server::Builder::new()
                .serve_connection(TokioIo::new(proxy_in), route)
                .with_upgrades()
                .await
                .unwrap();
        });

        let (mut browser, connection) = http1::handshake(TokioIo::new(browser)).await.unwrap();
        tokio::spawn(connection.with_upgrades());
        let mut response = browser
            .send_request(upgrade_request("/api/v1/ws?ticket=fixture"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        let mut browser = TokioIo::new(hyper::upgrade::on(&mut response).await.unwrap());
        browser.write_all(b"hello").await.unwrap();
        let mut received = [0_u8; 5];
        browser.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"world");
    })
    .await
    .unwrap();
}
