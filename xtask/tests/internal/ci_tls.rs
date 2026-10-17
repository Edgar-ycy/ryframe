use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use rustls::{
    ClientConfig, ClientConnection, RootCertStore, StreamOwned,
    pki_types::{CertificateDer, ServerName, pem::PemObject},
};
use tempfile::tempdir;

use crate::ci::tls::{
    FixtureServers, ProcessOutcome, TESTS, check_redis, collect_test_failures,
    generate_certificates_with_program, redis_endpoint, reserve_output_dir, run_redirected,
    test_command, test_environment,
};

struct TestFixture {
    _directory: tempfile::TempDir,
    ca: PathBuf,
    servers: FixtureServers,
    log: PathBuf,
}

impl TestFixture {
    fn start(upstream: SocketAddr) -> Self {
        let directory = tempdir().expect("创建 TLS fixture 测试目录");
        let material_dir = directory.path().join("material");
        fs::create_dir(&material_dir).expect("创建证书目录");
        let material = generate_certificates_with_program(
            openssl_program(),
            &material_dir,
            &directory.path().join("openssl.log"),
        )
        .expect("生成测试证书");
        let log = directory.path().join("fixtures.log");
        let servers =
            FixtureServers::start(&material.server_cert, &material.server_key, upstream, &log)
                .expect("启动 TLS fixture");
        Self {
            _directory: directory,
            ca: material.ca_cert,
            servers,
            log,
        }
    }
}

#[test]
fn fixtures_serve_real_https_and_redis_tls_with_generated_ca() {
    let (upstream, upstream_thread) = pong_upstream();
    let mut fixture = TestFixture::start(upstream);

    let response = tls_exchange(
        Some(&fixture.ca),
        fixture.servers.https_port(),
        "127.0.0.1",
        b"GET /fixture HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .expect("HTTPS fixture 应完成 TLS 往返");
    assert!(
        response
            .windows(b"ryframe-https-ok".len())
            .any(|window| window == b"ryframe-https-ok"),
        "HTTPS fixture 响应缺少预期正文：{}",
        String::from_utf8_lossy(&response)
    );

    let pong = tls_exchange(
        Some(&fixture.ca),
        fixture.servers.redis_port(),
        "127.0.0.1",
        b"*1\r\n$4\r\nPING\r\n",
    )
    .expect("Redis TLS 代理应完成往返");
    assert!(pong.starts_with(b"+PONG"));

    fixture.servers.stop().expect("停止 TLS fixture");
    upstream_thread.join().expect("等待本地 Redis 上游退出");
    let log = fs::read_to_string(&fixture.log).expect("读取 fixture 日志");
    assert!(log.contains("redis_tcp_accept="));
    assert!(log.contains("https_tcp_accept="));
    assert!(log.contains("fixtures_stopped=1"));
}

#[test]
fn tls_client_rejects_untrusted_certificate_and_wrong_hostname() {
    let upstream = "127.0.0.1:1".parse().expect("解析未使用的回环上游地址");
    let mut fixture = TestFixture::start(upstream);
    let request = b"GET /fixture HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

    let untrusted = tls_exchange(None, fixture.servers.https_port(), "127.0.0.1", request)
        .expect_err("未知 CA 的服务证书必须被拒绝");
    assert!(untrusted.to_lowercase().contains("certificate"));

    let wrong_name = tls_exchange(
        Some(&fixture.ca),
        fixture.servers.https_port(),
        "wrong.example",
        request,
    )
    .expect_err("主机名不匹配的服务证书必须被拒绝");
    assert!(wrong_name.to_lowercase().contains("certificate"));
    fixture.servers.stop().expect("停止 TLS fixture");
}

#[test]
fn redis_probe_rejects_unexpected_protocol_and_remote_hosts() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定错误协议上游");
    let address = listener.local_addr().expect("读取错误协议端口");
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("接受 Redis 探测");
        let mut input = [0_u8; 1024];
        let _ = socket.read(&mut input);
        socket.write_all(b"-WRONG\r\n").expect("返回错误协议响应");
    });
    let endpoint = super::ci::tls::RedisEndpoint {
        host: "127.0.0.1".to_owned(),
        port: address.port(),
    };
    let error = check_redis(&endpoint).expect_err("错误 Redis 响应必须失败");
    assert!(error.to_string().contains("未返回预期"));
    server.join().expect("等待错误协议上游退出");

    let error = redis_endpoint("redis.shared.example", "6379").expect_err("远程 Redis 必须拒绝");
    assert!(error.to_string().contains("只允许连接回环 Redis"));
    let error = redis_endpoint("127.0.0.1", "70000").expect_err("越界端口必须拒绝");
    assert!(error.to_string().contains("RYFRAME_REDIS_PORT"));
}

#[test]
fn missing_openssl_reports_an_actionable_error() {
    let directory = tempdir().expect("创建缺失 OpenSSL 测试目录");
    let missing = directory.path().join("missing-openssl");
    let error = generate_certificates_with_program(
        missing.into_os_string(),
        directory.path(),
        &directory.path().join("openssl.log"),
    )
    .err()
    .expect("缺失 OpenSSL 必须失败");
    let message = error.to_string();
    assert!(message.contains("RYFRAME_OPENSSL"));
    assert!(message.contains("Git for Windows"));
}

