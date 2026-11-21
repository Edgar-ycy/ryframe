use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::{Configuration, Operation, PutRequest, Request, response};

fn execute(request: Request) -> crate::Result<serde_json::Value> {
    crate::http_session::install_crypto_provider()?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(super::client::execute(
            request,
            std::time::Duration::from_secs(3),
        ))
}

fn config(endpoint: &str) -> Configuration {
    Configuration::read(&json!({"endpoint": endpoint,"region":"us-east-1", "access_key":"fixture-access", "secret_key":"fixture-secret"})).unwrap()
}

fn server(response: Vec<u8>) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    listener.set_nonblocking(true).unwrap();
    let task = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(error) => panic!("模拟 S3 未在期限内收到连接：{error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
            assert!(bytes.len() < 32768);
        }
        tx.send(String::from_utf8(bytes).unwrap()).unwrap();
        let _ = socket.write_all(&response);
    });
    (endpoint, rx, task)
}

fn object_response(length: u64, body: &[u8]) -> Vec<u8> {
    let mut result = format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {length}\r\nContent-Type: application/octet-stream\r\nETag: \"fixture\"\r\nLast-Modified: Thu, 22 Oct 2026 01:02:03 GMT\r\nx-amz-meta-custom: raw-value\r\n\r\n").into_bytes();
    result.extend_from_slice(body);
    result
}

fn put_server(
    response: Vec<u8>,
    after_read: impl FnOnce() + Send + 'static,
) -> (String, mpsc::Receiver<Vec<u8>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    listener.set_nonblocking(true).unwrap();
    let task = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(value) => break value,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("模拟 S3 PUT 未在期限内收到连接：{error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        let (header_end, content_length) = loop {
            let mut chunk = [0_u8; 4096];
            let count = socket.read(&mut chunk).unwrap();
            assert_ne!(count, 0, "模拟 S3 在请求头完成前断开");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let end = index + 4;
                let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::parse::<usize>)
                    })
                    .unwrap()
                    .unwrap();
                break (end, length);
            }
            assert!(bytes.len() < 64 * 1024);
        };
        while bytes.len() - header_end < content_length {
            let mut chunk = [0_u8; 64 * 1024];
            let count = socket.read(&mut chunk).unwrap();
            assert_ne!(count, 0, "模拟 S3 在请求体完成前断开");
            bytes.extend_from_slice(&chunk[..count]);
        }
        assert_eq!(bytes.len() - header_end, content_length);
        after_read();
        tx.send(bytes).unwrap();
        let _ = socket.write_all(&response);
    });
    (endpoint, rx, task)
}

fn metadata() -> serde_json::Value {
    json!({
        "ContentType": "text/plain; charset=utf-8",
        "CacheControl": "private, max-age=60",
        "ContentDisposition": "attachment; filename=report.txt",
        "ContentEncoding": "gzip",
        "ContentLanguage": "zh-CN",
        "Expires": "2030-01-01T08:00:00+08:00",
        "WebsiteRedirectLocation": "/report",
        "Metadata": {"empty": "", "purpose": "fixture"},
    })
}

fn put_request(endpoint: &str, source: &std::path::Path) -> PutRequest {
    let bytes = std::fs::read(source).unwrap();
    PutRequest {
        configuration: config(endpoint),
        bucket: "uploads".into(),
        key: "target/system/中文 report.txt".into(),
        source: source.to_path_buf(),
        expected_bytes: bytes.len() as u64,
        expected_sha256: hex::encode(Sha256::digest(&bytes)),
        metadata: metadata(),
    }
}

#[test]
fn native_get_signs_conditions_and_streams_exact_bytes() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let destination = temp.path().join("object.bin");
    let body = vec![42; 131_072];
    let (endpoint, received, task) = server(object_response(body.len() as u64, &body));
    let result = execute(Request {
        configuration: config(&endpoint),
        bucket: "uploads".into(),
        operation: Operation::Get {
            key: "scope/中文 +%//key".into(),
            if_match: Some("\"fixture\"".into()),
            version: Some("a+/=".into()),
            destination: destination.clone(),
            max_bytes: body.len() as u64,
        },
    })
    .unwrap();
    task.join().unwrap();
    let request = received.recv().unwrap();
    assert!(request.starts_with(
        "GET /uploads/scope/%E4%B8%AD%E6%96%87%20%2B%25//key?versionId=a%2B%2F%3D HTTP/1.1"
    ));
    assert!(request.contains("authorization: AWS4-HMAC-SHA256 Credential=fixture-access/"));
    assert!(request.contains("SignedHeaders=host;if-match;x-amz-content-sha256;x-amz-date"));
    assert!(!request.contains("fixture-secret"));
    assert_eq!(std::fs::read(destination).unwrap(), body);
    assert_eq!(result["Metadata"]["custom"], "raw-value");
    assert_eq!(result["LastModified"], "2026-10-22T01:02:03+00:00");
}

