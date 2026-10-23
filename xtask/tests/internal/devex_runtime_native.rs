use super::*;
#[path = "devex_runtime_native_boundaries.rs"]
mod boundaries;
#[path = "devex_runtime_native_evidence.rs"]
mod evidence;
#[path = "devex_runtime_native_imports.rs"]
mod import_requests;
#[path = "devex_runtime_native_migration.rs"]
mod migration;
#[path = "devex_runtime_native_provenance.rs"]
mod provenance_process;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn asynchronous<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
fn input(root: &Path) -> Input {
    Input {
        backend: root.into(),
        frontend: root.into(),
        runner_frontend: root.into(),
        driver: root.into(),
        output: root.join("result.json"),
        suite: "api".into(),
        cache: "warm".into(),
        concurrency: 10,
        input_sha256: "a".repeat(64),
        driver_fingerprint: format!("sha256:{}", "d".repeat(64)),
        runner_frontend_fingerprint: format!("sha256:{}", "e".repeat(64)),
        source_fingerprints: super::super::metadata::SourceFingerprints {
            backend: format!("sha256:{}", "b".repeat(64)),
            frontend: Some(format!("sha256:{}", "f".repeat(64))),
            runner_frontend: Some(format!("sha256:{}", "e".repeat(64))),
        },
        native_tool: super::super::job_evidence::artifact::current_binding().unwrap(),
        app_env: "test".into(),
    }
}
fn pool(count: u64) -> Value {
    json!({"contract":{"cycles":count,"homepage":{"identity_pool":"system","visitor_interval_ms":0},
        "identity_pools":{"system":{"roles_sha256":"a".repeat(64),"permissions_sha256":"b".repeat(64),
            "client_address_model":"fixed-benchmark-per-user","slots":(0..count).map(|index|json!({"tenant_slot":"system","user_slot":format!("user-{index}"),"client_address":format!("198.18.0.{index}")})).collect::<Vec<_>>(),
            "selections":{count.to_string():{"system":count}}}},
        "workloads":{"api":[{"name":"list","identity_pool":"system"}]}},
        "bindings":{"api_url":"http://127.0.0.1:18000","frontend_url":"http://127.0.0.1:18001",
            "identity_pools":{"system":(0..count).map(|index|json!({"tenant_id":"system","username":format!("bench-{index}"),"password":"file-only-fixture"})).collect::<Vec<_>>()}}})
}
fn pacer(directory: &Path) -> pacing::Pacing {
    pacing::Pacing::fixture(
        json!({"request_interval_ms":0,"operation_intervals_ms":{},"sample_prepare_wait_ms":1}),
        json!({"user":{"enabled":true},"api":{"rules":{}}}),
        directory,
    )
}

#[test]
fn native_input_rejects_missing_bindings_and_cold_api() {
    let temp = tempfile::tempdir().unwrap();
    let mut value = input(temp.path());
    assert!(value.validate().is_ok());
    value.cache = "cold".into();
    assert!(value.validate().is_err());
    value.cache = "warm".into();
    value.runner_frontend = "relative".into();
    assert!(value.validate().is_err());
    value.runner_frontend = temp.path().into();
    value.driver_fingerprint = "sha256:missing".into();
    assert!(value.validate().is_err());
}
#[test]
fn native_input_round_trip_requires_typed_source_and_artifact_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let input = input(temp.path());
    let encoded = serde_json::to_value(&input).unwrap();
    let parsed: Input = serde_json::from_value(encoded.clone()).unwrap();
    parsed.validate().unwrap();
    for field in ["native_tool", "source_fingerprints", "input_sha256"] {
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Input>(missing).is_err());
    }
    let mut invalid = encoded.clone();
    invalid["concurrency"] = json!("10");
    assert!(serde_json::from_value::<Input>(invalid).is_err());
    let mut invalid = encoded;
    invalid["source_fingerprints"]["unbound"] = json!("value");
    assert!(serde_json::from_value::<Input>(invalid).is_err());
}

