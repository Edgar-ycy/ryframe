use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Result;

const MAX_BOOTSTRAP_BYTES: u64 = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 4096;
const PASSWORD_NAME: &str = "reset-admin-password.txt";

pub(super) struct CredentialBroker {
    endpoint: String,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    failure: Arc<Mutex<Option<&'static str>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl CredentialBroker {
    pub(super) fn start(bootstrap: &Path) -> Result<Self> {
        let password = read_password(bootstrap)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("http://{}/v1/credentials", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let failure = Arc::new(Mutex::new(None));
        let thread = {
            let stop = Arc::clone(&stop);
            let requests = Arc::clone(&requests);
            let failure = Arc::clone(&failure);
            thread::spawn(move || serve(listener, password, &stop, &requests, &failure))
        };
        Ok(Self {
            endpoint,
            stop,
            requests,
            failure,
            thread: Some(thread),
        })
    }

    pub(super) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(super) fn finish(mut self, require_request: bool) -> Result<()> {
        self.stop();
        let failure = *self.failure.lock().map_err(|_| "浏览器凭据代理状态损坏")?;
        if let Some(message) = failure {
            return Err(message.into());
        }
        if require_request && self.requests.load(Ordering::Acquire) == 0 {
            return Err("浏览器验收未读取受管凭据；拒绝接受可能跳过登录的结果".into());
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CredentialBroker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_password(bootstrap: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(bootstrap)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BOOTSTRAP_BYTES {
        return Err("浏览器凭据环境必须是大小受限的普通文件".into());
    }
    let bytes = fs::read(bootstrap)?;
    let root: Value = serde_json::from_slice(&bytes).map_err(|_| "浏览器凭据环境不是有效 JSON")?;
    let descriptor = root
        .get("secret_files")
        .and_then(Value::as_object)
        .and_then(|files| files.get(PASSWORD_NAME))
        .and_then(Value::as_object)
        .ok_or("浏览器凭据环境缺少管理员密码文件绑定")?;
    let path = descriptor
        .get("path")
        .and_then(Value::as_str)
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or("浏览器管理员密码绑定路径无效")?;
    let expected_bytes = descriptor
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or("浏览器管理员密码绑定字节数无效")?;
    let expected_sha = descriptor
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or("浏览器管理员密码绑定摘要无效")?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() != expected_bytes {
        return Err("浏览器管理员密码文件类型或字节数与绑定不一致".into());
    }
    let password = fs::read(path)?;
    let actual_sha = hex::encode(Sha256::digest(&password));
    if actual_sha != expected_sha {
        return Err("浏览器管理员密码文件摘要与绑定不一致".into());
    }
    let password = String::from_utf8(password).map_err(|_| "浏览器管理员密码文件必须是 UTF-8")?;
    let password = password.trim().to_owned();
    if password.is_empty() || password.contains(['\r', '\n', '\0']) {
        return Err("浏览器管理员密码文件内容无效".into());
    }
    Ok(password)
}

fn serve(
    listener: TcpListener,
    password: String,
    stop: &AtomicBool,
    requests: &AtomicUsize,
    failure: &Mutex<Option<&'static str>>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, address)) if address.ip().is_loopback() => {
                if handle(stream, &password).is_ok() {
                    requests.fetch_add(1, Ordering::AcqRel);
                }
            }
            Ok(_) => set_failure(failure, "浏览器凭据代理拒绝了非本机连接"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                set_failure(failure, "浏览器凭据代理接受连接失败");
                return;
            }
        }
    }
}

fn handle(mut stream: TcpStream, password: &str) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    exchange(&mut stream, password)
}

fn exchange(stream: &mut (impl Read + Write), password: &str) -> Result<()> {
    let request = read_request(stream)?;
    if !valid_request(&request) {
        stream.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Err("浏览器凭据请求无效".into());
    }
    stream.write_all(&credential_response(password)?)?;
    stream.flush()?;
    Ok(())
}

fn read_request(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut request = Vec::new();
    while request.len() <= MAX_REQUEST_BYTES && !request.ends_with(b"\r\n\r\n") {
        let mut chunk = [0_u8; 512];
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
    }
    Ok(request)
}

fn valid_request(request: &[u8]) -> bool {
    request.len() <= MAX_REQUEST_BYTES
        && std::str::from_utf8(request).is_ok_and(|request| {
            request.starts_with("GET /v1/credentials HTTP/1.1\r\n") && request.contains("\r\n\r\n")
        })
}

fn credential_response(password: &str) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(&json!({"password": password}))?;
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    Ok([header.as_bytes(), &body].concat())
}

fn set_failure(failure: &Mutex<Option<&'static str>>, message: &'static str) {
    if let Ok(mut failure) = failure.lock() {
        failure.get_or_insert(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn fixture(password: &str) -> (TempDir, std::path::PathBuf) {
        let directory = TempDir::new().unwrap();
        let secret = directory.path().join(PASSWORD_NAME);
        fs::write(&secret, password).unwrap();
        let bootstrap = directory.path().join("bootstrap.json");
        fs::write(
            &bootstrap,
            serde_json::to_vec(&json!({
                "secret_files": {PASSWORD_NAME: {
                    "path": secret,
                    "bytes": password.len(),
                    "sha256": hex::encode(Sha256::digest(password.as_bytes()))
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        (directory, bootstrap)
    }

    #[test]
    fn serves_bound_password_without_placing_it_in_endpoint() {
        let (_directory, bootstrap) = fixture("Private-Sentinel-123");
        let broker = CredentialBroker::start(&bootstrap).unwrap();
        assert!(!broker.endpoint().contains("Private-Sentinel-123"));
        let mut stream = Duplex {
            input: Cursor::new(b"GET /v1/credentials HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_vec()),
            output: Vec::new(),
        };
        exchange(&mut stream, "Private-Sentinel-123").unwrap();
        let response = String::from_utf8(stream.output).unwrap();
        assert!(response.contains("\r\nCache-Control: no-store\r\n"));
        assert!(response.ends_with(r#"{"password":"Private-Sentinel-123"}"#));
        assert!(valid_request(
            b"GET /v1/credentials HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        ));
        assert!(!valid_request(b"GET /other HTTP/1.1\r\n\r\n"));
        broker.finish(false).unwrap();
    }

    #[test]
    fn rejects_drift_and_a_success_without_a_credential_read() {
        let (_directory, bootstrap) = fixture("Private-Sentinel-123");
        let mut value: Value = serde_json::from_slice(&fs::read(&bootstrap).unwrap()).unwrap();
        value["secret_files"][PASSWORD_NAME]["sha256"] = json!("0".repeat(64));
        fs::write(&bootstrap, serde_json::to_vec(&value).unwrap()).unwrap();
        let error = CredentialBroker::start(&bootstrap)
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("Private-Sentinel-123"));

        let (_directory, bootstrap) = fixture("Private-Sentinel-123");
        let error = CredentialBroker::start(&bootstrap)
            .unwrap()
            .finish(true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("未读取受管凭据"));
        assert!(!error.contains("Private-Sentinel-123"));
    }
}