#[test]
fn head_not_found_returns_closed_structured_outcome() {
    let response =
        b"HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 14\r\n\r\nfixture-secret"
            .to_vec();
    let (endpoint, received, task) = server(response);
    let result = execute(Request {
        configuration: config(&endpoint),
        bucket: "uploads".into(),
        operation: Operation::Head {
            key: "scope/missing.bin".into(),
            if_match: None,
            version: None,
        },
    })
    .unwrap();
    task.join().unwrap();
    let request = received.recv().unwrap();
    assert!(request.starts_with("HEAD /uploads/scope/missing.bin HTTP/1.1"));
    assert_eq!(
        result,
        json!({
            "status": "not_found",
            "http_status": 404,
            "remote_write_may_have_occurred": false,
        })
    );
    assert!(!result.to_string().contains("fixture-secret"));
}

#[test]
fn create_bucket_signs_put_and_reports_unverified_write() {
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec();
    let (endpoint, received, task) = server(response);
    let result = super::create_bucket(config(&endpoint), "uploads".into()).unwrap();
    task.join().unwrap();
    let request = received.recv().unwrap();
    assert!(request.starts_with("PUT /uploads HTTP/1.1"));
    assert!(request.contains("authorization: AWS4-HMAC-SHA256 Credential=fixture-access/"));
    assert!(!request.contains("fixture-secret"));
    assert_eq!(result["status"], "created_unverified");
    assert_eq!(result["http_status"], 200);
    assert_eq!(result["remote_write_may_have_occurred"], true);
    assert_eq!(result["automatic_retry_permitted"], false);
}

#[test]
fn truncated_get_preserves_partial_file_and_reports_failure() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let destination = temp.path().join("partial.bin");
    let (endpoint, received, task) = server(object_response(100, b"short"));
    let error = execute(Request {
        configuration: config(&endpoint),
        bucket: "uploads".into(),
        operation: Operation::Get {
            key: "scope/key".into(),
            if_match: None,
            version: None,
            destination: destination.clone(),
            max_bytes: 100,
        },
    })
    .unwrap_err();
    task.join().unwrap();
    received.recv().unwrap();
    assert!(error.to_string().contains("中断") || error.to_string().contains("不足"));
    assert!(destination.exists());
    assert!(std::fs::metadata(destination).unwrap().len() < 100);
}

#[test]
fn get_rejects_declared_limit_before_creating_output() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let destination = temp.path().join("oversize.bin");
    let (endpoint, received, task) = server(object_response(4, b"data"));
    let error = execute(Request {
        configuration: config(&endpoint),
        bucket: "uploads".into(),
        operation: Operation::Get {
            key: "scope/key".into(),
            if_match: None,
            version: None,
            destination: destination.clone(),
            max_bytes: 3,
        },
    })
    .unwrap_err();
    task.join().unwrap();
    received.recv().unwrap();
    assert!(error.to_string().contains("上限"));
    assert!(!destination.exists());
}

#[test]
fn redirects_and_service_errors_do_not_expose_credentials_or_follow_location() {
    for code in [206, 307, 403, 500] {
        let reply = format!("HTTP/1.1 {code} Failed\r\nLocation: http://127.0.0.1:1/fixture-secret\r\nContent-Length: 14\r\nConnection: close\r\n\r\nfixture-secret").into_bytes();
        let (endpoint, received, task) = server(reply);
        let error = execute(Request {
            configuration: config(&endpoint),
            bucket: "uploads".into(),
            operation: Operation::HeadBucket,
        })
        .unwrap_err()
        .to_string();
        task.join().unwrap();
        received.recv().unwrap();
        assert!(error.contains(&code.to_string()));
        assert!(!error.contains("fixture-secret"));
        assert!(!error.contains("fixture-access"));
    }
}

#[test]
fn configuration_and_paths_reject_ambiguous_identity() {
    for endpoint in [
        "http://user:password@localhost",
        "http://localhost/path",
        "http://localhost?key=value",
        "file:///path",
    ] {
        assert!(
            Configuration::read(
                &json!({"endpoint":endpoint,"region":"us-east-1","access_key":"a","secret_key":"b"})
            )
            .is_err()
        );
    }
    for key in ["scope/../key", "scope/./key"] {
        let request = Request {
            configuration: config("http://localhost:9000"),
            bucket: "uploads".into(),
            operation: Operation::Head {
                key: key.into(),
                if_match: None,
                version: None,
            },
        };
        assert!(request.prepare().is_err());
    }
    let request = Request {
        configuration: config("http://localhost:9000"),
        bucket: "uploads".into(),
        operation: Operation::Head {
            key: "scope/%2E//key".into(),
            if_match: None,
            version: None,
        },
    };
    assert_eq!(
        request.prepare().unwrap().1.path(),
        "/uploads/scope/%252E//key"
    );
}

