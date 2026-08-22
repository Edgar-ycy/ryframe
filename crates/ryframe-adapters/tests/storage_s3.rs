use ryframe_adapters::storage::{
    S3Config, S3ObjectStorage, normalize_sha256, parse_list_objects_response,
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
