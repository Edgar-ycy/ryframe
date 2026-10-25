use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use ryframe_adapters::RedisClient;
use ryframe_config::RedisConfig;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

/// 仅转发本用例的回环连接，不终止 TLS，也不接触 Redis 命令或凭据。
pub(super) struct Proxy {
    address: SocketAddr,
    upstream: (String, u16),
    running: Option<Running>,
}

struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), String>>,
}

impl Proxy {
    pub(super) async fn start(config: &RedisConfig) -> Result<Self, String> {
        let ip = match config.host.as_str() {
            "localhost" | "127.0.0.1" => IpAddr::V4(Ipv4Addr::LOCALHOST),
            "::1" => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            _ => return Err("授权缓存网络故障用例只接受明确回环 Redis；不改写 TLS 主机名".into()),
        };
        let listener = TcpListener::bind(SocketAddr::new(ip, 0))
            .await
            .map_err(|_| "无法创建本用例回环 Redis 代理")?;
        let address = listener
            .local_addr()
            .map_err(|_| "无法读取本用例代理地址")?;
        let upstream = (config.host.clone(), config.port);
        let running = Some(spawn(listener, upstream.clone()));
        Ok(Self {
            address,
            upstream,
            running,
        })
    }

    pub(super) fn configuration(&self, original: &RedisConfig) -> RedisConfig {
        let mut derived = original.clone();
        // host、TLS、认证、database、scope 和超时均保持原配置，仅绑定本用例端口。
        derived.port = self.address.port();
        derived
    }

    pub(super) async fn disconnect(&mut self) -> Result<(), String> {
        let running = self.running.take().ok_or("本用例代理没有运行")?;
        let _ = running.stop.send(());
        running.task.await.map_err(|_| "本用例代理停止任务异常")?
    }

    pub(super) async fn restore(&mut self) -> Result<(), String> {
        if self.running.is_some() {
            return Err("本用例代理仍在运行，拒绝重复绑定".into());
        }
        let listener = TcpListener::bind(self.address)
            .await
            .map_err(|_| "无法重新独占原代理端口；不换端口、不重建客户端，保留失败")?;
        self.running = Some(spawn(listener, self.upstream.clone()));
        Ok(())
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = running.stop.send(());
            // 运行任务的 JoinSet 随任务释放会中止全部连接，覆盖 panic/取消路径。
            running.task.abort();
        }
    }
}

fn spawn(listener: TcpListener, upstream: (String, u16)) -> Running {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                biased;
                _ = &mut stopped => break Ok(()),
                accepted = listener.accept() => {
                    let (mut inbound, peer) = match accepted {
                        Ok(accepted) => accepted,
                        Err(_) => break Err("本用例代理 accept 失败".into()),
                    };
                    if !peer.ip().is_loopback() { break Err("本用例代理拒绝非回环连接".into()); }
                    let upstream = upstream.clone();
                    connections.spawn(async move {
                        let mut outbound = TcpStream::connect((upstream.0.as_str(), upstream.1)).await?;
                        if !outbound.peer_addr()?.ip().is_loopback() {
                            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,
                                "本用例代理拒绝非回环上游"));
                        }
                        tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await
                    });
                }
                completed = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(_)) = completed { break Err("本用例代理转发任务 panic".into()); }
                    // Redis 客户端正常退役或断开连接也可能产生 I/O 错误；以真实命令结果判定可用性。
                }
            }
        };
        drop(listener);
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        result
    });
    Running { stop, task }
}

pub(super) async fn require_reconnected(
    client: &RedisClient,
    timeout_secs: u64,
) -> Result<(), String> {
    // 只重试无写入的 PING，等待同一客户端恢复；这不是性能门槛，也不重放写入。
    let deadline = Duration::from_secs(timeout_secs).saturating_add(Duration::from_secs(60));
    tokio::time::timeout(deadline, async {
        loop {
            if client.ping().await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "原 Redis 客户端未在有界期限内恢复；不替换客户端".to_owned())
}