#[test]
fn canonical_hash_pointer_and_expansion_preserve_values() {
    assert_eq!(
        configuration::hash(&json!({"b":1,"a":{"x":2}})).unwrap(),
        configuration::hash(&json!({"a":{"x":2},"b":1})).unwrap()
    );
    let variables = json!({"zero":0,"flag":false,"id":"1/2"});
    assert_eq!(
        configuration::expand(
            &json!({"a":"${zero}","b":"${flag}","c":"x-${id}"}),
            &variables
        )
        .unwrap(),
        json!({"a":0,"b":false,"c":"x-1/2"})
    );
    assert_eq!(
        configuration::pointer(&json!({"a/b":{"~":0}}), "/a~1b/~0").unwrap(),
        &json!(0)
    );
    assert!(configuration::expand(&json!("${missing}"), &variables).is_err());
    assert!(configuration::pointer(&json!({}), "missing").is_err());
}
#[test]
fn identity_receipt_has_no_credentials_or_actual_usernames() {
    let config = pool(10);
    let selected = configuration::selection_config(&config).unwrap();
    let receipt =
        super::super::runtime_selection::validate_suite(&selected, "homepage", 0).unwrap();
    assert!(!receipt.to_string().contains("file-only-fixture"));
    assert!(!receipt.to_string().contains("bench-"));
    let identities = configuration::identities(&config, "homepage", "homepage", 10).unwrap();
    assert_eq!(identities[0]["password"], "file-only-fixture");
    assert!(identities[0].get("password_env").is_none());
    let mut invalid = config;
    invalid["bindings"]["identity_pools"]["system"][1]["username"] = json!("bench-0");
    assert!(configuration::identities(&invalid, "homepage", "homepage", 10).is_err());
}
#[test]
fn identity_file_binding_rejects_environment_and_duplicates() {
    let mut config = pool(10);
    let identity = &mut config["bindings"]["identity_pools"]["system"][0];
    identity.as_object_mut().unwrap().remove("password");
    identity["password_env"] = json!("OLD_SECRET");
    assert!(configuration::selection_config(&config).is_err());
    let mut config = pool(10);
    config["contract"]["identity_pools"]["system"]["slots"][1]["client_address"] =
        json!("198.18.0.0");
    assert!(configuration::identities(&config, "homepage", "homepage", 10).is_err());
}
#[test]
fn output_is_atomic_does_not_overwrite_and_reads_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/evidence.json");
    artifacts::write(&path, &json!({"value":1})).unwrap();
    assert!(artifacts::write(&path, &json!({"value":2})).is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&artifacts::read(&path, 1000).unwrap()).unwrap(),
        json!({"value":1})
    );
    assert!(artifacts::read(&path, 1).is_err());
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}
#[test]
fn failure_categories_do_not_retain_exception_bodies() {
    let value = failure::Failure::request("http_status", "get_system_posts", Some(403));
    assert_eq!(
        value.document(),
        json!({"category":"http_status","operation":"get_system_posts","status":403})
    );
    assert_eq!(
        failure::Failure::request("invalid_response", "get_system_posts", Some(200)).document(),
        json!({"category":"invalid_response","operation":"get_system_posts"})
    );
}
#[test]
fn nearest_rank_and_zero_queue_values_are_preserved() {
    assert_eq!(load::quantile(&[1.0, 100.0, 2.0, 3.0, 4.0], 0.95), 100.0);
    let result = load::summarize(&[10.0, 20.0, 30.0], 2, 1000.0, &[0.0, 20.0], &[2.0, 4.0]);
    assert_eq!(result["completed"], 3);
    assert_eq!(result["failed"], 2);
    assert_eq!(result["throughput"], 3.0);
    assert_eq!(result["queue_p95_ms"], 20.0);
    assert_eq!(result["p99_ms"], 30.0);
}
fn service(cpu: f64, started: f64, memory: f64) -> String {
    format!(
        "ryframe_process_cpu_seconds_total {cpu}\nryframe_process_start_time_seconds {started}\nryframe_process_resident_memory_bytes {memory}\n"
    )
}
fn database(count: &str) -> String {
    format!(
        "mysql_up 1\nmysql_exporter_collector_success{{collector=\"collect.global_status\"}} 1\nmysql_global_status_threads_connected {count}\n"
    )
}
#[test]
fn telemetry_tracks_per_process_deltas_and_resource_peaks() {
    let mut collector = telemetry::Collector::default();
    collector
        .collect(
            &[service(1.0, 10.0, 50.0), service(3.0, 11.0, 70.0)],
            &[database("3")],
        )
        .unwrap();
    collector
        .collect(
            &[service(2.0, 10.0, 30.0), service(6.0, 11.0, 80.0)],
            &[database("5")],
        )
        .unwrap();
    assert_eq!(collector.cpu, 4.0);
    assert_eq!(collector.peak_memory, 120.0);
    assert_eq!(collector.peak_connections, 5.0);
    assert!(
        collector
            .collect(
                &[service(3.0, 10.0, 1.0), service(0.0, 12.0, 1.0)],
                &[database("1")]
            )
            .is_err()
    );
    assert_eq!(collector.cpu, 4.0);
}
#[test]
fn stale_database_and_duplicate_metrics_fail_without_updating() {
    let mut collector = telemetry::Collector::default();
    collector
        .collect(&[service(1.0, 10.0, 1.0)], &[database("3")])
        .unwrap();
    for source in [
        database("3").replace("mysql_up 1", "mysql_up 0"),
        database("3.5"),
        database("NaN"),
        format!("{}mysql_up 1\n", database("3")),
    ] {
        assert!(
            collector
                .collect(&[service(2.0, 10.0, 500.0)], &[source])
                .is_err()
        );
        assert_eq!(collector.peak_memory, 1.0);
    }
    assert_eq!(
        telemetry::metric("m{a=\"x\"} 1\nm{a=\"y\"} 2", "m").unwrap(),
        3.0
    );
    assert!(telemetry::metric("m -1", "m").is_err());
    assert!(telemetry::metric("", "m").is_err());
}
#[test]
fn cancellation_interrupts_wait_and_does_not_publish_success() {
    asynchronous(async {
        let cancel = process::Cancellation::default();
        let trigger = cancel.clone();
        let started = Instant::now();
        let (_, result) = futures_util::future::join(
            async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                trigger.cancel();
            },
            cancel.sleep(Duration::from_secs(5)),
        )
        .await;
        assert_eq!(result.unwrap_err().category, "request_cancelled");
        assert!(started.elapsed() < Duration::from_secs(1));
    });
}
#[test]
fn pacing_shares_clock_per_address_but_keeps_clients_parallel() {
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let pacing = pacing::Pacing::fixture(
            json!({"request_interval_ms":30,"operation_intervals_ms":{}}),
            json!({"user":{"enabled":true},"api":{"rules":{}}}),
            temp.path(),
        );
        let cancel = process::Cancellation::default();
        let operation = json!({"method":"GET","path":"/csrf"});
        let a = json!({"tenant_id":"system","username":"a","client_address":"198.18.0.1"});
        let b = json!({"tenant_id":"system","username":"b","client_address":"198.18.0.2"});
        pacing
            .before(&a, "get_auth_csrf", &operation, &cancel)
            .await
            .unwrap();
        let started = Instant::now();
        let (same, other) = futures_util::future::join(
            pacing.before(&a, "get_auth_csrf", &operation, &cancel),
            pacing.before(&b, "get_auth_csrf", &operation, &cancel),
        )
        .await;
        same.unwrap();
        other.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(30));
        let mut changed = a.clone();
        changed["client_address"] = json!("198.18.0.3");
        assert!(
            pacing
                .before(&changed, "get_auth_csrf", &operation, &cancel)
                .await
                .is_err()
        );
    });
}
#[test]
fn homepage_requires_loopback_and_separate_visitor_identity() {
    let config = pool(10);
    assert_eq!(homepage::visitors(&config).unwrap().len(), 10);
    let mut invalid = config.clone();
    invalid["bindings"]["api_url"] = json!("https://example.test");
    assert!(homepage::visitors(&invalid).is_err());
    let mut invalid = config;
    invalid["contract"]["homepage"]["visitor_interval_ms"] = json!(-1);
    assert!(homepage::visitors(&invalid).is_err());
}
#[test]
fn homepage_failure_counts_once_and_requires_real_lcp_and_network() {
    let identities = configuration::identities(&pool(10), "homepage", "homepage", 10).unwrap();
    let rows=(0..10).map(|index|json!({"cycle":index,"cache":"cold","client_address":format!("198.18.0.{index}"),"phase":"measuring","succeeded":true,"session_failure":false,"measured_requests":3,"diagnostics":[],"times":{"navigation":10,"lcp":20,"content":30}})).collect::<Vec<_>>();
    let mut response = json!({"format_version":1,"kind":"ryframe-devex-homepage","cache":"cold","elapsed_ms":1000,"cycles":rows});
    response["cycles"][0]["succeeded"] = json!(false);
    response["cycles"][0]["session_failure"] = json!(true);
    response["cycles"][0]["phase"] = json!("login");
    response["cycles"][0]["failure"] = json!("timeout");
    let measured = homepage::validate(&response, 10, "cold", &identities).unwrap();
    assert_eq!(measured.failed_cycles, 1);
    assert_eq!(measured.session_failures, 1);
    assert!(measured.scenarios.values().all(|row| row["failed"] == 1));
    response["cycles"][1]["times"]["lcp"] = json!(0);
    assert!(homepage::validate(&response, 10, "cold", &identities).is_err());
}
#[test]
fn exact_job_selection_preserves_tenant_and_rejects_wrong_or_oversized_ids() {
    let workflow = json!({"job":{"kind":"export","id":"${id}","job_type":"system.export.execute"}});
    let value = jobs::selection(
        &workflow,
        &json!({"id":"9223372036854775807","tenant_id":"system"}),
    )
    .unwrap();
    assert_eq!(value["tenant_id"], "system");
    for id in ["0", "1 OR 1=1", "9223372036854775808"] {
        assert!(jobs::selection(&workflow, &json!({"id":id,"tenant_id":"system"})).is_err());
    }
}

