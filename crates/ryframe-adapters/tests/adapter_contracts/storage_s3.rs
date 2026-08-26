use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use ryframe_adapters::{
    metrics,
    storage::{
        ObjectStorage, S3Config, S3ObjectStorage, normalize_sha256, parse_list_objects_response,
    },
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn upload_file_hashes_and_rewinds_the_same_handle() {
    let directory = tempfile::tempdir().expect("创建测试目录");
    let path = directory.path().join("artifact.xlsx");
    let content = b"streamed artifact";
    tokio::fs::write(&path, content)
        .await
        .expect("写入测试文件");

    let (mut file, length, hash) = S3ObjectStorage::prepare_upload_file(&path, None)
        .await
        .expect("准备上传文件");
    let mut reread = Vec::new();
    file.read_to_end(&mut reread).await.expect("重新读取文件");

    assert_eq!(length, content.len() as u64);
    assert_eq!(hash, hex::encode(Sha256::digest(content)));
    assert_eq!(reread, content);
}

#[test]
fn supplied_hash_is_validated_and_normalized() {
    let uppercase = "A".repeat(64);
    assert_eq!(
        normalize_sha256(&uppercase).expect("规范化哈希"),
        "a".repeat(64)
    );
    assert!(normalize_sha256("not-a-hash").is_err());
}

#[test]
fn list_request_contains_exact_prefix_cursor_and_limit() {
    let storage = S3ObjectStorage::new(S3Config {
        endpoint: "127.0.0.1:9000".to_owned(),
        access_key: "test-access".to_owned(),
        secret_key: "test-secret".to_owned(),
        use_ssl: false,
        region: "us-east-1".to_owned(),
        request_timeout_secs: 30,
    })
    .expect("创建离线 S3 客户端");
    let url = storage
        .list_url("exports", "scope/jobs/", Some("opaque+cursor"), 37)
        .expect("构造列举地址");
    let query = url.query_pairs().collect::<Vec<_>>();

    assert!(query.contains(&("list-type".into(), "2".into())));
    assert!(query.contains(&("prefix".into(), "scope/jobs/".into())));
    assert!(query.contains(&("max-keys".into(), "37".into())));
    assert!(query.contains(&("continuation-token".into(), "opaque+cursor".into())));
    assert!(storage.list_url("exports", "scope", None, 1).is_err());
    assert_eq!(storage.late_put_completion_bound(), Duration::from_secs(30));
}

#[test]
fn s3_config_redacts_credentials_and_bounds_timeout() {
    let config = S3Config {
        endpoint: "127.0.0.1:9000".to_owned(),
        access_key: "access-must-not-leak".to_owned(),
        secret_key: "secret-must-not-leak".to_owned(),
        use_ssl: false,
        region: "us-east-1".to_owned(),
        request_timeout_secs: 0,
    };
    let debug = format!("{config:?}");
    assert!(!debug.contains("access-must-not-leak"));
    assert!(!debug.contains("secret-must-not-leak"));
    assert!(debug.contains("<redacted>"));
    assert!(S3ObjectStorage::new(config).is_err());
}

#[test]
fn list_response_is_unescaped_bounded_and_prefix_checked() {
    let body = br#"<?xml version="1.0" encoding="UTF-8"?>
        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
          <IsTruncated>true</IsTruncated>
          <Contents><Key>scope/a&amp;b.txt</Key></Contents>
          <Contents><Key>scope/b.txt</Key></Contents>
          <NextContinuationToken>next&amp;token</NextContinuationToken>
        </ListBucketResult>"#;
    let page = parse_list_objects_response(body, "scope/", 2).expect("解析 S3 列举结果");
    assert_eq!(page.keys, ["scope/a&b.txt", "scope/b.txt"]);
    assert_eq!(page.next_cursor.as_deref(), Some("next&token"));

    let outside = br#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>scope-other/a.txt</Key></Contents></ListBucketResult>"#;
    assert!(parse_list_objects_response(outside, "scope/", 1).is_err());

    let too_many = br#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>scope/a.txt</Key></Contents><Contents><Key>scope/b.txt</Key></Contents></ListBucketResult>"#;
    assert!(parse_list_objects_response(too_many, "scope/", 1).is_err());
}

#[tokio::test]
async fn s3_metrics_record_each_complete_logical_operation_once() {
    let not_found =
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let truncated_body =
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nxx".to_vec();
    let invalid_list =
        b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n<broken>".to_vec();
    let (endpoint, server) = spawn_http_responses(vec![
        not_found.clone(),
        not_found,
        truncated_body,
        invalid_list,
    ]);
    let storage = S3ObjectStorage::new(S3Config {
        endpoint,
        access_key: "test-access".to_owned(),
        secret_key: "test-secret".to_owned(),
        use_ssl: false,
        region: "us-east-1".to_owned(),
        request_timeout_secs: 2,
    })
    .expect("创建 S3 指标测试客户端");
    let before = metrics::metrics_text();

    storage
        .delete("exports", "scope/missing.txt")
        .await
        .expect("删除不存在对象应保持幂等");
    assert!(
        !storage
            .exists("exports", "scope/missing.txt")
            .await
            .expect("检查不存在对象应成功")
    );
    assert!(
        storage.get("exports", "scope/truncated.txt").await.is_err(),
        "截断响应体必须使完整 GET 失败"
    );
    assert!(
        storage
            .list_page("exports", "scope/", None, 1)
            .await
            .is_err(),
        "无效列表响应必须使完整 LIST 失败"
    );
    server.join().expect("S3 指标测试服务应正常结束");

    let after = metrics::metrics_text();
    for (operation, result) in [
        ("DELETE", "success"),
        ("EXISTS", "success"),
        ("GET", "error"),
        ("LIST", "error"),
    ] {
        assert_eq!(
            operation_total(&after, operation) - operation_total(&before, operation),
            1.0,
            "{operation} 应且只应记录一次完整逻辑操作"
        );
        assert_eq!(
            operation_result_total(&after, operation, result)
                - operation_result_total(&before, operation, result),
            1.0,
            "{operation} 应按最终逻辑结果归类为 {result}"
        );
    }
}

#[tokio::test]
async fn s3_service_error_discards_remote_body_and_bucket_location() {
    let remote_body = "remote-secret-body exports scope/private.txt";
    let response = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{remote_body}",
        remote_body.len()
    )
    .into_bytes();
    let (endpoint, server) = spawn_http_responses(vec![response]);
    let storage = S3ObjectStorage::new(S3Config {
        endpoint,
        access_key: "test-access".to_owned(),
        secret_key: "test-secret".to_owned(),
        use_ssl: false,
        region: "us-east-1".to_owned(),
        request_timeout_secs: 2,
    })
    .expect("创建 S3 脱敏测试客户端");

    let error = storage
        .bucket_exists("exports")
        .await
        .expect_err("远端失败必须返回错误")
        .to_string();
    server.join().expect("S3 脱敏测试服务应正常结束");

    assert!(error.contains("HTTP 503"));
    assert!(!error.contains(remote_body));
    assert!(!error.contains("exports"));
    assert!(!error.contains("scope/private.txt"));
}

