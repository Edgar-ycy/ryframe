use chrono::{DateTime, Utc};
use ryframe_adapters::backup::runtime_verifier;
use ryframe_application::ports::backup::{
    RestoreBusinessProof, RestorePlan, RestoreRecord, RestoreStatus,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};

#[derive(Clone, Copy)]
struct StubResponse {
    status: u16,
    location: Option<&'static str>,
}

struct LocalHttpFixture {
    address: std::net::SocketAddr,
    stop: mpsc::Sender<()>,
    requests: mpsc::Receiver<String>,
    task: Option<thread::JoinHandle<()>>,
}

impl LocalHttpFixture {
    fn start(routes: &[(&str, u16, Option<&'static str>)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let routes = routes
            .iter()
            .map(|(path, status, location)| {
                (
                    (*path).to_owned(),
                    StubResponse {
                        status: *status,
                        location: *location,
                    },
                )
            })
            .collect();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let (request_sender, request_receiver) = mpsc::channel();
        let task = thread::spawn(move || serve(listener, routes, stop_receiver, request_sender));
        Self {
            address,
            stop: stop_sender,
            requests: request_receiver,
            task: Some(task),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    fn finish(mut self) -> Vec<String> {
        self.stop();
        self.requests.try_iter().collect()
    }

    fn stop(&mut self) {
        let _ = self.stop.send(());
        if let Some(task) = self.task.take() {
            task.join().expect("本机 HTTP fixture 不应失败");
        }
    }
}

impl Drop for LocalHttpFixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = self.stop.send(());
            let _ = self.task.take().map(|task| task.join());
        } else {
            self.stop();
        }
    }
}

fn serve(
    listener: TcpListener,
    routes: BTreeMap<String, StubResponse>,
    stop: mpsc::Receiver<()>,
    requests: mpsc::Sender<String>,
) {
    loop {
        match stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match listener.accept() {
            Ok((stream, _)) => serve_request(stream, &routes, &requests),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("接受本机 HTTP 请求失败：{error}"),
        }
    }
}

fn serve_request(
    mut stream: TcpStream,
    routes: &BTreeMap<String, StubResponse>,
    requests: &mpsc::Sender<String>,
) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let path = read_request_path(&mut stream);
    requests.send(path.clone()).unwrap();
    let response = routes.get(&path).copied().unwrap_or(StubResponse {
        status: 500,
        location: None,
    });
    let reason = match response.status {
        200 => "OK",
        302 => "Found",
        503 => "Service Unavailable",
        _ => "Fixture Error",
    };
    let location = response
        .location
        .map_or_else(String::new, |value| format!("Location: {value}\r\n"));
    write!(
        stream,
        "HTTP/1.1 {} {reason}\r\n{location}Content-Length: 0\r\nConnection: close\r\n\r\n",
        response.status
    )
    .unwrap();
    stream.flush().unwrap();
}

fn read_request_path(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "HTTP 请求头提前结束");
        request.extend_from_slice(&chunk[..read]);
        assert!(request.len() <= 16 * 1024, "HTTP 请求头过大");
    }
    let first_line = std::str::from_utf8(&request)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    first_line.split_whitespace().nth(1).unwrap().to_owned()
}

fn restore_record(api_ready_url: String, worker_ready_url: String) -> RestoreRecord {
    let timestamp = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
    RestoreRecord {
        plan: RestorePlan {
            id: "runtime-drill".into(),
            backup_id: "runtime-backup".into(),
            scope_id: "restored".into(),
            fault_at: timestamp,
            databases: Vec::new(),
            object_endpoint: "local".into(),
            object_prefix: "restored/".into(),
            api_ready_url,
            worker_ready_url,
            frontend_sha: "a".repeat(40),
        },
        plan_hash: "b".repeat(64),
        status: RestoreStatus::DataVerified,
        started_at: timestamp,
        data_verified_at: Some(timestamp),
        completed_at: None,
        recovered_at: timestamp,
        failure: None,
    }
}

