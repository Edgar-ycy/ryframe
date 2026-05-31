/// grpc 模块测试
/// 从 crates/ryframe-core/src/grpc.rs 内联测试迁移
use ryframe_core::grpc::{GrpcClientConfig, GrpcServerConfig};

#[test]
fn test_grpc_server_config_default() {
    let config = GrpcServerConfig::default();
    assert_eq!(config.addr.to_string(), "0.0.0.0:50051");
    assert!(config.enable_reflection);
}

#[test]
fn test_grpc_client_config() {
    let config = GrpcClientConfig::new("http://localhost:9000").with_timeout(5);
    assert_eq!(config.endpoint, "http://localhost:9000");
    assert_eq!(config.timeout_secs, 5);
}

#[tokio::test]
async fn test_grpc_server_smoke() {
    let server_config = GrpcServerConfig::new("127.0.0.1:50052".parse().unwrap());
    assert_eq!(server_config.addr.port(), 50052);

    let client_config = GrpcClientConfig::new("http://127.0.0.1:50052");
    assert_eq!(client_config.endpoint, "http://127.0.0.1:50052");
}
