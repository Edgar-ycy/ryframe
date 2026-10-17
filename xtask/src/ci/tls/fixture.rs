use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use crate::Result;

use super::RedisEndpoint;

const ACCEPT_POLL: Duration = Duration::from_millis(10);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const UPSTREAM_RESPONSE_IDLE: Duration = Duration::from_millis(500);
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
struct FixtureLog {
    file: Arc<Mutex<File>>,
}

impl FixtureLog {
    fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| format!("无法创建 TLS fixture 日志 {}：{error}", path.display()))?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }

    fn line(&self, message: impl AsRef<str>) -> io::Result<()> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("TLS fixture 日志锁已损坏"))?;
        writeln!(file, "{}", message.as_ref())?;
        file.flush()
    }
}

#[derive(Clone)]
enum ConnectionHandler {
    RedisProxy(SocketAddr),
    Https,
}

pub(crate) struct FixtureServers {
    redis: TlsListener,
    https: TlsListener,
    log: FixtureLog,
    stopped: bool,
}

impl FixtureServers {
    pub(crate) fn start(
        certificate: &Path,
        private_key: &Path,
        upstream: SocketAddr,
        log_path: &Path,
    ) -> Result<Self> {
        let config = server_config(certificate, private_key)?;
        let log = FixtureLog::open(log_path)?;
        let redis = TlsListener::start(
            "redis-tls",
            "redis".to_owned(),
            config.clone(),
            ConnectionHandler::RedisProxy(upstream),
            log.clone(),
        )?;
        let https = TlsListener::start(
            "https",
            "https".to_owned(),
            config,
            ConnectionHandler::Https,
            log.clone(),
        )?;
        log.line(format!(
            "redis_tls_ready=127.0.0.1:{}\nhttps_ready=127.0.0.1:{}",
            redis.port, https.port
        ))?;
        Ok(Self {
            redis,
            https,
            log,
            stopped: false,
        })
    }

    pub(crate) const fn redis_port(&self) -> u16 {
        self.redis.port
    }

    pub(crate) const fn https_port(&self) -> u16 {
        self.https.port
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.redis.stop();
        self.https.stop();
        self.stopped = true;
        self.log
            .line("fixtures_stopped=1")
            .map_err(|error| format!("写入 TLS fixture 停止记录失败：{error}"))?;
        Ok(())
    }
}

