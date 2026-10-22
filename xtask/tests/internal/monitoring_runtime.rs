use super::process_liveness::process_is_running;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::{
    recovery::monitoring::{
        config,
        contract::{Binding, ROLES, process_identity, read_document},
        http, lifecycle, observe, receipts, repository, staging, supervisor, webhook,
    },
    workspace::root_dir,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[path = "monitoring_runtime_failure.rs"]
mod failure;

#[path = "monitoring_runtime_cleanup.rs"]
mod cleanup;
#[path = "monitoring_runtime_driver.rs"]
mod driver;

use driver::FakeRuntime;

#[derive(Clone)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct MockHttp {
    url: String,
}

fn mock_http(
    requests: usize,
    mut handler: impl FnMut(HttpRequest) -> (u16, &'static str, Vec<u8>) + Send + 'static,
) -> MockHttp {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let (status, content_type, body) = handler(request);
            write_http_response(&mut stream, status, content_type, &body);
        }
    });
    MockHttp {
        url: format!("http://127.0.0.1:{port}"),
    }
}

fn read_http_request(stream: &mut TcpStream) -> HttpRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut raw = Vec::new();
    let mut block = [0_u8; 4096];
    loop {
        let read = stream.read(&mut block).unwrap();
        assert!(read > 0);
        raw.extend_from_slice(&block[..read]);
        if let Some(end) = raw.windows(4).position(|value| value == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&raw[..end]).unwrap().to_owned();
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                .unwrap_or(0);
            while raw.len() < end + 4 + length {
                let read = stream.read(&mut block).unwrap();
                assert!(read > 0);
                raw.extend_from_slice(&block[..read]);
            }
            let mut lines = headers.lines();
            let mut request_line = lines.next().unwrap().split_whitespace();
            let method = request_line.next().unwrap().to_owned();
            let path = request_line.next().unwrap().to_owned();
            let headers = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
                .collect();
            return HttpRequest {
                method,
                path,
                headers,
                body: raw[end + 4..end + 4 + length].to_vec(),
            };
        }
    }
}

