//! 独立进程验证文件配置的 OTLP 导出，不让环境负例污染其他测试。

use super::*;
use opentelemetry::{
    KeyValue,
    trace::{Span as _, Tracer as _},
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

type ProbeResult<T> = std::result::Result<T, &'static str>;
type Headers = Vec<(String, String)>;

const PROBE: &str = "telemetry::file_only_tests::environment_probe";
const SUCCESS: &str = "RYFRAME_FILE_ONLY_PROBE_COMPLETED";
const ENV_SECRET: &str = "ryframe_env_secret_c394b01e7f62413c";
const SERVICE: &str = "ryframe_file_service_2fb2e034d3ca";
const SPAN: &str = "ryframe_file_span_734ce01e2f59";
const FIRST: &str = "file_first_attribute_78cbd29e";
const SECOND: &str = "file_second_attribute_b35ef98c";
const THIRD: &str = "file_dropped_attribute_c987ab42";
const MAX_OUTPUT: usize = 128 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;
const SERVER_TIMEOUT: Duration = Duration::from_secs(8);

#[test]
fn telemetry_uses_only_file_configuration_for_real_http_export() {
    let proxy = TcpListener::bind("127.0.0.1:0").expect("绑定宿主代理观察端口");
    proxy.set_nonblocking(true).unwrap();
    let executable =
        std::env::current_exe().unwrap_or_else(|_| panic!("无法定位 telemetry 回归测试程序"));
    let mut command = Command::new(executable);
    command
        .args([
            "--exact",
            PROBE,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    negative_environment(&mut command);
    command
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .env_remove("REQUEST_METHOD");
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(key, format!("http://{}", proxy.local_addr().unwrap()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = ProbeChild(
        command
            .spawn()
            .unwrap_or_else(|_| panic!("无法启动 telemetry 回归子进程")),
    );
    let stdout = capture(
        child
            .0
            .stdout
            .take()
            .unwrap_or_else(|| panic!("缺少子进程标准输出管道")),
    );
    let stderr = capture(
        child
            .0
            .stderr
            .take()
            .unwrap_or_else(|| panic!("缺少子进程错误输出管道")),
    );
    let status = wait_for_child(&mut child.0);
    let reaped = child.stop();
    let stdout = join_finished(stdout, Duration::from_secs(3)).and_then(std::convert::identity);
    let stderr = join_finished(stderr, Duration::from_secs(3)).and_then(std::convert::identity);
    let clean = [&stdout, &stderr].iter().all(|output| {
        output
            .as_ref()
            .is_ok_and(|bytes| bytes.len() <= MAX_OUTPUT && !contains(bytes, ENV_SECRET.as_bytes()))
    });
    let completed = stdout
        .as_ref()
        .is_ok_and(|bytes| contains(bytes, SUCCESS.as_bytes()));
    assert!(
        matches!(proxy.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "产品默认 OTLP 客户端不应访问宿主代理"
    );
    assert!(
        reaped && status.is_ok_and(|status| status.success()) && clean && completed,
        "telemetry 文件配置子进程回归失败、超时或输出不安全；捕获内容不回显"
    );
}

fn negative_environment(command: &mut Command) {
    command
        .env("OTEL_SERVICE_NAME", ENV_SECRET)
        .env(
            "OTEL_RESOURCE_ATTRIBUTES",
            format!("environment.marker={ENV_SECRET}"),
        )
        .env("OTEL_TRACES_SAMPLER", "traceidratio")
        .env("OTEL_TRACES_SAMPLER_ARG", ENV_SECRET);
    for key in [
        "OTEL_ATTRIBUTE_COUNT_LIMIT",
        "OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT",
        "OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT",
        "OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT",
        "OTEL_SPAN_EVENT_COUNT_LIMIT",
        "OTEL_SPAN_LINK_COUNT_LIMIT",
        "OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT",
        "OTEL_LINK_ATTRIBUTE_COUNT_LIMIT",
    ] {
        command.env(key, "0");
    }
    for (key, value) in [
        ("OTEL_BSP_MAX_QUEUE_SIZE", "1"),
        ("OTEL_BSP_MAX_EXPORT_BATCH_SIZE", "1"),
        ("OTEL_BSP_SCHEDULE_DELAY", "86400000"),
        ("OTEL_BSP_EXPORT_TIMEOUT", "1"),
        ("OTEL_BSP_MAX_CONCURRENT_EXPORTS", "1"),
    ] {
        command.env(key, value);
    }
    for signal in ["", "_TRACES"] {
        for (field, value) in [
            ("ENDPOINT", format!("invalid endpoint {ENV_SECRET}")),
            ("PROTOCOL", "grpc".to_owned()),
            ("COMPRESSION", ENV_SECRET.to_owned()),
            ("TIMEOUT", "1".to_owned()),
            ("INSECURE", ENV_SECRET.to_owned()),
            (
                "HEADERS",
                format!("x-env-only={ENV_SECRET},x-file-only={ENV_SECRET}"),
            ),
        ] {
            command.env(format!("OTEL_EXPORTER_OTLP{signal}_{field}"), value);
        }
    }
}

struct ProbeChild(Child);

impl ProbeChild {
    fn stop(&mut self) -> bool {
        let _ = self.0.kill();
        self.0.wait().is_ok()
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wait_for_child(child: &mut Child) -> ProbeResult<std::process::ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(status) = child.try_wait().map_err(|_| "子进程状态读取失败")? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err("子进程执行超时");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn capture(stream: impl Read + Send + 'static) -> JoinHandle<ProbeResult<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        stream
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "子进程输出读取失败")?;
        Ok(bytes)
    })
}

fn join_finished<T>(worker: JoinHandle<T>, timeout: Duration) -> ProbeResult<T> {
    let deadline = Instant::now() + timeout;
    while !worker.is_finished() {
        if Instant::now() >= deadline {
            return Err("回归测试线程未按时结束");
        }
        thread::sleep(Duration::from_millis(10));
    }
    worker.join().map_err(|_| "回归测试线程失败")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "仅由父测试在独立进程设置环境负例后执行"]
async fn environment_probe() {
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::stderr)
        .finish();
    assert!(
        tracing::subscriber::set_global_default(subscriber).is_ok(),
        "无法安装 telemetry 负例日志捕获器"
    );
    let receiver = HttpReceiver::start();
    let config = TelemetryConfig {
        enabled: true,
        endpoint: receiver.endpoint.clone(),
        service_name: SERVICE.into(),
        sample_ratio: 1.0,
        export_timeout_secs: 3,
        headers: [("x-file-only".to_owned(), "configured".to_owned())]
            .into_iter()
            .collect(),
        resource_attributes: [("file.marker".to_owned(), "configured".to_owned())]
            .into_iter()
            .collect(),
        span_limits: ryframe_config::TelemetrySpanLimits {
            max_attributes_per_span: 2,
            ..Default::default()
        },
        ..TelemetryConfig::default()
    };
    let guard = init_tracer_provider(&config);
    let tracer = guard
        .tracer
        .as_ref()
        .unwrap_or_else(|| panic!("文件配置导出器初始化失败"));
    let mut span = tracer.start(SPAN);
    span.set_attribute(KeyValue::new(FIRST, "first_value_17dfb8e2"));
    span.set_attribute(KeyValue::new(SECOND, "second_value_9a03c72d"));
    span.set_attribute(KeyValue::new(THIRD, "third_value_d826a10f"));
    span.end();
    tokio::task::yield_now().await;
    guard.shutdown();
    let request = receiver.finish();
    verify_request(&request);
    println!("{SUCCESS}");
}

struct CapturedRequest {
    headers: Headers,
    body: Vec<u8>,
}

struct HttpReceiver {
    endpoint: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<ProbeResult<CapturedRequest>>>,
}

impl HttpReceiver {
    fn start() -> Self {
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|_| panic!("无法绑定回环 HTTP 接收端"));
        listener
            .set_nonblocking(true)
            .unwrap_or_else(|_| panic!("无法设置 HTTP 接收截止时间"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|_| panic!("无法读取回环 HTTP 接收端口"));
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || receive_request(listener, &stopped));
        Self {
            endpoint: format!("http://{address}/v1/traces"),
            stop,
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> CapturedRequest {
        let worker = self
            .worker
            .take()
            .unwrap_or_else(|| panic!("HTTP 接收线程缺失"));
        join_finished(worker, SERVER_TIMEOUT + Duration::from_secs(2))
            .and_then(std::convert::identity)
            .unwrap_or_else(|_| panic!("未在时限内收到完整 OTLP HTTP 请求"))
    }
}

impl Drop for HttpReceiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = join_finished(worker, Duration::from_secs(2));
        }
    }
}

fn receive_request(listener: TcpListener, stop: &AtomicBool) -> ProbeResult<CapturedRequest> {
    let deadline = Instant::now() + SERVER_TIMEOUT;
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err("HTTP 接收等待结束");
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|_| "HTTP 连接设置失败")?;
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .map_err(|_| "HTTP 读取时限设置失败")?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .map_err(|_| "HTTP 写入时限设置失败")?;
                let request = read_request(&mut stream, stop, deadline)?;
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-protobuf\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .map_err(|_| "HTTP 响应失败")?;
                return Ok(request);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err("HTTP 接收失败"),
        }
    }
}