#[tokio::test]
async fn s3_transport_error_discards_request_url() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 S3 传输错误测试端口");
    let endpoint = listener
        .local_addr()
        .expect("读取 S3 传输错误测试端口")
        .to_string();
    drop(listener);
    let storage = S3ObjectStorage::new(S3Config {
        endpoint: endpoint.clone(),
        access_key: "test-access".to_owned(),
        secret_key: "test-secret".to_owned(),
        use_ssl: false,
        region: "us-east-1".to_owned(),
        request_timeout_secs: 1,
    })
    .expect("创建 S3 传输错误测试客户端");

    let error = storage
        .bucket_exists("exports")
        .await
        .expect_err("关闭端口必须返回传输错误")
        .to_string();

    assert!(!error.contains(&endpoint));
    assert!(!error.contains("exports"));
}

fn spawn_http_responses(responses: Vec<Vec<u8>>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 S3 指标测试端口");
    let endpoint = listener.local_addr().expect("读取 S3 指标测试端口");
    let server = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("接受 S3 指标测试请求");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("设置 S3 指标测试读取超时");
            let mut request = Vec::new();
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0_u8; 1024];
                let length = stream.read(&mut chunk).expect("读取 S3 指标测试请求");
                assert!(length > 0, "S3 指标测试请求头不完整");
                request.extend_from_slice(&chunk[..length]);
                assert!(request.len() <= 32 * 1024, "S3 指标测试请求头过大");
            }
            stream.write_all(&response).expect("写入 S3 指标测试响应");
        }
    });
    (endpoint.to_string(), server)
}

fn operation_total(metrics: &str, operation: &str) -> f64 {
    metrics
        .lines()
        .filter(|line| line.starts_with("ryframe_connector_operations_total{"))
        .filter(|line| line.contains("connector=\"s3\""))
        .filter(|line| line.contains(&format!("operation=\"{operation}\"")))
        .map(metric_value)
        .sum()
}

fn operation_result_total(metrics: &str, operation: &str, result: &str) -> f64 {
    metrics
        .lines()
        .find(|line| {
            line.starts_with("ryframe_connector_operations_total{")
                && line.contains("connector=\"s3\"")
                && line.contains(&format!("operation=\"{operation}\""))
                && line.contains(&format!("result=\"{result}\""))
        })
        .map_or(0.0, metric_value)
}

fn metric_value(line: &str) -> f64 {
    line.split_whitespace()
        .last()
        .expect("指标行应包含数值")
        .parse()
        .expect("指标值应为数字")
}