#[test]
fn command_runner_reports_error_exit_and_terminates_on_timeout() {
    let directory = tempdir().expect("创建命令运行测试目录");
    let mut exit_log = File::create(directory.path().join("exit.log")).expect("创建退出日志");
    let outcome = run_redirected(exit_command(), &mut exit_log, Duration::from_secs(5))
        .expect("错误退出命令应正常启动");
    match outcome {
        ProcessOutcome::Exited(status) => assert!(!status.success()),
        ProcessOutcome::TimedOut => panic!("错误退出命令不应超时"),
    }

    let mut timeout_log = File::create(directory.path().join("timeout.log")).expect("创建超时日志");
    let started = Instant::now();
    let outcome = run_redirected(
        blocking_command(),
        &mut timeout_log,
        Duration::from_millis(100),
    )
    .expect("阻塞命令应被超时终止");
    assert!(matches!(outcome, ProcessOutcome::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn gate_contract_uses_exact_tests_and_preserves_all_failures() {
    let command = test_command(&TESTS[0], Path::new("target/ci/backend"), 4);
    assert_eq!(command[0], "cargo");
    assert!(!command.iter().any(|argument| argument.contains(".py")));
    assert_eq!(command[command.len() - 2..], ["--exact", "--nocapture"]);
    let mut visited = Vec::new();
    let failures = collect_test_failures(|spec| {
        visited.push(spec.name);
        (spec.name == "s3-https").then(|| "退出码 9".to_owned())
    });
    assert_eq!(visited, ["redis-tls", "s3-https", "otlp-https"]);
    assert_eq!(failures, ["s3-https: 退出码 9"]);
}

#[test]
fn fixture_environment_uses_dynamic_ports_and_explicit_test_ca() {
    let environment = test_environment(
        Path::new("fixture-ca.pem"),
        41001,
        41002,
        Some("15".to_owned()),
    );
    let value = |name: &str| {
        environment
            .values
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
            .expect("缺少 fixture 环境变量")
    };
    assert_eq!(value("RYFRAME_REDIS_HOST"), "127.0.0.1");
    assert_eq!(value("RYFRAME_REDIS_PORT"), "41001");
    assert_eq!(value("RYFRAME_REDIS_DATABASE"), "15");
    assert_eq!(value("SSL_CERT_FILE"), "fixture-ca.pem");
    assert_eq!(
        value("RYFRAME_S3_HTTPS_ENDPOINT"),
        "https://127.0.0.1:41002"
    );
    assert_eq!(
        value("RYFRAME_OTLP_HTTPS_ENDPOINT"),
        "https://127.0.0.1:41002/v1/traces"
    );
    assert!(
        environment
            .removed
            .contains(&"RYFRAME_REDIS_TLS_CLIENT_CERT")
    );
    assert!(
        environment
            .removed
            .contains(&"RYFRAME_REDIS_TLS_CLIENT_KEY")
    );
    assert!(environment.removed.contains(&"SSL_CERT_DIR"));
}

#[test]
fn artifact_directory_is_never_overwritten() {
    let directory = tempdir().expect("创建产物目录测试父目录");
    let first = reserve_output_dir(directory.path(), "same/run").expect("创建首个目录");
    fs::write(first.join("evidence.log"), "first").expect("写入首个证据");
    let second = reserve_output_dir(directory.path(), "same/run").expect("创建第二个目录");
    assert_ne!(first, second);
    assert_eq!(
        fs::read_to_string(first.join("evidence.log")).expect("读取首个证据"),
        "first"
    );
}

fn openssl_program() -> OsString {
    env::var_os("RYFRAME_OPENSSL")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("openssl"))
}

fn pong_upstream() -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定本地 Redis 上游");
    let address = listener.local_addr().expect("读取本地 Redis 上游端口");
    let thread = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("接受 TLS 代理上游连接");
        let mut input = [0_u8; 1024];
        let _ = socket.read(&mut input);
        socket.write_all(b"+PONG\r\n").expect("返回 Redis PONG");
    });
    (address, thread)
}

fn tls_exchange(
    ca: Option<&Path>,
    port: u16,
    server_name: &str,
    request: &[u8],
) -> std::result::Result<Vec<u8>, String> {
    let mut roots = RootCertStore::empty();
    if let Some(ca) = ca {
        roots
            .add(CertificateDer::from_pem_file(ca).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|error| error.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name =
        ServerName::try_from(server_name.to_owned()).map_err(|error| error.to_string())?;
    let connection =
        ClientConnection::new(Arc::new(config), server_name).map_err(|error| error.to_string())?;
    let socket = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let mut stream = StreamOwned::new(connection, socket);
    stream
        .write_all(request)
        .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| error.to_string())?;
    Ok(response)
}

#[cfg(windows)]
fn exit_command() -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "exit", "9"]);
    command
}

#[cfg(not(windows))]
fn exit_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "exit 9"]);
    command
}

#[cfg(windows)]
fn blocking_command() -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "ping", "127.0.0.1", "-n", "10"]);
    command
}

#[cfg(not(windows))]
fn blocking_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 2"]);
    command
}
