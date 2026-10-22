#[path = "support/configuration.rs"]
mod configuration;
use configuration::{Configuration, ScopedClient};
use ryframe_config::StorageBackend;

use ryframe_adapters::storage::{ObjectStorage, S3Config, S3ObjectStorage, StorageError};

#[tokio::test]
async fn s3_client_reaches_a_real_https_endpoint() {
    if !configuration::enabled("s3-https") {
        return;
    }

    let snapshot = Configuration::load("s3-https", "s3-client-https");
    let config = &snapshot.app.object_storage;
    assert_eq!(
        config.backend,
        StorageBackend::S3,
        "测试要求 object_storage.backend=s3"
    );
    assert!(
        config.use_ssl && config.endpoint.starts_with("https://"),
        "S3 测试必须配置 HTTPS，不能降级"
    );
    let storage = S3ObjectStorage::new(S3Config {
        endpoint: config.endpoint.clone(),
        access_key: config.access_key.clone(),
        secret_key: config.secret_key.clone(),
        use_ssl: config.use_ssl,
        root_ca_pem: Some(snapshot.ca().unwrap_or_else(|error| panic!("{error}"))),
        region: config.region.clone(),
        request_timeout_secs: config.request_timeout_secs,
    })
    .unwrap_or_else(|error| panic!("创建 S3 HTTPS 客户端失败: {error}"));
    let storage = ScopedClient {
        client: storage,
        configuration: snapshot,
    };
    assert!(
        storage.endpoint().starts_with("https://"),
        "S3 HTTPS 测试端点不得降级为明文 HTTP"
    );

    match storage
        .get("aws-publicdatasets", "ryframe-aws-lc-https")
        .await
    {
        Ok(_) | Err(StorageError::Service { .. }) => {}
        Err(StorageError::Transport(error)) => {
            panic!(
                "S3 HTTPS 未获得真实 HTTP 响应: {error}; cause={}",
                source_chain(&error)
            )
        }
        Err(error) => panic!("S3 HTTPS 返回了非传输、非服务响应错误: {error}"),
    }
}

fn source_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut messages = Vec::new();
    let mut source = error.source();
    while let Some(error) = source {
        messages.push(error.to_string());
        source = error.source();
    }
    messages.join(" -> ")
}
