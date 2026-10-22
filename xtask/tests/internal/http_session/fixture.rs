use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde_json::{Value, json};

use super::Session;

pub(super) const PASSWORD: &str = "Fixture!Password123";

pub(super) struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub header_delay: Duration,
    pub body_delay: Duration,
    pub chunked: bool,
    pub disconnect: bool,
}

impl Reply {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_vec(&value).unwrap(),
            header_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            chunked: false,
            disconnect: false,
        }
    }

    pub fn cookie(mut self, value: &str) -> Self {
        self.headers.push(("Set-Cookie".into(), value.into()));
        self
    }
}

pub(super) struct Captured {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub(super) struct Server {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Captured>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let captured = Arc::clone(&seen);
        let signal = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            let mut replies = VecDeque::from(replies);
            while !signal.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows 接受的套接字可能继承非阻塞状态；夹具按阻塞 IO 和明确超时读取。
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        if let Some(request) = capture(&mut stream) {
                            captured.lock().unwrap().push(request);
                            let reply = replies.pop_front().unwrap_or_else(|| {
                                let mut reply = Reply::json(json!({"code": 500}));
                                reply.status = 500;
                                reply
                            });
                            if !reply.disconnect {
                                let _ = respond(&mut stream, reply);
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url,
            seen,
            stopped,
            worker: Some(worker),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("回环 HTTP 夹具线程必须正常退出");
        }
    }
}

fn capture(stream: &mut TcpStream) -> Option<Captured> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let boundary = loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 || bytes.len() + count > 1024 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..boundary]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let length = headers
        .get("content-length")
        .map_or(Some(0), |value| value.parse::<usize>().ok())?;
    if length > 1024 * 1024 {
        return None;
    }
    while bytes.len() < boundary + length {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Some(Captured {
        method,
        target,
        headers,
        body: bytes[boundary..boundary + length].to_vec(),
    })
}

fn respond(stream: &mut TcpStream, reply: Reply) -> std::io::Result<()> {
    thread::sleep(reply.header_delay);
    write!(
        stream,
        "HTTP/1.1 {} Fixture\r\nConnection: close\r\n",
        reply.status
    )?;
    for (key, value) in reply.headers {
        write!(stream, "{key}: {value}\r\n")?;
    }
    if reply.chunked {
        write!(stream, "Transfer-Encoding: chunked\r\n\r\n")?;
    } else {
        write!(stream, "Content-Length: {}\r\n\r\n", reply.body.len())?;
    }
    stream.flush()?;
    thread::sleep(reply.body_delay);
    if reply.chunked {
        for chunk in reply.body.chunks(64 * 1024) {
            write!(stream, "{:X}\r\n", chunk.len())?;
            stream.write_all(chunk)?;
            stream.write_all(b"\r\n")?;
        }
        stream.write_all(b"0\r\n\r\n")?;
    } else {
        stream.write_all(&reply.body)?;
    }
    stream.flush()
}

pub(super) fn catalog() -> BTreeMap<String, Value> {
    [
        ("get_auth_csrf", "GET", "/api/v1/auth/csrf"),
        ("post_auth_login", "POST", "/api/v1/auth/login"),
        ("post_auth_refresh", "POST", "/api/v1/auth/refresh"),
        ("post_auth_logout", "POST", "/api/v1/auth/logout"),
        ("get_auth_context", "GET", "/api/v1/auth/context"),
        (
            "get_monitor_schedules_by_id",
            "GET",
            "/api/v1/monitor/schedules/{id}",
        ),
        (
            "put_monitor_schedules_by_id_status",
            "PUT",
            "/api/v1/monitor/schedules/{id}/status",
        ),
        ("post_system_depts", "POST", "/api/v1/system/depts"),
    ]
    .into_iter()
    .map(|(id, method, path)| (id.into(), json!({"method": method, "path": path})))
    .collect()
}

pub(super) fn config(url: &str, timeout: u64) -> Value {
    json!({"bindings": {"api_url": url, "frontend_url": "http://localhost:4174"},
        "contract": {"timeout_ms": timeout}})
}

pub(super) fn identity() -> Value {
    json!({"username": "bench", "tenant_id": "tenant-a", "password": PASSWORD})
}

pub(super) fn session(server: &Server) -> Session {
    Session::new(&config(&server.url, 2000), catalog(), &identity()).unwrap()
}
