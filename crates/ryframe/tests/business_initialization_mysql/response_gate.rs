//! TLS 透明转发；只由权威已提交账本决定是否丢弃响应，不读取或记录协议载荷。
use std::{io, net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, tcp::OwnedReadHalf, tcp::OwnedWriteHalf},
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};

pub(super) struct ResponseGate {
    pub address: SocketAddr,
    pub responses: mpsc::Receiver<oneshot::Sender<bool>>,
    armed: watch::Sender<bool>,
    stop: watch::Sender<bool>,
    task: JoinHandle<io::Result<()>>,
}

impl ResponseGate {
    pub async fn start(host: String, port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let (events, responses) = mpsc::channel(16);
        let (armed, armed_rx) = watch::channel(false);
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(accept_connections(
            listener,
            Arc::new((host, port)),
            events,
            armed_rx,
            stop_rx,
        ));
        Ok(Self {
            address,
            responses,
            armed,
            stop,
            task,
        })
    }

    pub fn arm(&self) {
        self.armed.send_replace(true);
    }

    pub async fn stop(self) -> Result<(), String> {
        self.stop.send_replace(true);
        self.task
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    }
}

async fn accept_connections(
    listener: TcpListener,
    upstream: Arc<(String, u16)>,
    events: mpsc::Sender<oneshot::Sender<bool>>,
    armed: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            _ = stopped(&mut stop) => break Ok(()),
            accepted = listener.accept() => {
                let (client, _) = match accepted {
                    Ok(value) => value,
                    Err(error) => break Err(error),
                };
                connections.spawn(forward_connection(client, upstream.clone(), events.clone(), armed.clone(), stop.clone()));
            },
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed {
                    break Err(io::Error::other(error.to_string()));
                }
            },
        }
    };
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    result
}

async fn forward_connection(
    client: TcpStream,
    upstream: Arc<(String, u16)>,
    events: mpsc::Sender<oneshot::Sender<bool>>,
    armed: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let server = tokio::select! {
        _ = stopped(&mut stop) => return Ok(()),
        result = TcpStream::connect((upstream.0.as_str(), upstream.1)) => result?,
    };
    let (mut client_read, client_write) = client.into_split();
    let (server_read, mut server_write) = server.into_split();
    let response_stop = stop.clone();
    tokio::select! {
        _ = stopped(&mut stop) => Ok(()),
        result = tokio::io::copy(&mut client_read, &mut server_write) => result.map(|_| ()),
        result = forward_responses(server_read, client_write, events, armed, response_stop) => result,
    }
}

async fn forward_responses(
    mut source: OwnedReadHalf,
    mut destination: OwnedWriteHalf,
    events: mpsc::Sender<oneshot::Sender<bool>>,
    armed: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut buffer = [0_u8; 16_384];
    loop {
        let count = tokio::select! {
            _ = stopped(&mut stop) => return Ok(()),
            count = source.read(&mut buffer) => count?,
        };
        if count == 0 {
            return Ok(());
        }
        if *armed.borrow() {
            let (permit, waiting) = oneshot::channel();
            tokio::select! {
                _ = stopped(&mut stop) => return Ok(()),
                sent = events.send(permit) => sent.map_err(|_| io::Error::other("响应观察器已退出"))?,
            }
            let forward = tokio::select! {
                _ = stopped(&mut stop) => return Ok(()),
                permit = waiting => permit.unwrap_or(false),
            };
            if !forward {
                return Ok(());
            }
        }
        tokio::select! {
            _ = stopped(&mut stop) => return Ok(()),
            result = destination.write_all(&buffer[..count]) => result?,
        }
    }
}

async fn stopped(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}
