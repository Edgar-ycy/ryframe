use std::env;

use ryframe_adapters::storage::{ObjectStorage, S3Config, S3ObjectStorage, StorageError};

const ENABLE_ENV: &str = "RYFRAME_S3_HTTPS_INTEGRATION";

#[tokio::test]
async fn s3_client_reaches_a_real_https_endpoint() {
    if !integration_enabled() {
        eprintln!("跳过 S3 HTTPS 真实链路测试：设置 {ENABLE_ENV}=1 后才会访问显式端点");
        return;
    }

    let storage = S3ObjectStorage::new(S3Config {
        endpoint: required_env("RYFRAME_S3_HTTPS_ENDPOINT"),
        access_key: "ryframe-integration-access".to_owned(),
        secret_key: "ryframe-integration-secret".to_owned(),
        use_ssl: true,
        root_ca_pem: env::var("RYFRAME_HTTPS_CA_PEM")
            .ok()
            .map(|path| std::fs::read(path).expect("读取 S3 HTTPS 测试 CA")),
        region: env_value("RYFRAME_S3_HTTPS_REGION", "us-east-1"),
        request_timeout_secs: 15,
    })
    .unwrap_or_else(|error| panic!("创建 S3 HTTPS 客户端失败: {error}"));
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

fn integration_enabled() -> bool {
    match env::var(ENABLE_ENV) {
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("{ENABLE_ENV} 只接受精确值 1，当前值为 {value:?}"),
        Err(env::VarError::NotPresent) => false,
        Err(env::VarError::NotUnicode(_)) => panic!("{ENABLE_ENV} 必须是有效 UTF-8"),
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn required_env(name: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => panic!("{name} 不得为空"),
        Err(env::VarError::NotPresent) => panic!("启用测试时必须设置 {name}"),
        Err(env::VarError::NotUnicode(_)) => panic!("{name} 必须是有效 UTF-8"),
    }
}