type Calls = Arc<Mutex<Vec<String>>>;
fn server<B: AsRef<str> + Send + 'static>(
    replies: Vec<(&'static str, &'static str, B)>,
) -> (String, Calls, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let captured = calls.clone();
    let worker = std::thread::spawn(move || {
        for (status, headers, body) in replies {
            let body = body.as_ref();
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut block = [0; 4096];
            loop {
                let length = stream.read(&mut block).unwrap();
                assert_ne!(length, 0);
                bytes.extend_from_slice(&block[..length]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]);
                    let content = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + content {
                        break;
                    }
                }
            }
            captured
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&bytes).into_owned());
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
                body.len()
            );
        }
    });
    (address, calls, worker)
}
#[test]
fn real_http_binds_cookies_csrf_refresh_origin_and_counts_requests() {
    let (address, calls, worker) = server(vec![
        (
            "200 OK",
            "Content-Type: application/json\r\nSet-Cookie: ryframe_csrf=anonymous\r\n",
            r#"{"code":200}"#,
        ),
        (
            "200 OK",
            "Content-Type: application/json\r\nSet-Cookie: ryframe_refresh_token=refresh; HttpOnly\r\n",
            r#"{"code":200,"data":{"access_token":"first"}}"#,
        ),
        (
            "200 OK",
            "Content-Type: application/json\r\nSet-Cookie: ryframe_csrf=bound\r\n",
            r#"{"code":200}"#,
        ),
        (
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"code":200,"data":{"access_token":"second"}}"#,
        ),
        (
            "200 OK",
            "Content-Type: application/octet-stream\r\n",
            "xlsx",
        ),
        (
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"code":200,"data":{"status":"failed"}}"#,
        ),
    ]);
    asynchronous(async {
        let temp = tempfile::tempdir().unwrap();
        let mut config = pool(10);
        config["contract"]["timeout_ms"] = json!(1000);
        config["bindings"]["api_url"] = json!(address);
        let catalog = BTreeMap::from([
            (
                "get_auth_csrf".into(),
                json!({"method":"GET","path":"/csrf"}),
            ),
            (
                "post_auth_login".into(),
                json!({"method":"POST","path":"/login"}),
            ),
            (
                "post_auth_refresh".into(),
                json!({"method":"POST","path":"/refresh"}),
            ),
            (
                "get_system_users_import_template".into(),
                json!({"method":"GET","path":"/template"}),
            ),
            ("get_job".into(), json!({"method":"GET","path":"/job"})),
        ]);
        let identity = configuration::identities(&config, "homepage", "homepage", 10)
            .unwrap()
            .remove(0);
        let pacing = pacer(temp.path());
        let cancel = process::Cancellation::default();
        let mut session = request::Session::new(&config, &catalog, identity, None).unwrap();
        session.login(&pacing, &cancel).await.unwrap();
        session
            .request(&json!({"operation":"post_auth_refresh"}), &pacing, &cancel)
            .await
            .unwrap();
        let binary = session
            .request(
                &json!({"operation":"get_system_users_import_template","binary":true}),
                &pacing,
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(binary["base64"], "eGxzeA==");
        assert!(
            session
                .request(
                    &json!({"operation":"post_auth_refresh","poll":{}}),
                    &pacing,
                    &cancel
                )
                .await
                .is_err()
        );
        let failed=session.workflow(&json!({"steps":[{"operation":"get_job","poll":{"pointer":"/data/status","equals":"succeeded","failures":["failed"]}}]}),&mut json!({}),&pacing,&cancel).await.unwrap_err();
        assert_eq!(failed.category, "poll_failed");
        assert_eq!(session.requests, 6);
    });
    worker.join().unwrap();
    let calls = calls.lock().unwrap();
    assert!(calls[1].contains("x-csrf-token: anonymous"));
    assert!(calls[3].contains("x-csrf-token: bound"));
    assert!(calls[3].contains("authorization: Bearer first"));
    assert!(calls[3].contains("x-tenant-id: system"));
}

