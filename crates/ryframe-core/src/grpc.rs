//! gRPC 服务端/客户端模块
//!
//! 基于 tonic 提供 gRPC 微服务通信能力。
//! 支持服务端启动、客户端连接等。

use std::{convert::Infallible, net::SocketAddr};

use tonic::{body::Body, codegen::Service, server::NamedService};
use tracing::{info, warn};

/// gRPC 服务端配置
#[derive(Clone, Debug)]
pub struct GrpcServerConfig {
    /// 监听地址
    pub addr: SocketAddr,
    /// 是否启用反射（开发调试用）
    pub enable_reflection: bool,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0:50051".parse().unwrap(),
            enable_reflection: true,
        }
    }
}

impl GrpcServerConfig {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            enable_reflection: true,
        }
    }

    pub fn with_reflection(mut self, enable: bool) -> Self {
        self.enable_reflection = enable;
        self
    }
}

/// gRPC 客户端配置
#[derive(Clone, Debug)]
pub struct GrpcClientConfig {
    /// 目标地址（格式：`http://host:port`）
    pub endpoint: String,
    /// 连接超时（秒）
    pub timeout_secs: u64,
}

impl Default for GrpcClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_string(),
            timeout_secs: 10,
        }
    }
}

impl GrpcClientConfig {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            timeout_secs: 10,
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// gRPC 服务端
///
/// 封装 tonic Server，提供便捷的启动和关闭接口。
pub struct GrpcServer {
    config: GrpcServerConfig,
}

impl GrpcServer {
    /// 创建新的 gRPC 服务端
    pub fn new(config: GrpcServerConfig) -> Self {
        Self { config }
    }

    /// 启动 gRPC 服务端
    ///
    /// 返回 shutdown 发送端，调用 `send(())` 可优雅关闭服务。
    /// 传入的 `service` 为 tonic 服务实现（通过 proto 生成）。
    pub async fn serve<S>(
        self,
        service: S,
    ) -> Result<tokio::sync::oneshot::Sender<()>, Box<dyn std::error::Error>>
    where
        S: Service<http::Request<Body>, Error = Infallible>
            + NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        S::Response: axum::response::IntoResponse,
        S::Future: Send + 'static,
    {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let addr = self.config.addr;

        info!("gRPC 服务端启动于: {}", addr);

        let router = tonic::transport::Server::builder().add_service(service);

        let server = router.serve_with_shutdown(addr, async {
            let _ = shutdown_rx.await;
            info!("gRPC 服务端收到关闭信号");
        });

        tokio::spawn(async move {
            if let Err(e) = server.await {
                warn!("gRPC 服务端异常退出: {}", e);
            }
        });

        Ok(shutdown_tx)
    }
}

/// gRPC 客户端构建器
///
/// 简化 tonic 客户端连接的创建和管理。
pub struct GrpcClient;

impl GrpcClient {
    /// 创建到指定地址的通道
    pub async fn connect(
        config: &GrpcClientConfig,
    ) -> Result<tonic::transport::Channel, tonic::transport::Error> {
        let endpoint = tonic::transport::Endpoint::from_shared(config.endpoint.clone())?
            .timeout(std::time::Duration::from_secs(config.timeout_secs));

        info!("gRPC 客户端连接至: {}", config.endpoint);
        endpoint.connect().await
    }

    /// 创建到本地默认地址的通道
    pub async fn connect_default() -> Result<tonic::transport::Channel, tonic::transport::Error> {
        Self::connect(&GrpcClientConfig::default()).await
    }
}