fn read_request(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    deadline: Instant,
) -> ProbeResult<CapturedRequest> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let end = loop {
        if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break position + 4;
        }
        let remaining = MAX_HEADERS
            .checked_sub(bytes.len())
            .filter(|size| *size > 0)
            .ok_or("HTTP 请求头超限")?;
        let count = read_piece(stream, &mut buffer[..remaining.min(4096)], stop, deadline)?;
        bytes.extend_from_slice(&buffer[..count]);
    };
    let (headers, length) = parse_headers(&bytes[..end])?;
    let expected = end + length;
    if bytes.len() > expected {
        return Err("HTTP 请求长度不符");
    }
    while bytes.len() < expected {
        let count = read_piece(
            stream,
            &mut buffer[..(expected - bytes.len()).min(4096)],
            stop,
            deadline,
        )?;
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(CapturedRequest {
        headers,
        body: bytes[end..].to_vec(),
    })
}

fn parse_headers(bytes: &[u8]) -> ProbeResult<(Headers, usize)> {
    let text = std::str::from_utf8(bytes).map_err(|_| "HTTP 请求头编码错误")?;
    let mut lines = text.split("\r\n");
    if lines.next() != Some("POST /v1/traces HTTP/1.1") {
        return Err("HTTP 导出路径或方法不符");
    }
    let mut headers = Vec::new();
    let mut length = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("HTTP 请求头格式错误")?;
        let name = name.to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "transfer-encoding" {
            return Err("HTTP 请求必须使用确定的 Content-Length");
        }
        if name == "content-length" {
            if length.is_some() {
                return Err("HTTP 请求长度重复");
            }
            length = Some(value.parse::<usize>().map_err(|_| "HTTP 请求长度非法")?);
        }
        headers.push((name, value));
    }
    let length = length
        .filter(|length| (1..=MAX_BODY).contains(length))
        .ok_or("HTTP 请求体缺失或超限")?;
    Ok((headers, length))
}

