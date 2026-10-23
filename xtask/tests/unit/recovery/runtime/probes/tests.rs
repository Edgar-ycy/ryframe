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

fn site_responses(responses: Vec<(&str, Vec<u8>)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let responses: Vec<_> = responses
        .into_iter()
        .map(|(path, body)| (path.to_owned(), body))
        .collect();
    let handle = thread::spawn(move || {
        let mut paths = Vec::new();
        for (expected, body) in responses {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline, "探针未请求登记资源");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("探针测试监听失败：{error}"),
                }
            };
            // Windows accept 会继承监听器的非阻塞模式；请求读取使用有界阻塞。
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            let path = request.split_whitespace().nth(1).unwrap().to_owned();
            assert_eq!(path, expected);
            paths.push(path);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream
                .write_all(header.as_bytes())
                .and_then(|()| stream.write_all(&body));
        }
        paths
    });
    (format!("http://{address}"), handle)
}

fn production_files() -> Value {
    let mut files = Vec::new();
    for (path, body) in [
        ("index.html", b"<main>runtime</main>".as_slice()),
        ("app.js", b"\x00RyFrame fixture\xff".as_slice()),
        (".vite/manifest.json", b"{}".as_slice()),
        (
            "build-identity.json",
            br#"{"frontend_commit":"fixture"}"#.as_slice(),
        ),
    ] {
        files.push(
            json!({"path":path,"bytes":body.len(),"sha256":hex::encode(Sha256::digest(body))}),
        );
    }
    json!(files)
}

#[test]
fn three_role_runtime_probes_verify_readiness_and_every_public_frontend_artifact() {
    // 对应原 Python 完整运行核验中的真实网络部分；绑定与进程复核仍由原调用方覆盖。
    let (api, api_request) = server(
        200,
        "application/json",
        &serde_json::to_vec(&ready()).unwrap(),
        "",
    );
    let (worker, worker_request) = server(200, "text/plain", b"", "");
    let (frontend, frontend_requests) = site_responses(vec![
        ("/index.html", b"<main>runtime</main>".to_vec()),
        ("/app.js", b"\x00RyFrame fixture\xff".to_vec()),
        (
            "/build-identity.json",
            br#"{"frontend_commit":"fixture"}"#.to_vec(),
        ),
    ]);
    assert_eq!(
        request(&json!({"role":"api","url":format!("{api}/readyz")})).unwrap(),
        ready()
    );
    assert_eq!(
        request(&json!({"role":"worker","url":format!("{worker}/readyz")})).unwrap(),
        json!({"status":"verified"})
    );
    assert_eq!(
        request(&json!({"role":"frontend","url":frontend,"files":production_files()})).unwrap(),
        json!({"status":"verified"})
    );
    for received in [api_request.join().unwrap(), worker_request.join().unwrap()] {
        assert!(received.starts_with("GET /readyz HTTP/1.1\r\n"));
    }
    assert_eq!(
        frontend_requests.join().unwrap(),
        ["/index.html", "/app.js", "/build-identity.json"]
    );
}

#[test]
fn later_frontend_artifact_tampering_never_passes_on_the_strength_of_the_index() {
    for actual in [
        b"\x00RyFrame fixture\xfe".as_slice(),
        b"\x00RyFrame fixture".as_slice(),
        b"\x00RyFrame fixture\xffextra".as_slice(),
    ] {
        let (url, handle) = site_responses(vec![
            ("/index.html", b"<main>runtime</main>".to_vec()),
            ("/app.js", actual.to_vec()),
        ]);
        assert!(request(&json!({"role":"frontend","url":url,"files":production_files()})).is_err());
        assert_eq!(handle.join().unwrap(), ["/index.html", "/app.js"]);
    }
    let (url, handle) = site_responses(vec![
        ("/index.html", b"<main>runtime</main>".to_vec()),
        ("/app.js", b"\x00RyFrame fixture\xff".to_vec()),
        (
            "/build-identity.json",
            br#"{"frontend_commit":"changed"}"#.to_vec(),
        ),
    ]);
    assert!(request(&json!({"role":"frontend","url":url,"files":production_files()})).is_err());
    assert_eq!(
        handle.join().unwrap(),
        ["/index.html", "/app.js", "/build-identity.json"]
    );
}