fn proof() -> RestoreBusinessProof {
    let timestamp = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();
    RestoreBusinessProof {
        restore_id: "runtime-drill".into(),
        plan_hash: "b".repeat(64),
        backup_source_sha: "c".repeat(40),
        backend_product_sha: "b".repeat(40),
        backend_execution_sha: "b".repeat(40),
        backend_adapter_contract: None,
        frontend_sha: "a".repeat(40),
        runner_sha: "e".repeat(40),
        verifier_sha: "f".repeat(40),
        scope_id: "restored".into(),
        frontend_url: "http://127.0.0.1:4174".into(),
        runtime_receipt_sha256: "d".repeat(64),
        tests_receipt_sha256: "f".repeat(64),
        target_plan_sha256: "e".repeat(64),
        source_generation_sha256: "a".repeat(64),
        dataset_lineage_sha256: "b".repeat(64),
        started_at: timestamp,
        completed_at: timestamp,
        scenarios: Vec::new(),
        unexpected_console_messages: 0,
        unexpected_network_failures: 0,
        axe_serious_or_critical: 0,
    }
}

async fn verify(api: String, worker: String, frontend: String) -> ryframe_kernel::AppResult<()> {
    let record = restore_record(api.clone(), worker.clone());
    let mut proof = proof();
    proof.frontend_url = frontend;
    runtime_verifier(api, worker)?
        .restored_runtime(&record, &proof)
        .await
}

#[tokio::test]
async fn api_worker_and_frontend_must_return_exactly_ok() {
    let fixture = LocalHttpFixture::start(&[
        ("/api/readyz", 200, None),
        ("/worker/readyz", 200, None),
        ("/", 200, None),
    ]);
    verify(
        fixture.url("/api/readyz"),
        fixture.url("/worker/readyz"),
        fixture.url("/"),
    )
    .await
    .unwrap();
    assert_eq!(fixture.finish(), ["/api/readyz", "/worker/readyz", "/"]);
}

#[tokio::test]
async fn any_non_ok_endpoint_fails_closed() {
    for failed in ["/api/readyz", "/worker/readyz", "/"] {
        let routes = [
            (
                "/api/readyz",
                if failed == "/api/readyz" { 503 } else { 200 },
                None,
            ),
            (
                "/worker/readyz",
                if failed == "/worker/readyz" { 503 } else { 200 },
                None,
            ),
            ("/", if failed == "/" { 503 } else { 200 }, None),
        ];
        let fixture = LocalHttpFixture::start(&routes);
        assert!(
            verify(
                fixture.url("/api/readyz"),
                fixture.url("/worker/readyz"),
                fixture.url("/"),
            )
            .await
            .is_err(),
            "{failed}"
        );
        let expected = match failed {
            "/api/readyz" => vec!["/api/readyz".to_owned()],
            "/worker/readyz" => {
                vec!["/api/readyz".to_owned(), "/worker/readyz".to_owned()]
            }
            _ => vec![
                "/api/readyz".to_owned(),
                "/worker/readyz".to_owned(),
                "/".to_owned(),
            ],
        };
        assert_eq!(fixture.finish(), expected, "{failed}");
    }
}

#[tokio::test]
async fn redirect_is_not_followed() {
    let fixture = LocalHttpFixture::start(&[
        ("/api/readyz", 302, Some("/redirected")),
        ("/redirected", 200, None),
        ("/worker/readyz", 200, None),
        ("/", 200, None),
    ]);
    assert!(
        verify(
            fixture.url("/api/readyz"),
            fixture.url("/worker/readyz"),
            fixture.url("/"),
        )
        .await
        .is_err()
    );
    assert_eq!(fixture.finish(), ["/api/readyz"]);
}

#[tokio::test]
async fn plan_endpoint_mismatch_fails_before_network_access() {
    let fixture =
        LocalHttpFixture::start(&[("/api/readyz", 200, None), ("/worker/readyz", 200, None)]);
    let api = fixture.url("/api/readyz");
    let worker = fixture.url("/worker/readyz");
    let verifier = runtime_verifier(api.clone(), worker.clone()).unwrap();
    let record = restore_record(fixture.url("/other"), worker);
    assert!(verifier.restored_runtime(&record, &proof()).await.is_err());
    assert!(fixture.finish().is_empty());
}

#[test]
fn endpoints_reject_invalid_scheme_credentials_query_and_fragment() {
    let valid = "http://127.0.0.1:9/readyz".to_owned();
    for invalid in [
        "not a URL",
        "ftp://127.0.0.1/readyz",
        "http://user:secret@127.0.0.1/readyz",
        "http://127.0.0.1/readyz?probe=true",
        "http://127.0.0.1/readyz#result",
    ] {
        assert!(
            runtime_verifier(invalid.to_owned(), valid.clone()).is_err(),
            "{invalid}"
        );
        assert!(
            runtime_verifier(valid.clone(), invalid.to_owned()).is_err(),
            "{invalid}"
        );
    }
}
