use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn api_catalog() -> BTreeMap<String, Value> {
    [
        ("get_auth_csrf", "GET", "/csrf"),
        ("post_auth_login", "POST", "/login"),
        ("get_list", "GET", "/list"),
        ("post_auth_logout", "POST", "/logout"),
    ]
    .into_iter()
    .map(|(name, method, path)| (name.into(), json!({"method":method,"path":path})))
    .collect()
}

async fn concurrent_service(
    count: usize,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let business = Arc::new(AtomicUsize::new(0));
    let observed = business.clone();
    let admitted = Arc::new(tokio::sync::Barrier::new(count));
    let service = tokio::spawn(async move {
        let mut clients = Vec::new();
        for _ in 0..count * 6 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let admitted = admitted.clone();
            let observed = observed.clone();
            clients.push(tokio::spawn(async move {
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let size = socket.read(&mut chunk).await.unwrap();
                    assert_ne!(size, 0);
                    bytes.extend_from_slice(&chunk[..size]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let size = headers.lines().find_map(|line| {
                            line.to_ascii_lowercase().strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        }).unwrap_or(0);
                        if bytes.len() >= end + 4 + size { break; }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                let route = request.split_whitespace().nth(1).unwrap();
                let (status, body) = match route {
                    "/list" => {
                        observed.fetch_add(1, Ordering::SeqCst);
                        // 全部选中身份实际到达后才响应，串行退化不能通过。
                        admitted.wait().await;
                        ("409 Conflict", r#"{"private":"must-not-escape"}"#)
                    }
                    "/login" => ("200 OK", r#"{"code":200,"data":{"access_token":"fixture"}}"#),
                    "/csrf" | "/logout" => ("200 OK", r#"{"code":200}"#),
                    _ => panic!("未登记的请求路径"),
                };
                let response = format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nSet-Cookie: ryframe_csrf=fixture\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }));
        }
        for client in clients {
            client.await.unwrap();
        }
    });
    (address, business, service)
}

#[test]
fn all_selected_clients_remain_concurrent_and_capacity_failures_never_retry() {
    asynchronous(async {
        for count in [10, 50, 100] {
            let temp = tempfile::tempdir().unwrap();
            let (address, business, service) = concurrent_service(count).await;
            let mut config = pool(count as u64);
            config["bindings"]["api_url"] = json!(address);
            config["contract"]["cycles"] = json!(2);
            config["contract"]["timeout_ms"] = json!(10000);
            config["contract"]["workloads"]["api"][0]["steps"] = json!([{"operation":"get_list"}]);
            let catalog = api_catalog();
            let mut input = input(temp.path());
            input.concurrency = count as u64;
            let pacing = pacer(temp.path());
            let cancel = process::Cancellation::default();
            let mut result = tokio::time::timeout(
                Duration::from_secs(20),
                load::run(
                    &config,
                    &catalog,
                    &input,
                    temp.path(),
                    None,
                    &pacing,
                    &cancel,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            tokio::time::timeout(Duration::from_secs(5), service)
                .await
                .unwrap()
                .unwrap();
            result
                .finalize(&config, temp.path(), &cancel)
                .await
                .unwrap();
            assert_eq!(business.load(Ordering::SeqCst), count * 2);
            assert_eq!(result.network_requests, count as u64 * 6);
            assert_eq!(result.completed_cycles, 0);
            assert_eq!(result.failed_cycles, count as u64 * 2);
            assert_eq!(result.session_failures, 0);
            let evidence = fs::read_to_string(temp.path().join("cycles.jsonl")).unwrap();
            assert_eq!(evidence.lines().count(), count * 2);
            assert!(!evidence.contains("must-not-escape"));
            assert!(evidence.lines().all(|line| {
                let row: Value = serde_json::from_str(line).unwrap();
                row["failure"]["category"] == "http_status" && row["failure"]["status"] == 409
            }));
        }
    });
}

#[test]
fn mutation_and_logout_failures_keep_separate_counts_and_sanitized_cycle_evidence() {
    let json_header = "Content-Type: application/json\r\nSet-Cookie: ryframe_csrf=fixture\r\n";
    let (address, _, worker) = server(vec![
        ("200 OK", json_header, r#"{"code":200}"#),
        (
            "200 OK",
            json_header,
            r#"{"code":200,"data":{"access_token":"token"}}"#,
        ),
        (
            "409 Conflict",
            json_header,
            r#"{"private":"secret-business"}"#,
        ),
        ("200 OK", json_header, r#"{"code":200}"#),
        ("200 OK", json_header, r#"{"code":200}"#),
        (
            "500 Internal Server Error",
            json_header,
            r#"{"private":"secret-logout"}"#,
        ),
    ]);
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let mut config = pool(1);
        config["contract"]["cycles"] = json!(2);
        config["contract"]["timeout_ms"] = json!(1000);
        config["bindings"]["api_url"] = json!(address);
        config["contract"]["workloads"]["api"][0]["steps"] = json!([{"operation":"get_list"}]);
        let mut input = input(temp.path());
        input.concurrency = 1;
        let mut result = load::run(
            &config,
            &api_catalog(),
            &input,
            temp.path(),
            None,
            &pacer(temp.path()),
            &process::Cancellation::default(),
        )
        .await
        .unwrap();
        result
            .finalize(&config, temp.path(), &process::Cancellation::default())
            .await
            .unwrap();
        assert_eq!(result.completed_cycles, 1);
        assert_eq!(result.failed_cycles, 1);
        assert_eq!(result.session_failures, 1);
        assert_eq!(result.network_requests, 6);
        let cycles = fs::read_to_string(temp.path().join("cycles.jsonl")).unwrap();
        let events = fs::read_to_string(temp.path().join("session-events.jsonl")).unwrap();
        let rows: Vec<Value> = cycles
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0]["failure"]["status"], 409);
        assert_eq!(rows[0]["failure"]["operation"], "get_list");
        assert_eq!(rows[1]["succeeded"], true);
        let events_rows: Vec<Value> = events
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events_rows[1]["phase"], "logout");
        assert_eq!(events_rows[1]["failure"]["status"], 500);
        for secret in [
            "secret-business",
            "secret-logout",
            "file-only-fixture",
            "bench-0",
        ] {
            assert!(!cycles.contains(secret));
            assert!(!events.contains(secret));
        }
    });
    worker.join().unwrap();
}

#[test]
fn multipart_rejects_ambiguous_unregistered_and_environment_sources_before_http() {
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let mut config = pool(1);
        config["contract"]["timeout_ms"] = json!(1000);
        let catalog = BTreeMap::from([(
            "post_upload".into(),
            json!({"method":"POST","path":"/upload"}),
        )]);
        let identity =
            json!({"tenant_id":"system","username":"fixture","client_address":"198.18.0.1"});
        let mut session = request::Session::new(&config, &catalog, identity, None).unwrap();
        let pacing = pacer(temp.path());
        let cancel = process::Cancellation::default();
        for (source, category) in [
            (
                json!({"sample":"0:0","path":temp.path().join("fixed.xlsx")}),
                "multipart_binding",
            ),
            (json!({}), "multipart_binding"),
            (json!({"sample":"0:0"}), "multipart_sample"),
            (
                json!({"path_env":"OLD_MULTIPART"}),
                "file_configuration_required",
            ),
        ] {
            let step = json!({"operation":"post_upload","multipart":[source]});
            let error = session.request(&step, &pacing, &cancel).await.unwrap_err();
            assert_eq!(error.category, category);
            assert_eq!(session.requests, 0);
        }
    });
}

#[test]
fn each_abba_sample_has_its_own_complete_preparation_window() {
    asynchronous(async {
        for _ in ["warmup-a", "warmup-b", "a", "b", "b", "a"] {
            let temp = tempfile::tempdir().unwrap();
            let pacing = pacing::Pacing::fixture(
                json!({"sample_prepare_wait_ms":10,"authority_sha256":"a".repeat(64)}),
                json!({}),
                temp.path(),
            );
            pacing
                .prepare_sample(&process::Cancellation::default())
                .await
                .unwrap();
            let receipt: Value = serde_json::from_slice(
                &fs::read(temp.path().join("sample-preparation.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(receipt["success"], true);
            assert_eq!(receipt["reason"], "fixed_rate_limit_window");
            assert_eq!(receipt["requested_ms"], 10);
            assert!(receipt["duration_ms"].as_f64().unwrap() >= 10.0);
            assert!(receipt["started_at"].is_string());
            assert!(receipt["finished_at"].is_string());
        }
    });
}
