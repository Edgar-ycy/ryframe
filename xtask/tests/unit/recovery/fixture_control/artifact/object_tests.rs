use super::{EvidenceObjects, objects::Objects};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

fn server(responses: Vec<Vec<u8>>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(10);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("S3 请求未到达：{error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 4096];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
                assert!(request.len() < 32768);
            }
            requests.push(String::from_utf8(request).unwrap());
            stream.write_all(&response).unwrap();
        }
        requests
    });
    (endpoint, handle)
}
fn objects(root: &std::path::Path, endpoint: &str) -> Objects {
    let config: toml::Table = toml::from_str(&format!("[object_storage]\nbackend='s3'\nendpoint='{endpoint}'\nregion='us-east-1'\naccess_key='fixture-access'\nsecret_key='fixture-secret'\n")).unwrap();
    Objects::new(root, root, &config, "artifact-test").unwrap()
}
fn response(length: usize, body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {length}\r\nContent-Type: application/octet-stream\r\nETag: \"fixture-version\"\r\nLast-Modified: Thu, 22 Oct 2026 01:02:03 GMT\r\n\r\n").into_bytes();
    response.extend_from_slice(body);
    response
}

#[test]
fn artifact_s3_exact_get_signs_request_and_binds_head_version() {
    let root = tempfile::tempdir().unwrap();
    let (endpoint, server) = server(vec![response(4, b""), response(4, b"data")]);
    assert_eq!(
        objects(root.path(), &endpoint).bytes("中文.xlsx").unwrap(),
        Some(b"data".to_vec())
    );
    let requests = server.join().unwrap();
    assert!(
        requests[0].starts_with("HEAD /exports/artifact-test/%E4%B8%AD%E6%96%87.xlsx HTTP/1.1")
    );
    assert!(requests[1].contains("if-match: \"fixture-version\""));
    assert!(requests.iter().all(
        |request| request.contains("authorization: AWS4-HMAC-SHA256")
            && !request.contains("fixture-secret")
    ));
}

#[test]
fn artifact_s3_only_explicit_404_means_missing_and_denial_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    for (status, missing) in [(404, true), (403, false), (503, false)] {
        let (endpoint, server) = server(vec![
            format!("HTTP/1.1 {status} Failure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_bytes(),
        ]);
        let value = objects(root.path(), &endpoint).bytes("result.xlsx");
        if missing {
            assert_eq!(value.unwrap(), None);
        } else {
            assert!(value.is_err());
        }
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[test]
fn artifact_s3_interrupted_get_preserves_partial_evidence_without_retry() {
    let root = tempfile::tempdir().unwrap();
    let (endpoint, server) = server(vec![response(9, b""), response(9, b"partial")]);
    let result = objects(root.path(), &endpoint).bytes("result.xlsx");
    assert!(result.unwrap_err().to_string().contains("证据保留"));
    assert_eq!(server.join().unwrap().len(), 2);
    let retained = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 1);
    assert_eq!(fs::read(retained[0].join("object")).unwrap(), b"partial");
}