impl Drop for FixtureServers {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct TlsListener {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl TlsListener {
    fn start(
        name: &str,
        log_label: String,
        config: Arc<ServerConfig>,
        handler: ConnectionHandler,
        log: FixtureLog,
    ) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("无法绑定 {name} TLS fixture 回环端口：{error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("无法读取 {name} TLS fixture 端口：{error}"))?
            .port();
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("无法设置 {name} TLS fixture 非阻塞监听：{error}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let workers = Arc::new(Mutex::new(Vec::new()));
        let thread_workers = Arc::clone(&workers);
        let thread = thread::Builder::new()
            .name(format!("ryframe-{name}-fixture"))
            .spawn(move || {
                accept_loop(
                    listener,
                    config,
                    handler,
                    log,
                    thread_stop,
                    thread_workers,
                    log_label,
                );
            })
            .map_err(|error| format!("无法启动 {name} TLS fixture 线程：{error}"))?;
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
            workers,
        })
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let handles = self
            .workers
            .lock()
            .map(|mut workers| std::mem::take(&mut *workers))
            .unwrap_or_default();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(
    listener: TcpListener,
    config: Arc<ServerConfig>,
    handler: ConnectionHandler,
    log: FixtureLog,
    stop: Arc<AtomicBool>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    log_label: String,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((socket, peer)) => {
                let _ = log.line(format!("{log_label}_tcp_accept={peer}"));
                if let Err(error) = prepare_socket(&socket) {
                    let _ = log.line(format!("{log_label}_socket_error={error}"));
                    continue;
                }
                let connection = match ServerConnection::new(Arc::clone(&config)) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = log.line(format!("{log_label}_connection_error={error}"));
                        continue;
                    }
                };
                let stream = StreamOwned::new(connection, socket);
                let handler = handler.clone();
                let worker_log = log.clone();
                let worker_label = log_label.clone();
                let worker_name = format!("ryframe-{log_label}-fixture-connection");
                match thread::Builder::new()
                    .name(worker_name)
                    .spawn(move || serve(stream, handler, worker_log, worker_label, peer))
                {
                    Ok(worker) => {
                        if let Ok(mut workers) = workers.lock() {
                            workers.push(worker);
                        }
                    }
                    Err(error) => {
                        let _ = log.line(format!("{log_label}_worker_error={error}"));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                let _ = log.line(format!("{log_label}_accept_error={error}"));
                thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

fn prepare_socket(socket: &TcpStream) -> io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    socket.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    socket.set_nodelay(true)
}

fn serve(
    mut stream: StreamOwned<ServerConnection, TcpStream>,
    handler: ConnectionHandler,
    log: FixtureLog,
    log_label: String,
    peer: SocketAddr,
) {
    if let Err(error) = stream.conn.complete_io(&mut stream.sock) {
        let _ = log.line(format!("{log_label}_tls_handshake_error={peer}: {error}"));
        return;
    }
    let result = match handler {
        ConnectionHandler::RedisProxy(upstream) => proxy_redis(&mut stream, upstream),
        ConnectionHandler::Https => serve_https(&mut stream),
    };
    if let Err(error) = result {
        let _ = log.line(format!("{log_label}_proxy_client_error={peer}: {error}"));
    }
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

fn proxy_redis(
    stream: &mut StreamOwned<ServerConnection, TcpStream>,
    upstream: SocketAddr,
) -> io::Result<()> {
    let mut upstream = TcpStream::connect_timeout(&upstream, SOCKET_TIMEOUT)?;
    upstream.set_read_timeout(Some(UPSTREAM_RESPONSE_IDLE))?;
    upstream.set_write_timeout(Some(SOCKET_TIMEOUT))?;
    upstream.set_nodelay(true)?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        upstream.write_all(&buffer[..read])?;
        upstream.flush()?;
        loop {
            match upstream.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => {
                    stream.write_all(&buffer[..read])?;
                    stream.flush()?;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn serve_https(stream: &mut StreamOwned<ServerConnection, TcpStream>) -> io::Result<()> {
    let Some((head, initial_body)) = read_http_head(stream)? else {
        return write_http_response(stream, 400, "");
    };
    let Some(first_line) = head.lines().next() else {
        return write_http_response(stream, 400, "");
    };
    let method = first_line
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default();
    let content_length = match header_value(&head, "content-length") {
        Some(value) => match value.parse::<usize>() {
            Ok(length) if length <= MAX_REQUEST_BYTES => length,
            _ => return write_http_response(stream, 413, ""),
        },
        None => 0,
    };
    if header_value(&head, "expect").is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        stream.flush()?;
    }
    discard_body(stream, content_length, initial_body)?;
    match method {
        "HEAD" => write_http_response(stream, 200, ""),
        "GET" => write_http_response(stream, 200, "ryframe-https-ok"),
        "POST" => write_http_response(stream, 200, ""),
        _ => write_http_response(stream, 405, ""),
    }
}

fn read_http_head(stream: &mut impl Read) -> io::Result<Option<(String, Vec<u8>)>> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let body = bytes.split_off(position + 4);
            return String::from_utf8(bytes)
                .map(|head| Some((head, body)))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Ok(None);
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

fn header_value<'a>(head: &'a str, wanted: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(wanted)
            .then(|| value.trim())
    })
}

fn discard_body(stream: &mut impl Read, length: usize, initial: Vec<u8>) -> io::Result<()> {
    let mut remaining = length.saturating_sub(initial.len());
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let limit = remaining.min(buffer.len());
        let read = stream.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HTTP 请求体提前结束",
            ));
        }
        remaining -= read;
    }
    Ok(())
}

fn write_http_response(stream: &mut impl Write, status: u16, body: &str) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn server_config(certificate: &Path, private_key: &Path) -> Result<Arc<ServerConfig>> {
    let certificates = CertificateDer::pem_file_iter(certificate)
        .map_err(|error| {
            format!(
                "无法读取 TLS fixture 服务证书 {}：{error}",
                certificate.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "无法解析 TLS fixture 服务证书 {}：{error}",
                certificate.display()
            )
        })?;
    let key = PrivateKeyDer::from_pem_file(private_key).map_err(|error| {
        format!(
            "无法读取 TLS fixture 私钥 {}：{error}",
            private_key.display()
        )
    })?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|error| format!("无法配置 TLS fixture 协议版本：{error}"))?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|error| format!("无法装载 TLS fixture 证书和私钥：{error}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub(crate) fn resolved_endpoints(endpoint: &RedisEndpoint) -> Result<Vec<SocketAddr>> {
    use std::net::ToSocketAddrs;

    let addresses = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|error| {
            format!(
                "无法解析回环 Redis {}:{}：{error}",
                endpoint.host, endpoint.port
            )
        })?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!(
            "回环 Redis {}:{} 没有可用地址",
            endpoint.host, endpoint.port
        )
        .into());
    }
    Ok(addresses)
}