#[test]
fn cycle_success_and_failed_login_use_identical_counting_rules() {
    for login_success in [true, false] {
        let mut replies = vec![(
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"{"code":200}"#,
        )];
        if login_success {
            replies.extend([
                (
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"{"code":200,"data":{"access_token":"token"}}"#,
                ),
                (
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"{"code":200,"data":{"value":0}}"#,
                ),
                (
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"{"code":200,"data":{"value":0}}"#,
                ),
                (
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"{"code":200}"#,
                ),
                (
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"{"code":200}"#,
                ),
            ]);
        } else {
            replies.push((
                "403 Forbidden",
                "Content-Type: application/json\r\n",
                r#"{"secret":"must-not-appear"}"#,
            ));
        }
        let (address, _, worker) = server(replies);
        asynchronous(async {
            let temp = tempfile::tempdir().unwrap();
            let mut config = pool(1);
            config["contract"]["cycles"] = json!(2);
            config["contract"]["timeout_ms"] = json!(1000);
            config["bindings"]["api_url"] = json!(address);
            config["contract"]["workloads"]["api"][0]["steps"] =
                json!([{"operation":"get_list","expect":[{"pointer":"/data/value","equals":0}]}]);
            let catalog = BTreeMap::from([
                (
                    "get_auth_csrf".into(),
                    json!({"method":"GET","path":"/csrf"}),
                ),
                (
                    "post_auth_login".into(),
                    json!({"method":"POST","path":"/login"}),
                ),
                (
                    "post_auth_logout".into(),
                    json!({"method":"POST","path":"/logout"}),
                ),
                ("get_list".into(), json!({"method":"GET","path":"/list"})),
            ]);
            let mut input = input(temp.path());
            input.concurrency = 1;
            let pacing = pacer(temp.path());
            let cancel = process::Cancellation::default();
            let mut measured = load::run(
                &config,
                &catalog,
                &input,
                temp.path(),
                None,
                &pacing,
                &cancel,
            )
            .await
            .unwrap();
            assert!(measured.scenarios.is_empty());
            measured
                .finalize(&config, temp.path(), &cancel)
                .await
                .unwrap();
            assert_eq!(measured.completed_cycles, if login_success { 2 } else { 0 });
            assert_eq!(measured.failed_cycles, if login_success { 0 } else { 2 });
            assert_eq!(measured.session_failures, u64::from(!login_success));
            assert_eq!(measured.network_requests, if login_success { 6 } else { 2 });
            let events = fs::read_to_string(temp.path().join("session-events.jsonl")).unwrap();
            assert!(!events.contains("must-not-appear"));
            if !login_success {
                assert!(events.contains("http_status"));
                assert!(events.contains("403"));
            }
        });
        worker.join().unwrap();
    }
}