fn read_piece(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> ProbeResult<usize> {
    loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err("HTTP 请求读取超时");
        }
        match stream.read(buffer) {
            Ok(0) => return Err("HTTP 请求提前结束"),
            Ok(count) => return Ok(count),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return Err("HTTP 请求读取失败"),
        }
    }
}

fn verify_request(request: &CapturedRequest) {
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "x-file-only" && value == "configured"),
        "实际请求缺少文件配置 header"
    );
    assert!(
        !request
            .headers
            .iter()
            .any(|(name, value)| name == "x-env-only"
                || contains(value.as_bytes(), ENV_SECRET.as_bytes())),
        "实际请求包含环境 header"
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/x-protobuf"),
        "实际请求不是 OTLP protobuf"
    );
    for marker in [
        "service.name",
        SERVICE,
        "file.marker",
        "configured",
        SPAN,
        FIRST,
        "first_value_17dfb8e2",
        SECOND,
        "second_value_9a03c72d",
    ] {
        assert!(
            contains(&request.body, marker.as_bytes()),
            "protobuf 缺少文件配置、span 或保留属性"
        );
    }
    for marker in [
        THIRD,
        "third_value_d826a10f",
        ENV_SECRET,
        "environment.marker",
    ] {
        assert!(
            !contains(&request.body, marker.as_bytes()),
            "protobuf 包含环境标记或超限属性"
        );
    }
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|part| part == needle)
}