#[test]
fn xml_listing_requires_complete_state_and_decodes_key_without_changing_token() {
    let operation = Operation::List {
        prefix: "scope/".into(),
        continuation: None,
    };
    let value = response::xml(br#"<ListBucketResult><IsTruncated>true</IsTruncated><EncodingType>url</EncodingType><Contents><Key>scope/%E4%B8%AD%2B%25</Key></Contents><NextContinuationToken>a+%2F&amp;=</NextContinuationToken></ListBucketResult>"#, &operation).unwrap();
    assert_eq!(value["Contents"][0]["Key"], "scope/中+%");
    assert_eq!(value["NextContinuationToken"], "a+%2F&=");
    for body in [
        "<ListBucketResult><EncodingType>url</EncodingType></ListBucketResult>",
        "<ListBucketResult><IsTruncated>false</IsTruncated><EncodingType>url</EncodingType><Contents><Key>%xx</Key></Contents></ListBucketResult>",
        "<!DOCTYPE x><ListBucketResult/>",
    ] {
        assert!(response::xml(body.as_bytes(), &operation).is_err());
    }
}

#[test]
fn versioning_and_duplicate_identity_headers_fail_closed() {
    assert_eq!(
        response::xml(b"<VersioningConfiguration/>", &Operation::Versioning).unwrap(),
        json!({})
    );
    assert_eq!(
        response::xml(
            b"<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>",
            &Operation::Versioning
        )
        .unwrap()["Status"],
        "Suspended"
    );
    let mut headers = reqwest::header::HeaderMap::new();
    headers.append("etag", "a".parse().unwrap());
    headers.append("etag", "b".parse().unwrap());
    assert!(response::metadata(&headers).is_err());
    headers.clear();
    headers.insert(
        "x-amz-new-policy",
        "sensitive-policy-value".parse().unwrap(),
    );
    let error = response::metadata(&headers).unwrap_err().to_string();
    assert!(error.contains("未知对象策略"));
    assert!(!error.contains("sensitive-policy-value"));
}

#[test]
fn conditional_put_streams_exact_bytes_and_preserves_metadata() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let source = temp.path().join("source.bin");
    let body = vec![42_u8; 196_733];
    std::fs::write(&source, &body).unwrap();
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\nETag: \"created\"\r\nx-amz-version-id: v1\r\n\r\n".to_vec();
    let (endpoint, received, task) = put_server(response, || {});
    let result = super::put_if_absent(put_request(&endpoint, &source)).unwrap();
    let request = received.recv().unwrap();
    task.join().unwrap();
    let end = request
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap()
        + 4;
    let headers = String::from_utf8(request[..end].to_vec()).unwrap();
    assert!(
        headers.starts_with("PUT /uploads/target/system/%E4%B8%AD%E6%96%87%20report.txt HTTP/1.1")
    );
    assert!(headers.contains("if-none-match: *"));
    assert!(headers.contains("x-amz-meta-purpose: fixture"));
    assert!(headers.contains("x-amz-website-redirect-location: /report"));
    assert!(headers.contains("expires: Tue, 01 Jan 2030 00:00:00 GMT"));
    assert!(headers.contains(&format!(
        "x-amz-content-sha256: {}",
        hex::encode(Sha256::digest(&body))
    )));
    assert!(headers.contains("SignedHeaders="));
    assert!(!headers.contains("fixture-secret"));
    assert_eq!(&request[end..], body);
    assert_eq!(result["status"], "created_unverified");
    assert_eq!(result["response"]["ETag"], "\"created\"");
    assert_eq!(result["response"]["VersionId"], "v1");
    assert_eq!(result["automatic_retry_permitted"], false);
}

#[test]
fn conditional_put_classifies_known_and_unknown_outcomes_without_retry() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let source = temp.path().join("source.bin");
    std::fs::write(&source, b"fixture").unwrap();
    for (code, expected, may_have_written) in [
        (412, "already_exists", false),
        (403, "denied", false),
        (409, "needs_reconciliation", true),
        (500, "needs_reconciliation", true),
    ] {
        let response =
            format!("HTTP/1.1 {code} Result\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .into_bytes();
        let (endpoint, received, task) = put_server(response, || {});
        let result = super::put_if_absent(put_request(&endpoint, &source)).unwrap();
        received.recv().unwrap();
        task.join().unwrap();
        assert_eq!(result["status"], expected);
        assert_eq!(result["remote_write_may_have_occurred"], may_have_written);
        assert_eq!(result["automatic_retry_permitted"], false);
    }
}