fn write_http_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
}

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let directory = root_dir().join(".local-tests").join(format!(
            "monitoring-runtime-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("监控测试失败现场保留在 {}", self.directory.display());
            return;
        }
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

fn metric_binding(api: &str, worker: &str) -> Value {
    json!({
        "scope_id": "restore-scope",
        "authority": {
            "endpoints": {
                "api_metrics": api,
                "worker_metrics": worker,
            }
        },
        "endpoints": {
            "prometheus": "http://127.0.0.1:29090",
            "alertmanager": "http://127.0.0.1:29091",
            "webhook": "http://127.0.0.1:29092",
        },
        "ports": {"prometheus": 29090, "alertmanager": 29091, "webhook": 29092},
    })
}

#[test]
fn metrics_require_bearer_and_persist_only_the_accepted_body() {
    let fixture = Fixture::new("metrics");
    let evidence = fixture.directory.join("evidence");
    fs::create_dir(&evidence).unwrap();
    let server = mock_http(6, |request| {
        match request.headers.get("authorization").map(String::as_str) {
            None => (401, "text/plain", Vec::new()),
            Some("Bearer ryframe-invalid-probe") => (401, "text/plain", Vec::new()),
            Some("Bearer secret-token") => (
                200,
                "text/plain; version=0.0.4",
                b"# TYPE ryframe_probe gauge\nryframe_probe 1\nprocess_open_fds 2\n".to_vec(),
            ),
            Some(_) => (403, "text/plain", Vec::new()),
        }
    });
    let binding = metric_binding(&server.url, &server.url);
    let result =
        observe::verify_metrics(&fixture.directory, &binding, "secret-token", &evidence).unwrap();
    assert_eq!(result["api"]["missing_status"], 401);
    assert_eq!(result["api"]["invalid_status"], 401);
    assert_eq!(result["api"]["accepted_status"], 200);
    assert_eq!(result["api"]["sample_count"], 2);
    assert_eq!(result["api"]["ryframe_sample_count"], 1);
    assert_eq!(
        fs::read(evidence.join("api-metrics.txt")).unwrap(),
        b"# TYPE ryframe_probe gauge\nryframe_probe 1\nprocess_open_fds 2\n"
    );
    assert!(
        !fs::read_to_string(evidence.join("api-metrics.txt"))
            .unwrap()
            .contains("secret-token")
    );
}

#[test]
fn metrics_reject_wrong_token_and_unexpected_http_status() {
    let fixture = Fixture::new("metrics-failure");
    let evidence = fixture.directory.join("evidence");
    fs::create_dir(&evidence).unwrap();
    let server = mock_http(3, |request| {
        if request.headers.get("authorization").map(String::as_str) == Some("Bearer correct") {
            (500, "text/plain", b"wrong".to_vec())
        } else {
            (401, "text/plain", Vec::new())
        }
    });
    let binding = metric_binding(&server.url, &server.url);
    let error =
        observe::verify_metrics(&fixture.directory, &binding, "correct", &evidence).unwrap_err();
    assert!(error.to_string().contains("Bearer"));
    assert!(!evidence.join("api-metrics.txt").exists());
}

#[test]
fn prometheus_evidence_uses_mock_loopback_and_rejects_invalid_json() {
    let fixture = Fixture::new("prometheus");
    let evidence = fixture.directory.join("evidence");
    fs::create_dir(&evidence).unwrap();
    let targets = json!({
        "status": "success",
        "data": {"activeTargets": [
            {"labels": {"job": "ryframe-api"}, "health": "up",
             "scrapeUrl": "http://127.0.0.1:18080/api/v1/monitor/metrics", "lastError": ""},
            {"labels": {"job": "ryframe-worker"}, "health": "up",
             "scrapeUrl": "http://127.0.0.1:19091/metrics", "lastError": ""}
        ]}
    });
    let rules = json!({
        "status": "success",
        "data": {"groups": [{"rules": [
            {"name": "RyFrameBackupAging"},
            {"name": "RyFrameBackupStale"},
            {"name": "RyFrameBackupMissing"},
            {"name": "RyFrameBackupExpired"},
            {"name": "RyFrameBackupInvalid"},
            {"name": "RyFrameBackupMetricMissing"},
            {"name": "RyFrameRestoreFailed"},
            {"name": "RyFrameRestoreOverdue"}
        ]}]}
    });
    let server = mock_http(10, move |request| {
        if request.path == "/api/v1/targets" {
            (
                200,
                "application/json",
                serde_json::to_vec(&targets).unwrap(),
            )
        } else {
            (200, "application/json", serde_json::to_vec(&rules).unwrap())
        }
    });
    let mut binding = metric_binding(
        "http://127.0.0.1:18080/api/v1/monitor/metrics",
        "http://127.0.0.1:19091/metrics",
    );
    binding["endpoints"]["prometheus"] = json!(server.url);
    binding["rules"] = json!({
        "alerts": {"path": root_dir().join("deploy/prometheus/ryframe-alerts.yml")},
        "tests": {"path": root_dir().join("deploy/prometheus/ryframe-alerts.test.yml")}
    });
    let result = observe::verify_prometheus_with_timeout(
        &fixture.directory,
        &binding,
        &evidence,
        Duration::from_millis(250),
    )
    .unwrap();
    assert_eq!(result["targets"].as_object().unwrap().len(), 2);
    assert_eq!(result["alerts"].as_array().unwrap().len(), 8);
    assert!(evidence.join("prometheus-targets.json").is_file());

    let bad = mock_http(3, |_| (200, "application/json", b"{".to_vec()));
    binding["endpoints"]["prometheus"] = json!(bad.url);
    assert!(
        observe::verify_prometheus_with_timeout(
            &fixture.directory,
            &binding,
            &evidence,
            Duration::from_millis(100),
        )
        .is_err()
    );
}

#[test]
fn alert_delivery_persists_exact_firing_and_resolved_events() {
    let fixture = Fixture::new("delivery");
    let events = fixture.directory.join("events.jsonl");
    fs::write(&events, b"").unwrap();
    let events_for_server = events.clone();
    let calls = Arc::new(Mutex::new(0_u32));
    let calls_for_server = Arc::clone(&calls);
    let server = mock_http(2, move |request| {
        assert_eq!(request.method, "POST");
        let payload: Value = serde_json::from_slice(&request.body).unwrap();
        let status = {
            let mut calls = calls_for_server.lock().unwrap();
            *calls += 1;
            if *calls == 1 { "firing" } else { "resolved" }
        };
        let mut alert = payload[0].clone();
        alert["status"] = json!(status);
        let event = json!({
            "received_at": "2026-09-12T00:00:00.000000Z",
            "remote": "127.0.0.1",
            "payload": {
                "receiver": "ryframe-local-webhook",
                "status": status,
                "alerts": [alert],
            }
        });
        let mut encoded = serde_json::to_vec(&event).unwrap();
        encoded.push(b'\n');
        fs::OpenOptions::new()
            .append(true)
            .open(&events_for_server)
            .unwrap()
            .write_all(&encoded)
            .unwrap();
        (200, "application/json", Vec::new())
    });
    let binding = json!({
        "scope_id": "restore-scope",
        "endpoints": {
            "prometheus": "http://127.0.0.1:29090",
            "alertmanager": server.url,
            "webhook": "http://127.0.0.1:29092",
        }
    });
    let result =
        observe::verify_delivery_with_timeout(&binding, &events, Duration::from_secs(2)).unwrap();
    assert_eq!(result["firing"], true);
    assert_eq!(result["resolved"], true);
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[test]
fn alert_delivery_rejects_http_error_and_unknown_event() {
    let fixture = Fixture::new("delivery-failure");
    let events = fixture.directory.join("events.jsonl");
    fs::write(&events, b"").unwrap();
    let server = mock_http(1, |_| (503, "application/json", Vec::new()));
    let mut binding = json!({
        "scope_id": "restore-scope",
        "endpoints": {
            "prometheus": "http://127.0.0.1:29090",
            "alertmanager": server.url,
            "webhook": "http://127.0.0.1:29092",
        }
    });
    assert!(
        observe::verify_delivery_with_timeout(&binding, &events, Duration::from_millis(100))
            .is_err()
    );

    let server = mock_http(1, |_| (200, "application/json", Vec::new()));
    binding["endpoints"]["alertmanager"] = json!(server.url);
    let started = Instant::now();
    assert!(
        observe::verify_delivery_with_timeout(&binding, &events, Duration::from_millis(150))
            .is_err()
    );
    assert!(started.elapsed() >= Duration::from_millis(100));
}

#[test]
fn ready_wait_times_out_when_no_listener_exists() {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let started = Instant::now();
    assert!(
        config::wait_ready(
            &format!("http://127.0.0.1:{port}/readyz"),
            Duration::from_millis(120),
        )
        .is_err()
    );
    assert!(started.elapsed() >= Duration::from_millis(80));
}

#[test]
fn native_webhook_serves_readiness_appends_json_and_rejects_other_media() {
    let fixture = Fixture::new("webhook");
    let events = fixture.directory.join("events.jsonl");
    fs::write(&events, b"").unwrap();
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let protocol = webhook::Protocol {
        host: "127.0.0.1".into(),
        port,
        events: events.clone(),
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__monitoring-webhook")
        .arg(protocol.encode().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let identity = super::recovery::clone::native::resources::common::process_identity(child.id())
        .unwrap()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    config::wait_ready(&format!("{base}/readyz"), Duration::from_secs(5)).unwrap();
    let rejected = http::request(
        &format!("{base}/alerts"),
        &[("Content-Type", "text/plain")],
        Some(br#"{"status":"firing"}"#),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(rejected.status, 415);
    let accepted = http::request(
        &format!("{base}/alerts"),
        &[("Content-Type", "application/json")],
        Some(br#"{"receiver":"ryframe-local-webhook","status":"firing","alerts":[]}"#),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(accepted.status, 204);
    let line = fs::read_to_string(&events).unwrap();
    let event: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(event["remote"], "127.0.0.1");
    assert_eq!(event["payload"]["receiver"], "ryframe-local-webhook");
    super::recovery::clone::native::resources::common::terminate_tree(&identity).unwrap();
    let _ = child.wait();
}

#[cfg(windows)]
#[test]
fn supervisor_registers_stops_and_reaps_a_short_lived_process_tree() {
    let fixture = Fixture::new("supervisor");
    let runtime = fixture.directory.join("processes");
    fs::create_dir(&runtime).unwrap();
    let operation = "1".repeat(32);
    let system_root = std::env::var_os("SystemRoot").unwrap();
    let powershell = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let protocol = supervisor::Protocol {
        runtime_directory: runtime.clone(),
        role: "webhook".into(),
        scope_id: "restore-scope".into(),
        operation_id: operation.clone(),
        command: vec![
            powershell.to_string_lossy().into_owned(),
            "-NoProfile".into(),
            "-Command".into(),
            "Start-Sleep -Seconds 60".into(),
        ],
        cwd: fixture.directory.clone(),
        environment: minimal_windows_environment(),
        log: runtime.join("webhook.log"),
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__monitoring-supervisor")
        .env(supervisor::ENVIRONMENT_KEY, protocol.encode().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let tree = wait_tree(&runtime);
    let product_pid = tree["process"]["pid"].as_u64().unwrap() as u32;
    let supervisor_pid = tree["supervisor"]["pid"].as_u64().unwrap() as u32;
    assert!(process_identity(&tree["process"], "产品进程").is_ok());
    supervisor::request_stop(&tree, false).unwrap();
    let proof = supervisor::wait_stopped(&tree, Duration::from_secs(10)).unwrap();
    assert_eq!(proof["status"], "stopped");
    assert!(!process_is_running(product_pid));
    assert!(!process_is_running(supervisor_pid));
    let _ = child.wait();
    assert!(
        runtime
            .join(format!("webhook-members-{operation}-stopped.json"))
            .is_file()
    );
}

#[cfg(windows)]
#[test]
fn supervisor_records_natural_exit_and_missing_process_without_faking_success() {
    let fixture = Fixture::new("supervisor-natural");
    let runtime = fixture.directory.join("processes");
    fs::create_dir(&runtime).unwrap();
    let system_root = std::env::var_os("SystemRoot").unwrap();
    let powershell = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let protocol = supervisor::Protocol {
        runtime_directory: runtime.clone(),
        role: "prometheus".into(),
        scope_id: "restore-scope".into(),
        operation_id: "2".repeat(32),
        command: vec![
            powershell.to_string_lossy().into_owned(),
            "-NoProfile".into(),
            "-Command".into(),
            "Start-Sleep -Milliseconds 150".into(),
        ],
        cwd: fixture.directory.clone(),
        environment: minimal_windows_environment(),
        log: runtime.join("prometheus.log"),
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__monitoring-supervisor")
        .env(supervisor::ENVIRONMENT_KEY, protocol.encode().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let tree = wait_tree(&runtime);
    let proof = supervisor::wait_stopped(&tree, Duration::from_secs(10)).unwrap();
    assert_eq!(proof["status"], "stopped");
    assert!(tree["process"]["pid"].as_u64().is_some());
    assert!(!process_is_running(
        tree["process"]["pid"].as_u64().unwrap() as u32
    ));
    let _ = child.wait();
}

#[cfg(windows)]
fn minimal_windows_environment() -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    for name in [
        "SystemRoot",
        "WINDIR",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERNAME",
    ] {
        if let Ok(value) = std::env::var(name) {
            environment.insert(name.into(), value);
        }
    }
    environment
}

fn wait_tree(runtime: &Path) -> Value {
    let path = runtime.join("webhook-tree.json");
    let fallback = runtime.join("prometheus-tree.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let path = if path.exists() { &path } else { &fallback };
        if path.exists() {
            return serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        }
        assert!(Instant::now() < deadline, "supervisor 未发布进程树");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn staging_is_create_only_hides_token_and_rejects_unknown_members() {
    let fixture = Fixture::new("staging");
    let backend = fixture.directory.clone();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let rules_dir = backend.join("deploy").join("prometheus");
    fs::create_dir_all(&rules_dir).unwrap();
    fs::copy(
        root_dir().join("deploy/prometheus/ryframe-alerts.yml"),
        rules_dir.join("ryframe-alerts.yml"),
    )
    .unwrap();
    fs::copy(
        root_dir().join("deploy/prometheus/ryframe-alerts.test.yml"),
        rules_dir.join("ryframe-alerts.test.yml"),
    )
    .unwrap();
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "monitoring@example.invalid"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Monitoring Test"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["add", "deploy/prometheus"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "--quiet", "-m", "fixture"])
        .current_dir(&backend)
        .status()
        .unwrap();
    let run = local.join("run");
    fs::create_dir_all(&run).unwrap();
    let token = run.join("metrics-token.txt");
    fs::write(&token, b"top-secret-token\n").unwrap();
    let credential =
        staging::read_credential_with_security(&token, |_| Ok(json!({"platform": "fixture"})))
            .unwrap();
    let tools_dir = local.join("tools");
    fs::create_dir(&tools_dir).unwrap();
    let tool_source = std::env::current_exe().unwrap();
    let mut tools = BTreeMap::new();
    let mut versions = BTreeMap::new();
    for (name, version) in [
        ("prometheus", "prometheus, version 3.5.0"),
        ("promtool", "promtool, version 3.5.0"),
        ("alertmanager", "alertmanager, version 0.34.0"),
        ("amtool", "amtool, version 0.34.0"),
    ] {
        let path = tools_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fs::copy(&tool_source, &path).unwrap();
        tools.insert(name.to_owned(), path);
        versions.insert(name.to_owned(), version.to_owned());
    }
    let coordinator = json!({
        "root": backend.to_string_lossy(),
        "head": "a".repeat(40),
        "inventory_sha256": "b".repeat(64),
    });
    let manifest =
        staging::create(&backend, &run, &coordinator, &credential, &tools, &versions).unwrap();
    let staged = staging::verify_without_tools(&run, &manifest, &coordinator, &credential).unwrap();
    assert_eq!(staged.files.len(), 6);
    let raw = fs::read_to_string(run.join("input-staging/manifest.json")).unwrap();
    assert!(!raw.contains("top-secret-token"));
    assert!(
        staging::create(&backend, &run, &coordinator, &credential, &tools, &versions,).is_err()
    );
    fs::write(run.join("input-staging/unknown.txt"), b"unknown").unwrap();
    assert!(staging::verify_without_tools(&run, &manifest, &coordinator, &credential).is_err());
}

#[test]
fn generated_configs_bind_loopback_receiver_rules_and_credential_file() {
    let fixture = Fixture::new("configs");
    let configs = fixture.directory.join("configs");
    fs::create_dir(&configs).unwrap();
    let binding = json!({
        "authority": {"endpoints": {
            "api_metrics": "http://127.0.0.1:18080/api/v1/monitor/metrics",
            "worker_metrics": "http://127.0.0.1:19091/metrics",
        }},
        "endpoints": {
            "alertmanager": "http://127.0.0.1:29091",
            "webhook": "http://127.0.0.1:29092",
        },
        "credential": {"path": fixture.directory.join("metrics-token.txt")},
    });
    let paths = BTreeMap::from([
        ("rules".into(), configs.join("ryframe-alerts.yml")),
        ("prometheus".into(), configs.join("prometheus.yml")),
        ("alertmanager".into(), configs.join("alertmanager.yml")),
    ]);
    let generated = config::generated_configs(&binding, &paths).unwrap();
    assert!(generated["prometheus"].contains("127.0.0.1:18080"));
    assert!(generated["prometheus"].contains("metrics-token.txt"));
    assert!(generated["alertmanager"].contains("ryframe-local-webhook"));
    assert!(generated["alertmanager"].contains("http://127.0.0.1:29092/alerts"));
    assert!(!generated["prometheus"].contains("secret"));
    assert!(!generated["alertmanager"].contains("external.example"));
}

#[test]
fn rule_binding_requires_tracked_head_bytes_and_all_fixed_alerts() {
    let fixture = Fixture::new("rules");
    let backend = fixture.directory.clone();
    let rules = backend.join("deploy").join("prometheus");
    fs::create_dir_all(&rules).unwrap();
    for name in ["ryframe-alerts.yml", "ryframe-alerts.test.yml"] {
        fs::copy(
            root_dir().join("deploy/prometheus").join(name),
            rules.join(name),
        )
        .unwrap();
    }
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "monitoring@example.invalid"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Monitoring Test"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["add", "deploy/prometheus"])
        .current_dir(&backend)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "--quiet", "-m", "rules"])
        .current_dir(&backend)
        .status()
        .unwrap();
    let (receipt, documents) = repository::bind_rules(&backend).unwrap();
    assert_eq!(receipt.as_object().unwrap().len(), 2);
    assert_eq!(documents.len(), 2);
    let alerts = rules.join("ryframe-alerts.yml");
    let mut changed = fs::read_to_string(&alerts).unwrap();
    changed = changed.replacen(
        "alert: RyFrameBackupAging",
        "alert: RyFrameBackupAgingChanged",
        1,
    );
    fs::write(&alerts, changed).unwrap();
    assert!(repository::bind_rules(&backend).is_err());
}

fn lifecycle_binding(fixture: &Fixture) -> Binding {
    let run = fixture.directory.join("run");
    let staging = run.join("input-staging");
    fs::create_dir_all(staging.join("deploy/prometheus")).unwrap();
    let token = run.join("metrics-token.txt");
    fs::write(&token, b"secret-token\n").unwrap();
    let alerts = root_dir().join("deploy/prometheus/ryframe-alerts.yml");
    let tests = root_dir().join("deploy/prometheus/ryframe-alerts.test.yml");
    fs::copy(
        &alerts,
        staging.join("deploy/prometheus/ryframe-alerts.yml"),
    )
    .unwrap();
    fs::copy(
        &tests,
        staging.join("deploy/prometheus/ryframe-alerts.test.yml"),
    )
    .unwrap();
    let product_ports = [free_port(), free_port(), free_port()];
    let monitor_ports = [free_port(), free_port(), free_port()];
    let binding_path = run.join("binding.json");
    let value = json!({
        "format_version": 1,
        "kind": "restore-monitoring-binding",
        "status": "bound",
        "run_id": "monitor-runtime-test",
        "scope_id": "restore-scope",
        "authority": {
            "endpoints": {
                "api_ready": format!("http://127.0.0.1:{}/readyz", product_ports[0]),
                "worker_ready": format!("http://127.0.0.1:{}/readyz", product_ports[1]),
                "frontend": format!("http://127.0.0.1:{}", product_ports[2]),
                "api_metrics": format!("http://127.0.0.1:{}/api/v1/monitor/metrics", product_ports[0]),
                "worker_metrics": format!("http://127.0.0.1:{}/metrics", product_ports[1]),
            },
            "restore": {"scope_id": "restore-scope"},
        },
        "environment": {"path": fixture.directory.join("environment.json"), "bytes": 2, "sha256": "1".repeat(64)},
        "staging": {"path": staging.join("manifest.json"), "bytes": 2, "sha256": "2".repeat(64)},
        "credential": {"path": token},
        "tools": {},
        "rules": {
            "alerts": staging::descriptor(&alerts).unwrap(),
            "tests": staging::descriptor(&tests).unwrap(),
        },
        "endpoints": {
            "prometheus": format!("http://127.0.0.1:{}", monitor_ports[0]),
            "alertmanager": format!("http://127.0.0.1:{}", monitor_ports[1]),
            "webhook": format!("http://127.0.0.1:{}", monitor_ports[2]),
        },
        "ports": {
            "prometheus": monitor_ports[0],
            "alertmanager": monitor_ports[1],
            "webhook": monitor_ports[2],
        },
        "contact_policy": {"loopback_only": true, "send_resolved": true, "external_receivers": []},
        "allowed_writes": ["alertmanager-data", "configs", "evidence", "processes", "prometheus-data"],
        "created_at": "2026-10-18T00:00:00Z",
    });
    fs::write(&binding_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    Binding {
        document: read_document(&binding_path, "测试监控绑定").unwrap(),
        value,
        run,
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn powershell_sleep_command() -> Vec<String> {
    let system_root = std::env::var_os("SystemRoot").unwrap();
    vec![
        PathBuf::from(system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe")
            .to_string_lossy()
            .into_owned(),
        "-NoProfile".into(),
        "-Command".into(),
        "Start-Sleep -Seconds 60".into(),
    ]
}

#[cfg(windows)]
#[test]
fn start_publishes_last_and_close_stops_all_roles_in_reverse_order() {
    let fixture = Fixture::new("start-close");
    let binding = lifecycle_binding(&fixture);
    let command = powershell_sleep_command();
    let runtime = FakeRuntime {
        commands: ROLES
            .into_iter()
            .map(|role| (role.to_owned(), command.clone()))
            .collect(),
        fail_role: None,
        supervisor_executable: PathBuf::from(env!("CARGO_BIN_EXE_xtask")),
    };
    let start = lifecycle::start_runtime(&binding, &runtime).unwrap();
    assert_eq!(start["status"], "running");
    assert!(binding.run.join("start.json").is_file());
    assert_eq!(
        start["processes"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        ROLES.iter().map(|value| (*value).to_owned()).collect()
    );
    let observations = ROLES
        .into_iter()
        .map(|role| {
            let pid = start["processes"][role]["identity"]["pid"]
                .as_u64()
                .unwrap() as u32;
            (role, pid)
        })
        .collect::<Vec<_>>();
    let close = lifecycle::close_runtime(&binding).unwrap();
    assert_eq!(close["status"], "closed");
    assert_eq!(close["completions"].as_object().unwrap().len(), ROLES.len());
    for (role, pid) in observations {
        assert!(
            !super::process_liveness::process_is_running(pid),
            "{role} 仍在运行"
        );
        let operation = start["processes"][role]["operation_id"].as_str().unwrap();
        assert!(
            binding
                .run
                .join("processes")
                .join(format!("{role}-members-{operation}-stopped.json"))
                .is_file()
        );
    }
    let before = fs::read(binding.run.join("close.json")).unwrap();
    assert!(lifecycle::close_runtime(&binding).is_err());
    assert_eq!(fs::read(binding.run.join("close.json")).unwrap(), before);
    fs::write(binding.run.join("unknown.txt"), b"unknown").unwrap();
    assert!(super::recovery::monitoring::contract::validate_root_shape(&binding.run).is_err());
}

#[cfg(windows)]
#[test]
fn start_failure_reaps_already_started_trees_and_never_publishes_success() {
    let fixture = Fixture::new("start-failure");
    let binding = lifecycle_binding(&fixture);
    let command = powershell_sleep_command();
    let runtime = FakeRuntime {
        commands: ROLES
            .into_iter()
            .map(|role| (role.to_owned(), command.clone()))
            .collect(),
        fail_role: Some("alertmanager".into()),
        supervisor_executable: PathBuf::from(env!("CARGO_BIN_EXE_xtask")),
    };
    let error = lifecycle::start_runtime(&binding, &runtime).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("fixture alertmanager launch failure")
    );
    assert!(!binding.run.join("start.json").exists());
    let failure = read_document(&binding.run.join("start-failure.json"), "启动失败收据").unwrap();
    assert_eq!(failure.value["status"], "failed");
    assert_eq!(failure.value["kind"], "restore-monitoring-start-failure");
    assert_eq!(
        failure.value["completions"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["webhook".to_owned()])
    );
    assert!(config::verify_ports_closed(&binding.value).is_ok());
}

#[cfg(windows)]
#[test]
fn observation_failure_reaps_all_registered_trees_and_records_failure() {
    let fixture = Fixture::new("observe-failure");
    let binding = lifecycle_binding(&fixture);
    let command = powershell_sleep_command();
    let runtime = FakeRuntime {
        commands: ROLES
            .into_iter()
            .map(|role| (role.to_owned(), command.clone()))
            .collect(),
        fail_role: None,
        supervisor_executable: PathBuf::from(env!("CARGO_BIN_EXE_xtask")),
    };
    lifecycle::start_runtime(&binding, &runtime).unwrap();
    let (start_document, start, _intent, operations) =
        receipts::read_start(&binding.run, &binding.value, &binding.document.descriptor()).unwrap();
    let error = observe::observe_runtime(
        &fixture.directory,
        &binding,
        &start_document,
        &start,
        &operations,
    )
    .unwrap_err();
    assert!(error.to_string().contains("监听") || error.to_string().contains("进程身份"));
    assert!(!binding.run.join("observation.json").exists());
    let failure = read_document(
        &binding.run.join("observation-failure.json"),
        "观测失败收据",
    )
    .unwrap();
    assert_eq!(failure.value["status"], "failed");
    assert_eq!(
        failure.value["completions"].as_object().unwrap().len(),
        ROLES.len()
    );
    for role in ROLES {
        let pid = start["processes"][role]["identity"]["pid"]
            .as_u64()
            .unwrap() as u32;
        assert!(
            !super::process_liveness::process_is_running(pid),
            "{role} 仍在运行"
        );
    }
    assert!(config::verify_ports_closed(&binding.value).is_ok());
}

#[test]
fn occupied_monitor_port_is_not_reported_closed() {
    let fixture = Fixture::new("occupied-port");
    let binding = lifecycle_binding(&fixture);
    let listener = TcpListener::bind((
        "127.0.0.1",
        binding.value["ports"]["webhook"].as_u64().unwrap() as u16,
    ))
    .unwrap();
    assert!(config::verify_ports_closed(&binding.value).is_err());
    drop(listener);
    assert!(config::verify_ports_closed(&binding.value).is_ok());
}
