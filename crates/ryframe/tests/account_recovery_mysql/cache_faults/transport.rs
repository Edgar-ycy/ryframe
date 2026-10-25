use std::{net::SocketAddr, time::Duration};

use ryframe_adapters::RedisClient;
use ryframe_config::RedisConfig;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

/// 仅转发本用例的回环连接，不终止 TLS，也不接触 Redis 命令或凭据。
pub(super) struct Proxy {
    addresses: Vec<SocketAddr>,
    upstream: (String, u16),
    running: Vec<Running>,
}

struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), String>>,
}

impl Proxy {
    pub(super) async fn start(config: &RedisConfig) -> Result<Self, String> {
        if !matches!(config.host.as_str(), "localhost" | "127.0.0.1" | "::1") {
            return Err("故障代理只接受显式回环主机，不改写 TLS 主机名".into());
        }
        let listeners = bind_hostname(&config.host).await?;
        let addresses = listeners
            .iter()
            .map(TcpListener::local_addr)
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|_| "无法读取代理监听地址")?;
        eprintln!("故障代理监听回环地址：{addresses:?}");
        let upstream = (config.host.clone(), config.port);
        let running = listeners
            .into_iter()
            .map(|listener| spawn(listener, upstream.clone()))
            .collect();
        Ok(Self {
            addresses,
            upstream,
            running,
        })
    }

    pub(super) fn configuration(&self, original: &RedisConfig) -> RedisConfig {
        let mut derived = original.clone();
        // host、TLS、认证、database、scope 和超时均保持原配置，仅绑定本用例端口。
        derived.port = self.addresses[0].port();
        derived
    }

    pub(super) async fn disconnect(&mut self) -> Result<(), String> {
        if self.running.is_empty() {
            return Err("本用例代理当前未运行".into());
        }
        let mut tasks = Vec::new();
        for running in self.running.drain(..) {
            let _ = running.stop.send(());
            tasks.push(running.task);
        }
        let mut errors = Vec::new();
        for task in tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(error),
                Err(_) => errors.push("本用例代理退出任务异常".into()),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    pub(super) async fn restore(&mut self) -> Result<(), String> {
        if !self.running.is_empty() {
            return Err("本用例代理仍在运行，拒绝重复绑定".into());
        }
        // 全部地址成功绑定后才接收连接；恢复不换主机名、端口或地址族。
        let mut listeners = Vec::new();
        for address in &self.addresses {
            listeners.push(
                TcpListener::bind(address)
                    .await
                    .map_err(|_| "原代理回环地址无法重新独占；不换端口或配置，保留现场")?,
            );
        }
        self.running = listeners
            .into_iter()
            .map(|listener| spawn(listener, self.upstream.clone()))
            .collect();
        Ok(())
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        for running in self.running.drain(..) {
            let _ = running.stop.send(());
            // 运行任务的 JoinSet 随任务释放会中止全部连接，覆盖 panic/取消路径。
            running.task.abort();
        }
    }
}

/// 与客户端使用同一主机名解析入口，完整监听其回环地址族，共用一个端口。
async fn bind_hostname(host: &str) -> Result<Vec<TcpListener>, String> {
    let mut addresses = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|_| "无法解析代理回环主机")?
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() || addresses.iter().any(|address| !address.ip().is_loopback()) {
        return Err("代理主机必须只解析到回环地址".into());
    }
    let mut listeners: Vec<TcpListener> = Vec::new();
    let mut port = 0;
    for mut address in addresses {
        address.set_port(port);
        let listener = TcpListener::bind(address)
            .await
            .map_err(|_| "无法独占代理主机的全部回环地址")?;
        port = listener
            .local_addr()
            .map_err(|_| "无法读取代理端口")?
            .port();
        listeners.push(listener);
    }
    Ok(listeners)
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
                    match completed {
                        Some(Err(_)) => break Err("本用例代理转发任务 panic".into()),
                        Some(Ok(Err(error))) => eprintln!(
                            "回环代理连接退役：kind={:?} os_code={:?}", error.kind(), error.raw_os_error()),
                        _ => {}
                    }
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