#[test]
fn incomplete_success_and_changed_source_require_reconciliation() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let source = temp.path().join("source.bin");
    std::fs::write(&source, b"fixture").unwrap();
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_vec();
    let (endpoint, received, task) = put_server(response, || {});
    let result = super::put_if_absent(put_request(&endpoint, &source)).unwrap();
    received.recv().unwrap();
    task.join().unwrap();
    assert_eq!(result["status"], "needs_reconciliation");
    assert_eq!(result["reason"], "success_response_incomplete");

    let changed = source.clone();
    let response =
        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\nETag: \"created\"\r\n\r\n"
            .to_vec();
    let (endpoint, received, task) = put_server(response, move || {
        std::fs::write(changed, b"changed").unwrap();
    });
    let result = super::put_if_absent(put_request(&endpoint, &source)).unwrap();
    received.recv().unwrap();
    task.join().unwrap();
    assert_eq!(result["status"], "needs_reconciliation");
    assert_eq!(result["reason"], "source_changed");
}

#[test]
fn invalid_put_input_fails_before_network_and_does_not_echo_credentials() {
    let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let source = temp.path().join("source.bin");
    std::fs::write(&source, b"fixture").unwrap();
    let mut invalid = put_request("http://127.0.0.1:1", &source);
    invalid.expected_sha256 = "private-secret".into();
    let error = super::put_if_absent(invalid).unwrap_err().to_string();
    assert!(error.contains("SHA256"));
    assert!(!error.contains("private-secret"));

    let mut invalid = put_request("http://127.0.0.1:1", &source);
    invalid.metadata["Metadata"] = json!({"Bad-Name": "private-secret"});
    let error = super::put_if_absent(invalid).unwrap_err().to_string();
    assert!(error.contains("Metadata"));
    assert!(!error.contains("private-secret"));
}

#[test]
fn private_put_protocol_rejects_old_credentials_and_scope_escape_before_network() {
    let base = json!({
        "format_version": 1,
        "kind": "ryframe-native-s3-put-if-absent",
        "scope_id": "target-scope",
        "configuration_binding": null,
        "bucket": "uploads",
        "key": "target-scope/file.txt",
        "source": "relative.txt",
        "expected": {"bytes": 0, "sha256": "0".repeat(64)},
        "metadata": metadata()
    });
    let mut legacy = base.clone();
    legacy["configuration"] = json!({
        "endpoint": "http://127.0.0.1:1",
        "region": "us-east-1",
        "access_key_env": "OLD_ACCESS",
        "secret_key_env": "OLD_SECRET"
    });
    let error = super::protocol::execute_request(legacy)
        .unwrap_err()
        .to_string();
    assert!(error.contains("字段"));

    let mut escaped = base;
    escaped["key"] = json!("other-scope/file.txt");
    let error = super::protocol::execute_request(escaped)
        .unwrap_err()
        .to_string();
    assert!(error.contains("scope"));
}

#[test]
fn private_read_protocol_rejects_scope_escape_and_old_credentials_before_network() {
    let base = json!({
        "format_version": 1,
        "kind": "ryframe-native-s3-read",
        "scope_id": "source-scope",
        "configuration_binding": null,
        "bucket": "uploads",
        "operation": "head-object",
        "key": "source-scope/file.txt",
        "if_match": null,
        "version": null,
        "destination": null,
        "max_bytes": 0
    });
    let mut escaped = base.clone();
    escaped["key"] = json!("other-scope/file.txt");
    let error = super::protocol::execute_read_request(escaped)
        .unwrap_err()
        .to_string();
    assert!(error.contains("scope"));

    let mut legacy = base;
    legacy["configuration"] = json!({
        "endpoint": "http://127.0.0.1:1",
        "region": "us-east-1",
        "access_key_env": "OLD_ACCESS",
        "secret_key_env": "OLD_SECRET"
    });
    let error = super::protocol::execute_read_request(legacy)
        .unwrap_err()
        .to_string();
    assert!(error.contains("字段"));
}

#[test]
fn private_inspect_protocol_restricts_operation_prefix_and_credentials_before_network() {
    let base = json!({
        "format_version": 1,
        "kind": "ryframe-native-s3-inspect",
        "scope_id": "source-scope",
        "configuration_binding": null,
        "bucket": "uploads",
        "operation": "list-objects-v2",
        "prefix": "other-scope/",
        "continuation": null
    });
    let error = super::protocol::execute_inspect_request(base.clone())
        .unwrap_err()
        .to_string();
    assert!(error.contains("scope"));

    let mut legacy = base;
    legacy["prefix"] = json!("source-scope/");
    legacy["configuration"] = json!({
        "endpoint": "http://127.0.0.1:1",
        "region": "us-east-1",
        "access_key_env": "OLD_ACCESS",
        "secret_key_env": "OLD_SECRET"
    });
    let error = super::protocol::execute_inspect_request(legacy)
        .unwrap_err()
        .to_string();
    assert!(error.contains("字段"));
}
