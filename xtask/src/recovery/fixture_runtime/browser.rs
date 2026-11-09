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

use crate::Result;
use serde_json::{Value, json};

const MAX_BOOTSTRAP_BYTES: u64 = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 4096;

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
    super::model::safe(bootstrap)?;
    let metadata = fs::symlink_metadata(bootstrap)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BOOTSTRAP_BYTES {
        return Err("浏览器凭据环境必须是大小受限的普通文件".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(bootstrap)?
        .take(MAX_BOOTSTRAP_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BOOTSTRAP_BYTES {
        return Err("浏览器凭据环境超过大小限制".into());
    }
    let root = crate::recovery::clone::native::json::parse_fragment(
        std::str::from_utf8(&bytes).map_err(|_| "浏览器凭据环境不是有效 UTF-8")?,
        "浏览器凭据环境",
    )
    .map_err(|_| "浏览器凭据环境不是有效 JSON")?;
    let execution = root
        .get("execution_backend")
        .and_then(Value::as_str)
        .map(Path::new)
        .ok_or("浏览器凭据环境缺少执行后端路径")?;
    crate::local_test_path::validate_local_test_path(
        bootstrap,
        execution,
        crate::local_test_path::LocalTestPathKind::ExistingFile,
    )?;
    let descriptor = root
        .get("administrator_credentials")
        .ok_or("浏览器凭据环境缺少当前管理员凭据绑定")?;
    let password = crate::recovery::clone::native::session::credentials::load_account(
        execution, descriptor, "system", "admin",
    )?;
    super::super::fixture_control::environment::validate_password(&password)
        .map_err(|_| "浏览器当前管理员密码不符合密码策略")?;
    super::model::safe(bootstrap)?;
    let mut after = Vec::new();
    fs::File::open(bootstrap)?
        .take(MAX_BOOTSTRAP_BYTES + 1)
        .read_to_end(&mut after)?;
    if after != bytes {
        return Err("浏览器凭据环境读取期间变化".into());
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
    use sha2::{Digest, Sha256};
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&root).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("browser-credentials-")
            .tempdir_in(root)
            .unwrap();
        let local = directory.path().join(".local-tests");
        fs::create_dir(&local).unwrap();
        let initial = b"Initial-Sentinel-456!";
        let secret = local.join("reset-admin-password.txt");
        fs::write(&secret, initial).unwrap();
        let current = local.join("administrator.toml");
        let contents = format!(
            "[administrator]\ntenant_id = \"system\"\nusername = \"admin\"\npassword = {password:?}\n"
        );
        fs::write(&current, &contents).unwrap();
        let bootstrap = local.join("bootstrap.json");
        fs::write(
            &bootstrap,
            serde_json::to_vec(&json!({
                "execution_backend": directory.path(),
                "administrator_credentials": {
                    "path": current,
                    "bytes": contents.len(),
                    "sha256": hex::encode(Sha256::digest(contents.as_bytes()))
                },
                "secret_files": {"reset-admin-password.txt": {
                    "path": secret,
                    "bytes": initial.len(),
                    "sha256": hex::encode(Sha256::digest(initial))
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
        assert!(!broker.endpoint().contains("Initial-Sentinel-456!"));
        let address = broker
            .endpoint()
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/v1/credentials")
            .unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .write_all(b"GET /v1/credentials HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.contains("\r\nCache-Control: no-store\r\n"));
        assert!(response.ends_with(r#"{"password":"Private-Sentinel-123"}"#));
        assert!(!response.contains("Initial-Sentinel-456!"));
        let mut stream = Duplex {
            input: Cursor::new(b"GET /other HTTP/1.1\r\n\r\n".to_vec()),
            output: Vec::new(),
        };
        assert!(exchange(&mut stream, "Private-Sentinel-123").is_err());
        assert!(stream.output.starts_with(b"HTTP/1.1 400 Bad Request\r\n"));
        assert!(valid_request(
            b"GET /v1/credentials HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        ));
        assert!(!valid_request(b"GET /other HTTP/1.1\r\n\r\n"));
        broker.finish(true).unwrap();
    }

    #[test]
    fn rejects_drift_and_a_success_without_a_credential_read() {
        for failure in [
            "digest",
            "missing-binding",
            "missing-file",
            "malformed",
            "account",
            "policy",
        ] {
            let (_directory, bootstrap) = fixture("Private-Sentinel-123");
            let mut value: Value = serde_json::from_slice(&fs::read(&bootstrap).unwrap()).unwrap();
            let path = std::path::PathBuf::from(
                value["administrator_credentials"]["path"].as_str().unwrap(),
            );
            match failure {
                "digest" => value["administrator_credentials"]["sha256"] = json!("0".repeat(64)),
                "missing-binding" => {
                    value
                        .as_object_mut()
                        .unwrap()
                        .remove("administrator_credentials");
                }
                "missing-file" => fs::remove_file(&path).unwrap(),
                _ => {
                    let contents = fs::read_to_string(&path).unwrap();
                    let changed = match failure {
                        "malformed" => format!("{contents}[Private-Sentinel-123"),
                        "account" => contents.replace("\"admin\"", "\"other\""),
                        _ => contents.replace("Private-Sentinel-123", "weak"),
                    };
                    fs::write(&path, &changed).unwrap();
                    value["administrator_credentials"]["bytes"] = json!(changed.len());
                    value["administrator_credentials"]["sha256"] =
                        json!(hex::encode(Sha256::digest(changed.as_bytes())));
                }
            }
            fs::write(&bootstrap, serde_json::to_vec(&value).unwrap()).unwrap();
            let error = CredentialBroker::start(&bootstrap)
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains("Private-Sentinel-123"));
            assert!(!error.contains("Initial-Sentinel-456!"));
        }

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
