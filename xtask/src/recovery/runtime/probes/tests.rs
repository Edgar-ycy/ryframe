use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn ready() -> Value {
    json!({"status":"ready","mysql":"up","redis":"optional_degraded","object_storage":"not_required"})
}

fn server(
    status: u16,
    mime: &str,
    body: &[u8],
    extra: &str,
) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let wire = format!(
        "HTTP/1.1 {status} Test\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let body = body.to_vec();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let _ = stream
            .write_all(wire.as_bytes())
            .and_then(|()| stream.write_all(&body));
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}"), handle)
}

#[test]
fn strict_readiness_preserves_degraded_and_rejects_duplicates_and_invalid_dependencies() {
    assert_eq!(
        readiness(&serde_json::to_vec(&ready()).unwrap()).unwrap(),
        ready()
    );
    for raw in [
        b"{}".as_slice(),
        b"null",
        b"{\"status\":NaN}",
        b"\xff",
        br#"{"status":"ready","status":"ready","mysql":"up","redis":"up","object_storage":"up"}"#,
    ] {
        assert!(readiness(raw).is_err());
    }
    for (field, value) in [
        ("status", "pending"),
        ("mysql", "down"),
        ("redis", "down"),
        ("object_storage", "down"),
        ("extra", "up"),
    ] {
        let mut candidate = ready();
        candidate[field] = json!(value);
        assert!(readiness(&serde_json::to_vec(&candidate).unwrap()).is_err());
    }
}

#[test]
fn request_preflight_rejects_credentials_unknown_fields_and_incomplete_or_unsafe_files() {
    for url in [
        "file:///tmp/x",
        "http://secret:password@localhost/readyz",
        "http://localhost/?x=secret",
        "http://localhost/#secret",
    ] {
        let error = request(&json!({"role":"api","url":url}))
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
    }
    assert!(request(&json!({"role":"api","url":"http://localhost","write":true})).is_err());
    assert!(request(&json!({"role":"other","url":"http://localhost"})).is_err());
    for path in ["../secret", "/absolute", "a\\b", "a//b", "a/./b"] {
        assert!(
            request(
                &json!({"role":"frontend","url":"http://localhost","files":[{
                    "path":path,"bytes":0,"sha256":"a".repeat(64)
                }]})
            )
            .is_err()
        );
    }
    assert!(request(&json!({"role":"frontend","url":"http://localhost","files":[]})).is_err());
}

#[test]
fn api_transport_validates_mime_status_and_bounded_body_without_redirecting() {
    let body = serde_json::to_vec(&ready()).unwrap();
    let (url, handle) = server(200, "application/json; charset=utf-8", &body, "");
    assert_eq!(request(&json!({"role":"api","url":url})).unwrap(), ready());
    let headers = handle.join().unwrap().to_lowercase();
    assert!(headers.contains("accept: application/json\r\n"));
    assert!(headers.contains("accept-encoding: identity\r\n"));
    for (status, mime, body, extra) in [
        (200, "text/plain", body.clone(), ""),
        (
            302,
            "application/json",
            Vec::new(),
            "Location: http://127.0.0.1:1/secret\r\n",
        ),
        (503, "application/json", body, ""),
        (200, "application/json", vec![b' '; 64 * 1024 + 1], ""),
    ] {
        let (url, handle) = server(status, mime, &body, extra);
        let error = request(&json!({"role":"api","url":url}))
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
        handle.join().unwrap();
    }
}

#[test]
fn worker_requires_an_empty_success_and_never_accepts_redirect_or_unavailable_endpoint() {
    for (body, valid) in [(b"".as_slice(), true), (b"unexpected".as_slice(), false)] {
        let (url, handle) = server(200, "text/plain", body, "");
        assert_eq!(request(&json!({"role":"worker","url":url})).is_ok(), valid);
        handle.join().unwrap();
    }
    let (url, handle) = server(302, "text/plain", b"", "Location: http://127.0.0.1:1/\r\n");
    assert!(
        request(&json!({"role":"worker","url":url}))
            .unwrap_err()
            .to_string()
            .contains("重定向")
    );
    handle.join().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(request(&json!({"role":"worker","url":url})).is_err());
}

fn files(body: &[u8]) -> Value {
    json!([
        {"path":"index.html","bytes":body.len(),"sha256":hex::encode(Sha256::digest(body))},
        {"path":".vite/manifest.json","bytes":2,"sha256":hex::encode(Sha256::digest(b"{}"))}
    ])
}

#[test]
fn frontend_verifies_exact_bytes_and_hash_and_skips_private_vite_manifest() {
    let body = b"<html>\x00\xff</html>";
    let (url, handle) = server(200, "text/html", body, "");
    request(&json!({"role":"frontend","url":url,"files":files(body)})).unwrap();
    assert!(handle.join().unwrap().starts_with("GET /index.html "));
    for (expected, actual) in [
        (b"abc".as_slice(), b"def".as_slice()),
        (b"ab", b"a"),
        (b"ab", b"abc"),
    ] {
        let (url, handle) = server(200, "text/html", actual, "");
        assert!(request(&json!({"role":"frontend","url":url,"files":files(expected)})).is_err());
        handle.join().unwrap();
    }
}

#[test]
fn frontend_encodes_reserved_characters_and_rejects_duplicate_files_before_network() {
    let body = b"index";
    let mut entries = files(body);
    entries
        .as_array_mut()
        .unwrap()
        .push(json!({"path":"index.html","bytes":5,"sha256":"a".repeat(64)}));
    assert!(request(&json!({"role":"frontend","url":"http://localhost","files":entries})).is_err());
    let mut base = Url::parse("http://localhost").unwrap();
    base.path_segments_mut()
        .unwrap()
        .clear()
        .extend(["a#?%", "中文.js"]);
    assert_eq!(
        base.as_str(),
        "http://localhost/a%23%3F%25/%E4%B8%AD%E6%96%87.js"
    );
}

#[test]
fn cancellation_drops_the_inflight_connection_and_never_returns_success() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sent, received) = tokio::sync::oneshot::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        sent.send(()).unwrap();
        stream.read(&mut byte)
    });
    crate::http_session::install_crypto_provider().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(controlled(&json!({"role":"worker","url":url}), async {
            received.await.unwrap();
        }))
        .unwrap_err();
    assert!(error.to_string().contains("已取消"));
    drop(runtime);
    // Windows 可能报告连接重置；EOF 与重置均表示没有继续读取或遗留连接。
    match handle.join().unwrap() {
        Ok(count) => assert_eq!(count, 0),
        Err(error) => assert!(matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::NotConnected
        )),
    }
}
