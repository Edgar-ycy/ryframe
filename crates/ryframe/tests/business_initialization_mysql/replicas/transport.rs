use ryframe_config::DbConnection;
use std::net::SocketAddr;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

/// 仅转发已绑定 MySQL 的回环字节流；不解析命令、凭据或 TLS。
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
    pub(super) async fn start(original: &DbConnection) -> Result<Self, String> {
        if !matches!(original.host.as_str(), "localhost" | "127.0.0.1" | "::1") {
            return Err("故障代理只接受显式回环主机，不改写 TLS 主机名".into());
        }
        let listeners = bind_hostname(&original.host).await?;
        let addresses = listeners
            .iter()
            .map(TcpListener::local_addr)
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|_| "无法读取代理监听地址")?;
        eprintln!("故障代理监听回环地址：{addresses:?}");
        let upstream = (original.host.clone(), original.port);
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

    pub(super) fn port(&self) -> u16 {
        self.addresses[0].port()
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
            return Err("拒绝重复启动本用例代理".into());
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

    pub(super) async fn close(&mut self) -> Result<(), String> {
        if !self.running.is_empty() {
            self.disconnect().await
        } else {
            Ok(())
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        for running in self.running.drain(..) {
            let _ = running.stop.send(());
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
                        Ok(value) => value,
                        Err(_) => break Err("本用例 MySQL 代理 accept 失败".into()),
                    };
                    if !peer.ip().is_loopback() { break Err("本用例代理拒绝非回环连接".into()); }
                    let upstream = upstream.clone();
                    connections.spawn(async move {
                        // 不让代理额外的 Nagle 等待改变生产探测的两秒预算。
                        inbound.set_nodelay(true)?;
                        let mut outbound = TcpStream::connect((upstream.0.as_str(), upstream.1)).await?;
                        if !outbound.peer_addr()?.ip().is_loopback() {
                            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "本用例代理拒绝非回环上游"));
                        }
                        outbound.set_nodelay(true)?;
                        eprintln!("MySQL 透明代理已连接：listener={} upstream={}",
                            inbound.local_addr()?, outbound.peer_addr()?);
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
                    // 普通连接退役的 I/O 结果不模拟数据库错误，业务 SQL 使用真实结果。
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
