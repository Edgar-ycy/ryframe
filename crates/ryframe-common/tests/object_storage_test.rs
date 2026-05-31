/// object_storage 模块测试
/// 从 crates/ryframe-common/src/utils/object_storage.rs 内联测试迁移
use ryframe_common::utils::object_storage::{
    LocalObjectStorage, MinioConfig, MinioStorage, ObjectStorage,
};
use tempfile::TempDir;

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

    let url = storage.public_url("photos", "images/photo.jpg");
    assert_eq!(url, "http://example.com/uploads/photos/images/photo.jpg");
}

#[test]
fn test_minio_config() {
    let config = MinioConfig {
        endpoint: "http://localhost:9000".into(),
        access_key: "minioadmin".into(),
        secret_key: "minioadmin".into(),
        use_ssl: false,
    };

    let storage = MinioStorage::new(config).unwrap();
    assert!(storage.endpoint().contains("localhost"));
}

/// MinIO/S3 连通性集成测试
/// 运行方式: cargo test -p ryframe-common test_s3_integration -- --ignored
#[tokio::test]
#[ignore = "需要 MinIO/S3 服务运行在 localhost:9000"]
async fn test_s3_integration_put_get_delete() {
    let config = MinioConfig {
        endpoint: "http://localhost:9000".into(),
        access_key: "rustfsadmin".into(),
        secret_key: "rustfsadmin".into(),
        use_ssl: false,
    };

    let bucket = "ryframe";

    // 创建 MinIO 客户端
    let storage = match MinioStorage::new(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("MinIO 连接失败: {}", e);
            return;
        }
    };
    println!("MinIO 端点: {}", storage.endpoint());

    // 确保 bucket 存在（不存在则自动创建）
    match storage.ensure_bucket(bucket).await {
        Ok(()) => println!("✅ Bucket 就绪: {}", bucket),
        Err(e) => panic!("Bucket 初始化失败: {}", e),
    }

    let test_key = "test/connectivity_test.txt";
    let test_data = b"hello ryframe s3 storage!";

    // 1. PUT (上传)
    match storage.put(bucket, test_key, test_data, "text/plain").await {
        Ok(()) => println!("✅ PUT 成功: {}", test_key),
        Err(e) => panic!("PUT 失败: {}", e),
    }

    // 2. EXISTS (检查存在)
    match storage.exists(bucket, test_key).await {
        Ok(true) => println!("✅ EXISTS 成功: 对象存在"),
        Ok(false) => panic!("EXISTS 失败: 对象不存在（刚上传的）"),
        Err(e) => panic!("EXISTS 失败: {}", e),
    }

    // 3. GET (下载)
    match storage.get(bucket, test_key).await {
        Ok(data) => {
            println!("✅ GET 成功: 读取到 {} bytes", data.len());
            assert_eq!(data, test_data, "下载数据与上传数据不一致");
        }
        Err(e) => panic!("GET 失败: {}", e),
    }

    // 4. PUBLIC_URL
    let url = storage.public_url(bucket, test_key);
    println!("✅ PUBLIC_URL: {}", url);
    assert!(url.contains(bucket));
    assert!(url.contains(test_key.trim_start_matches('/')));

    // 5. DELETE (删除)
    match storage.delete(bucket, test_key).await {
        Ok(()) => println!("✅ DELETE 成功"),
        Err(e) => panic!("DELETE 失败: {}", e),
    }

    // 6. 验证删除后不存在
    match storage.exists(bucket, test_key).await {
        Ok(false) => println!("✅ 删除后验证: 对象已不存在"),
        Ok(true) => panic!("删除后对象仍存在"),
        Err(e) => panic!("删除后 EXISTS 查询失败: {}", e),
    }

    println!("🎉 S3 对象存储连通性测试全部通过!");
}
