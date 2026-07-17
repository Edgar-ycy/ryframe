use ryframe_storage::{LocalObjectStorage, ObjectStorage, S3Config, S3ObjectStorage};
use tempfile::TempDir;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[tokio::test]
async fn test_local_storage_put_get_delete() {
    let tmp = TempDir::new().unwrap();
    let storage = LocalObjectStorage::new(tmp.path(), "http://localhost:8080/uploads");

    let bucket = "my-app";

    // Put
    storage
        .put(bucket, "test/file.txt", b"hello world", "text/plain")
        .await
        .unwrap();

    // Exists
    assert!(storage.exists(bucket, "test/file.txt").await.unwrap());

    // Get
    let data = storage.get(bucket, "test/file.txt").await.unwrap();
    assert_eq!(data, b"hello world");

    // Delete
    storage.delete(bucket, "test/file.txt").await.unwrap();
    assert!(!storage.exists(bucket, "test/file.txt").await.unwrap());
}

#[tokio::test]
async fn test_local_storage_public_url() {
    let tmp = TempDir::new().unwrap();
    let storage = LocalObjectStorage::new(tmp.path(), "http://example.com/uploads");

    let url = storage.public_url("photos", "images/photo.jpg").unwrap();
    assert_eq!(
        url.as_deref(),
        Some("http://example.com/uploads/photos/images/photo.jpg")
    );
}

#[test]
fn test_minio_config() {
    let config = S3Config {
        endpoint: "http://localhost:9000".into(),
        access_key: "minioadmin".into(),
        secret_key: "minioadmin".into(),
        use_ssl: false,
        region: "us-east-1".into(),
        public_base_url: None,
    };

    let storage = S3ObjectStorage::new(config).unwrap();
    assert!(storage.endpoint().contains("localhost"));
}

/// RustFS/S3 连通性集成测试
/// 运行方式: cargo test -p ryframe-storage test_s3_integration -- --ignored
#[tokio::test]
#[ignore = "需要 RustFS/S3 服务运行在 localhost:9000"]
async fn test_s3_integration_put_get_delete() {
    let endpoint = env_or("APP_OBJECT_STORAGE_ENDPOINT", "http://localhost:9000");
    let use_ssl = std::env::var("APP_OBJECT_STORAGE_USE_SSL")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or_else(|_| endpoint.starts_with("https://"));
    let config = S3Config {
        endpoint: endpoint.clone(),
        access_key: env_or("APP_OBJECT_STORAGE_ACCESS_KEY", "rustfsadmin"),
        secret_key: env_or("APP_OBJECT_STORAGE_SECRET_KEY", "rustfsadmin"),
        use_ssl,
        region: env_or("APP_OBJECT_STORAGE_REGION", "us-east-1"),
        public_base_url: Some(env_or("APP_OBJECT_STORAGE_PUBLIC_BASE_URL", &endpoint)),
    };

    let bucket = "ryframe";

    let storage = S3ObjectStorage::new(config).expect("create RustFS client");

    // 确保 bucket 存在（不存在则自动创建）
    storage.ensure_bucket(bucket).await.expect("ensure bucket");

    let test_key = "test/connectivity_test.txt";
    let test_data = b"hello ryframe s3 storage!";

    // 1. PUT (上传)
    storage
        .put(bucket, test_key, test_data, "text/plain")
        .await
        .expect("put object");

    // 2. EXISTS (检查存在)
    assert!(
        storage
            .exists(bucket, test_key)
            .await
            .expect("check object")
    );

    // 3. GET (下载)
    let data = storage.get(bucket, test_key).await.expect("get object");
    assert_eq!(data, test_data, "下载数据与上传数据不一致");

    // 4. PUBLIC_URL
    let url = storage.public_url(bucket, test_key).unwrap().unwrap();
    assert!(url.contains(bucket));
    assert!(url.contains(test_key.trim_start_matches('/')));

    // 5. DELETE (删除)
    storage
        .delete(bucket, test_key)
        .await
        .expect("delete object");

    // 6. 验证删除后不存在
    assert!(
        !storage
            .exists(bucket, test_key)
            .await
            .expect("check deletion")
    );
}