#[test]
fn output_preserves_failed_measurement_but_never_accepts_changed_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let input = input(temp.path());
    let path = input.backend.join(".local-tests/devex/runtime.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"before").unwrap();
    let output = json!({"collector_failures":0,"failed_cycles":1,"session_failures":0,"scenarios":{"list":{"completed":0,"failed":1}}});
    assert_eq!(
        publish(
            &input,
            b"before",
            &output,
            &process::Cancellation::default()
        )
        .unwrap(),
        1
    );
    fs::remove_file(&input.output).unwrap();
    fs::write(&path, b"after").unwrap();
    assert_eq!(
        publish(
            &input,
            b"before",
            &output,
            &process::Cancellation::default()
        )
        .unwrap_err()
        .category,
        "runtime_configuration_changed"
    );
    assert!(!input.output.exists());
}

#[test]
fn all_cleanup_failures_remain_visible_alongside_the_original_failure() {
    let error = merge::<()>(
        Err(failure::Failure::new("business_failure")),
        [
            Err(failure::Failure::new("pacing_failure")),
            Err(failure::Failure::new("provenance_failure")),
            Err(failure::Failure::new("artifact_failure")),
        ],
    )
    .unwrap_err();
    assert_eq!(error.category, "business_failure");
    assert_eq!(
        error.secondary,
        vec!["pacing_failure", "provenance_failure", "artifact_failure"]
    );
}

#[test]
fn native_entry_uses_bound_executable_and_stdin_instead_of_node_driver() {
    let temp = tempfile::tempdir().unwrap();
    let input = input(temp.path());
    let command = entry::command(&input).unwrap();
    assert_eq!(command.get_program(), std::env::current_exe().unwrap());
    assert_eq!(command.get_args().collect::<Vec<_>>(), vec![entry::MARKER]);
    assert!(!format!("{command:?}").contains("runtime.mjs"));
}
