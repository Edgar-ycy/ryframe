use super::*;
use crate::cli::RuntimeSources;

fn authority() -> Value {
    json!({
        "format_version":2, "kind":"restore-runtime-authority", "restore_id":"restore-1",
        "backup_id":"backup-1", "scope_id":"fixture-restore-1", "plan_hash":"a".repeat(64),
        "data_verified_at":"2026-09-06T00:00:00Z", "backup_source_sha":"b".repeat(40),
        "backend_product_sha":"c".repeat(40), "backend_execution_sha":"c".repeat(40),
        "backend_adapter_contract":null, "frontend_sha":"d".repeat(40),
        "api_endpoint":"http://127.0.0.1:18080/readyz", "worker_endpoint":"http://[::1]:19091/readyz",
        "frontend_endpoint":"http://127.0.0.1:14174",
    })
}

#[test]
fn direct_verify_stdin_is_bounded_strict_and_rejects_extra_authority() {
    let value = authority();
    let encoded = serde_json::to_vec(&value).unwrap();
    assert_eq!(read_authority(encoded.as_slice()).unwrap(), value);
    let duplicate = format!(
        "{{\"format_version\":2,{}",
        &str::from_utf8(&encoded).unwrap()[1..]
    );
    let error = read_authority(duplicate.as_bytes())
        .unwrap_err()
        .to_string();
    assert!(error.contains("重复 JSON 字段"), "{error}");
    assert!(read_authority([0xff].as_slice()).is_err());
    assert!(read_authority(io::repeat(b' ').take(strict::MAX_JSON_BYTES + 1)).is_err());
    reject_authority_field("target_plan", json!("unknown"), "恢复请求权威字段");
}

fn reject_authority_field(field: &str, input: Value, expected: &str) {
    let mut value = authority();
    value[field] = input;
    let encoded = serde_json::to_vec(&value).unwrap();
    let error = read_authority(encoded.as_slice())
        .expect_err(field)
        .to_string();
    assert!(
        error.contains(expected),
        "{field}={}: {error}",
        value[field]
    );
}

#[test]
fn direct_verify_stdin_checks_identifier_and_timestamp_boundaries() {
    for (field, input) in [
        ("restore_id", format!("a{}", "_".repeat(63))),
        ("backup_id", format!("b{}", "-".repeat(63))),
        ("scope_id", "a1".into()),
        ("scope_id", format!("a{}z", "_".repeat(46))),
    ] {
        let mut value = authority();
        value[field] = json!(input);
        assert_eq!(
            read_authority(serde_json::to_vec(&value).unwrap().as_slice()).unwrap(),
            value,
            "{field}"
        );
    }

    for (field, input) in [
        ("restore_id", "_restore".into()),
        ("restore_id", "Restore".into()),
        ("restore_id", "restore.one".into()),
        ("restore_id", "a".repeat(65)),
        ("backup_id", "-backup".into()),
    ] {
        reject_authority_field(field, json!(input), "计划资源名称无效");
    }
    for input in [
        "a".into(),
        "_scope".into(),
        "scope_".into(),
        "Scope".into(),
        "scope.one".into(),
        format!("a{}z", "_".repeat(47)),
    ] {
        reject_authority_field("scope_id", json!(input), "演练 scope 无效");
    }
    reject_authority_field(
        "data_verified_at",
        json!("2026-09-06T08:00:00"),
        "恢复请求校验时间不是有效时间或缺少时区",
    );
}

#[test]
fn direct_verify_stdin_requires_canonical_loopback_endpoints_with_distinct_ports() {
    let loopback = "恢复角色端点只接受明确的本机 loopback 端点";
    let explicit = "恢复角色端点必须使用显式端口和精确路径";
    let canonical = "恢复角色端点必须规范且使用互异显式端口";
    for (field, input, expected) in [
        ("api_endpoint", "http://localhost:18080/readyz", loopback),
        (
            "api_endpoint",
            "http://user@127.0.0.1:18080/readyz",
            loopback,
        ),
        (
            "api_endpoint",
            "http://127.0.0.1:18080/readyz?ok=1",
            loopback,
        ),
        ("api_endpoint", "http://127.0.0.1:18080/", explicit),
        ("api_endpoint", "http://127.0.0.1/readyz", explicit),
        ("api_endpoint", "http://127.0.0.1:018080/readyz", canonical),
        ("worker_endpoint", "http://[::1]:18080/readyz", canonical),
        ("frontend_endpoint", "http://127.0.0.1:14174/", explicit),
    ] {
        reject_authority_field(field, json!(input), expected);
    }
}

#[test]
fn direct_verify_binds_explicit_paths_and_adapter_before_network() {
    let root = std::env::current_dir().unwrap();
    let execution = root.join("execution");
    let product = root.join("product");
    let frontend = root.join("frontend");
    let bindings = root.join("bindings.json");
    let options = RuntimeVerifyOptions {
        sources: RuntimeSources {
            source_backend: execution.clone(),
            source_frontend: frontend.clone(),
            product_backend: Some(product.clone()),
            adapter_contract: Some("legacy-stable-readiness-b0-v1".to_owned()),
        },
        bindings: bindings.clone(),
        receipt: root.join("runtime.json"),
    };
    let runtime = json!({
        "paths":{"backend_product_root":product,"backend_execution_root":execution,
            "frontend_root":frontend,"bindings":bindings},
        "source":{"backend_adapter_contract":"legacy-stable-readiness-b0-v1"},
    });
    verify_paths(&runtime, &options.sources, &options.bindings).unwrap();
    let mut changed = runtime.clone();
    changed["paths"]["backend_product_root"] = json!(execution);
    assert!(verify_paths(&changed, &options.sources, &options.bindings).is_err());
    changed = runtime;
    changed["source"]["backend_adapter_contract"] = Value::Null;
    assert!(verify_paths(&changed, &options.sources, &options.bindings).is_err());
}

#[test]
fn direct_verify_stdin_pairs_backend_sources_and_only_selects_frozen_b0() {
    reject_authority_field(
        "backend_execution_sha",
        json!("e".repeat(40)),
        "恢复请求适配关系无效",
    );
    reject_authority_field(
        "backend_adapter_contract",
        json!("legacy-stable-readiness-b0-v1"),
        "恢复请求适配关系无效",
    );

    let mut value = authority();
    value["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    value["backend_product_sha"] = json!("815c5eafb09d4b493319d255fb7ab88ddd02c8b6");
    value["backend_execution_sha"] = json!("c05114bcdf5c369cd74087db6317ce3c8f89bee8");
    assert!(!b0::matches(&value));
    value["frontend_sha"] = json!("0087ea2ecf62530d042b9e52f5c950fb34c66d78");
    let parsed = read_authority(serde_json::to_vec(&value).unwrap().as_slice()).unwrap();
    assert_eq!(parsed, value);
    assert!(b0::matches(&parsed));
    value["backend_execution_sha"] = json!("e".repeat(40));
    assert!(!b0::matches(&value));
}

#[test]
fn direct_verify_requires_bound_authority_and_preserves_exact_reply() {
    let root = std::env::current_dir().unwrap();
    let authority = authority();
    let make_document = |name, value: Value| document::Document {
        path: root.join(name),
        raw: serde_json::to_vec(&value).unwrap(),
        value,
    };
    let bindings = make_document(
        "bindings.json",
        json!({
            "record":{"status":"data_verified","data_verified_at":authority["data_verified_at"],
                "plan_hash":authority["plan_hash"],"plan":{"id":authority["restore_id"],"backup_id":authority["backup_id"],
                    "scope_id":authority["scope_id"],"frontend_sha":authority["frontend_sha"],
                    "api_ready_url":authority["api_endpoint"],"worker_ready_url":authority["worker_endpoint"]}},
            "manifest":{"id":authority["backup_id"],"source_sha":authority["backup_source_sha"]},
        }),
    );
    let backend = make_document("backend.json", json!({}));
    let frontend = make_document(
        "frontend.json",
        json!({"files":[{"path":"index.html"},{"path":".vite/manifest.json"}]}),
    );
    let paths = json!({"bindings":bindings.path,"backend_build":backend.path,"frontend_build":frontend.path});
    let digests = json!({"bindings":bindings.sha256(),"backend_build":backend.sha256(),"frontend_build":frontend.sha256()});
    let roots =
        json!({"backend_product":root,"backend_execution":root,"frontend":root.join("frontend")});
    let launch = make_document(
        "launch.json",
        json!({"request":{"authority":authority,"paths":paths,"digests":digests,"roots":roots}}),
    );
    let documents = RuntimeDocuments {
        bindings,
        backend,
        frontend,
        launch,
    };
    let mut runtime_paths = paths;
    runtime_paths["backend_product_root"] = roots["backend_product"].clone();
    runtime_paths["backend_execution_root"] = roots["backend_execution"].clone();
    runtime_paths["frontend_root"] = roots["frontend"].clone();
    let source = json!({"backend_product_sha":authority["backend_product_sha"],
        "backend_execution_sha":authority["backend_execution_sha"],"backend_adapter_contract":null,
        "frontend_sha":authority["frontend_sha"]});
    let mut receipt_source = source.clone();
    receipt_source["backup_source_sha"] = authority["backup_source_sha"].clone();
    let runtime = make_document(
        "runtime.json",
        json!({
            "restore":{"id":authority["restore_id"],"backup_id":authority["backup_id"],"scope_id":authority["scope_id"],
                "data_verified_at":authority["data_verified_at"],"plan_hash":authority["plan_hash"]},
            "endpoints":{"api":authority["api_endpoint"],"worker":authority["worker_endpoint"],"frontend":authority["frontend_endpoint"]},
            "paths":runtime_paths,"digests":digests,"source":receipt_source,
        }),
    );
    verify_authority(&runtime.value, &documents, &authority, source.clone()).unwrap();
    let mut different = authority;
    different["plan_hash"] = json!("e".repeat(64));
    assert!(verify_authority(&runtime.value, &documents, &different, source).is_err());

    let processes = json!({"api":{"identity":{"pid":42}}});
    let readiness = json!({"status":"ready","mysql":"up","redis":"up","object_storage":"up"});
    let descriptor =
        |input: &document::Document| json!({"path":input.path,"sha256":input.sha256()});
    assert_eq!(
        reply(&runtime, &documents, processes.clone(), readiness.clone()),
        json!({
            "format_version":1,"kind":"restore-runtime-verification","status":"verified",
            "runtime_receipt_sha256":runtime.sha256(),"bindings":descriptor(&documents.bindings),
            "build_receipts":{"backend":descriptor(&documents.backend),"frontend":descriptor(&documents.frontend),"launch":descriptor(&documents.launch)},
            "restore":runtime.value["restore"],"source":runtime.value["source"],"endpoints":runtime.value["endpoints"],
            "processes":processes,"api_readiness":readiness,
            "frontend":{"build_receipt_sha256":documents.frontend.sha256(),"files":documents.frontend.value["files"]},
        })
    );
}

#[cfg(any(windows, target_os = "linux"))]
mod online_probes {
    use super::*;
    use crate::recovery::clone::native::resources::common;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    type Response = (&'static str, &'static str, &'static [u8]);
    const API_BODY: &[u8] =
        br#"{"status":"ready","mysql":"up","redis":"up","object_storage":"up"}"#;
    const API_RESPONSES: &[Response] = &[("/readyz", "application/json", API_BODY)];
    const WORKER_RESPONSES: &[Response] = &[("/readyz", "text/plain", b"")];
    const FRONTEND_RESPONSES: &[Response] = &[
        ("/index.html", "text/html", b"<main>restore</main>"),
        ("/app.js", "text/javascript", b"globalThis.ready=true"),
        (
            "/build-identity.json",
            "application/json",
            br#"{"revision":"fixture"}"#,
        ),
    ];

    fn respond<T: Send + 'static>(
        listener: &TcpListener,
        responses: &'static [Response],
        before_last_response: impl FnOnce() -> T + Send + 'static,
    ) -> JoinHandle<T> {
        let listener = listener.try_clone().unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(45);
            let (&last, earlier) = responses.split_last().unwrap();
            for &response in earlier {
                let stream = request_at(&listener, response.0, deadline);
                send_response(stream, response);
            }
            let stream = request_at(&listener, last.0, deadline);
            drop(listener);
            let outcome = before_last_response();
            send_response(stream, last);
            outcome
        })
    }

    fn request_at(listener: &TcpListener, path: &str, deadline: Instant) -> TcpStream {
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "没有收到恢复探针请求：{path}");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("恢复探针测试监听失败：{error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 8192, "恢复探针请求过长");
        }
        let request = std::str::from_utf8(&request).unwrap();
        assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
        stream
    }

    fn send_response(mut stream: TcpStream, (_, content_type, body): Response) {
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        stream.write_all(body).unwrap();
    }

    #[derive(Clone, Copy)]
    enum DuringProbe {
        Unchanged,
        ReplaceReceipt,
        CloseWorkerListener,
        StaleWorkerIdentity,
        AddWorkerConfiguration,
    }

    #[test]
    fn successful_http_probes_still_require_original_processes_and_configuration() {
        for change in [
            DuringProbe::Unchanged,
            DuringProbe::ReplaceReceipt,
            DuringProbe::CloseWorkerListener,
            DuringProbe::StaleWorkerIdentity,
            DuringProbe::AddWorkerConfiguration,
        ] {
            exercise(change);
        }
    }

    #[test]
    fn process_observation_rejects_receipt_and_worker_artifact_drift() {
        let root = crate::workspace::root_dir();
        let local = root.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let directory = tempfile::tempdir_in(local).unwrap();
        let listeners = [(); 3].map(|()| TcpListener::bind("127.0.0.1:0").unwrap());
        let identity = common::process_identity(std::process::id())
            .unwrap()
            .unwrap();
        let runtime = runtime_fixture(
            &root,
            directory.path(),
            role_endpoints(&listeners),
            &identity,
            &json!("process-drift"),
        );
        let commands = json!({"frontend":[identity["executable"]]});
        observe_processes(&runtime, &commands, &mut Inputs::new(&root)).unwrap();
        let reject = |candidate: &Value, expected: &str| {
            let result = observe_processes(candidate, &commands, &mut Inputs::new(&root));
            let error = result.err().expect("必须拒绝恢复进程输入漂移").to_string();
            assert!(error.contains(expected), "{error}");
        };
        for role in ["api", "frontend"] {
            let receipt = directory.path().join(format!("{role}.json"));
            let original = fs::read(&receipt).unwrap();
            let mut changed = original.clone();
            changed.push(b' ');
            fs::write(&receipt, &changed).unwrap();
            reject(&runtime, "进程收据摘要或创建身份不同");
            assert_eq!(fs::read(&receipt).unwrap(), changed);
            fs::write(&receipt, original).unwrap();
        }
        let mut changed = runtime.clone();
        changed["backend"]["artifacts"]["worker"]["sha256"] = json!("0".repeat(64));
        reject(&changed, "未运行登记的构建产物");
        let mut changed = runtime;
        let bytes = changed["backend"]["artifacts"]["worker"]["bytes"]
            .as_u64()
            .unwrap();
        changed["backend"]["artifacts"]["worker"]["bytes"] = json!(bytes + 1);
        reject(&changed, "未运行登记的构建产物");

        let receipt = directory.path().join("api.json");
        let mut extra = strict::read(&receipt).unwrap();
        extra["extra"] = json!(true);
        fs::write(&receipt, serde_json::to_vec(&extra).unwrap()).unwrap();
        let saved = document::read(&receipt, &root, "探针进程收据").unwrap();
        changed["processes"]["api"]["receipt_sha256"] = json!(saved.sha256());
        reject(&changed, "恢复产品进程收据字段缺失或包含未知字段");
    }

    fn exercise(change: DuringProbe) {
        let root = crate::workspace::root_dir();
        let local = root.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let runtime_dir = tempfile::Builder::new()
            .prefix("runtime-probe-recheck-")
            .tempdir_in(local)
            .unwrap();
        let listeners = [(); 3].map(|()| TcpListener::bind("127.0.0.1:0").unwrap());
        let ports = listeners
            .each_ref()
            .map(|listener| listener.local_addr().unwrap().port());
        let configuration = environment::tests::Fixture::with_ports(ports);
        let mut request = configuration.request();
        let endpoints = role_endpoints(&listeners);
        let [api, worker, frontend] = listeners;
        let identity = common::process_identity(std::process::id())
            .unwrap()
            .unwrap();
        let runtime = runtime_fixture(
            &root,
            runtime_dir.path(),
            endpoints,
            &identity,
            &request["authority"]["scope_id"],
        );
        let window = Window::direct().unwrap();
        let mut inputs = Inputs::new(&root);
        request["environment"] = environment::capture(&mut inputs, &request).unwrap();
        verify_role_configuration(&mut inputs, &request, runtime_dir.path()).unwrap();
        let commands = json!({"frontend":[identity["executable"]]});
        let mut processes = observe_processes(&runtime, &commands, &mut inputs).unwrap();
        assert_eq!(processes.value["worker"]["identity"], identity);
        if matches!(change, DuringProbe::StaleWorkerIdentity) {
            // 用已验证的真实监听保持 PID 不变，仅让后验观察携带旧创建身份。
            processes.value["worker"]["identity"]["started"] = json!("0");
            assert_ne!(processes.value["worker"]["identity"], identity);
        }
        let api_response = respond(&api, API_RESPONSES, || {});
        let worker_response = respond(&worker, WORKER_RESPONSES, || {});
        let mut worker = Some(worker);
        let close_during_probe = if matches!(change, DuringProbe::CloseWorkerListener) {
            worker.take()
        } else {
            None
        };
        let worker_receipt = runtime_dir.path().join("worker.json");
        let receipt_before = fs::read(&worker_receipt).unwrap();
        let mut changed_receipt = receipt_before.clone();
        let first_digit = changed_receipt
            .iter_mut()
            .find(|byte| **byte == b'1')
            .unwrap();
        *first_digit = b'2';
        let additional_config = configuration.worker_directory().join("app.dev.toml");
        assert!(!additional_config.exists());
        let new_config = additional_config.clone();
        let frontend_response = respond(&frontend, FRONTEND_RESPONSES, move || {
            drop(close_during_probe);
            match change {
                DuringProbe::ReplaceReceipt => Some(fs::write(worker_receipt, changed_receipt)),
                DuringProbe::AddWorkerConfiguration => {
                    Some(fs::write(new_config, b"unselected = true\n"))
                }
                _ => None,
            }
        });
        let files = frontend_files();
        let readiness = probe_and_recheck(&runtime, &files, &processes, || {
            verify_role_configuration(&mut inputs, &request, runtime_dir.path())
        });
        api_response.join().unwrap();
        worker_response.join().unwrap();
        let write_result = frontend_response.join().unwrap();
        let result = window.finish(inputs.finish(readiness));
        if matches!(change, DuringProbe::AddWorkerConfiguration) {
            assert_eq!(fs::read(additional_config).unwrap(), b"unselected = true\n");
        }
        if matches!(change, DuringProbe::StaleWorkerIdentity) {
            observe_listener(&runtime, "worker", &identity).unwrap();
        }
        if matches!(change, DuringProbe::Unchanged) {
            assert_probe_reply(
                &root,
                runtime_dir.path(),
                &runtime,
                &files,
                processes.value,
                result.as_ref().unwrap().clone(),
            );
        }
        assert_recheck(
            change,
            result,
            write_result,
            &runtime_dir.path().join("worker.json"),
            &receipt_before,
            API_BODY,
        );
    }

    fn role_endpoints(listeners: &[TcpListener; 3]) -> Value {
        let [api, worker, frontend] = listeners
            .each_ref()
            .map(|listener| listener.local_addr().unwrap());
        json!({"api":format!("http://{api}/readyz"),
            "worker":format!("http://{worker}/readyz"),"frontend":format!("http://{frontend}")})
    }

    fn frontend_files() -> Value {
        let public = FRONTEND_RESPONSES
            .iter()
            .map(|(path, _, body)| (path.strip_prefix('/').unwrap(), *body));
        Value::Array(
            public
                .chain([(".vite/manifest.json", b"{}".as_slice())])
                .map(|(path, body)| {
                    json!({"path":path,"bytes":body.len(),"sha256":hex::encode(Sha256::digest(body))})
                })
                .collect(),
        )
    }

    fn assert_probe_reply(
        root: &Path,
        directory: &Path,
        runtime: &Value,
        files: &Value,
        processes: Value,
        readiness: Value,
    ) {
        let save = |name, value| {
            let file = directory.join(name);
            strict::write_new(&file, value).unwrap();
            document::read(&file, root, "探针核验收据").unwrap()
        };
        let receipt = save("runtime.json", runtime);
        let documents = RuntimeDocuments {
            bindings: save("bindings.json", &json!({"record":"fixture"})),
            backend: save("backend-build.json", &runtime["backend"]),
            frontend: save("frontend-build.json", &json!({"files":files})),
            launch: save(
                "runtime-launch.json",
                &json!({"processes":runtime["processes"]}),
            ),
        };
        let frontend_sha = documents.frontend.sha256();
        let observed = Observed {
            documents,
            processes,
            readiness,
        }
        .reply(&receipt);
        assert_eq!(observed["status"], "verified");
        assert_eq!(observed["runtime_receipt_sha256"], receipt.sha256());
        assert_eq!(
            observed["api_readiness"],
            serde_json::from_slice::<Value>(API_BODY).unwrap()
        );
        assert_eq!(
            observed["frontend"],
            json!({"build_receipt_sha256":frontend_sha,"files":files})
        );
        let actual = observed["processes"].as_object().unwrap();
        assert_eq!(actual.len(), generation::ROLES.len());
        for role in generation::ROLES {
            let recorded = &runtime["processes"][role];
            assert_eq!(actual[role]["identity"], recorded["identity"]);
            assert_eq!(
                actual[role]["process_receipt"],
                json!({
                    "path":recorded["receipt_path"],"sha256":recorded["receipt_sha256"],
                })
            );
        }
    }

    fn assert_recheck(
        change: DuringProbe,
        result: Result<Value>,
        write_result: Option<io::Result<()>>,
        worker_receipt: &Path,
        receipt_before: &[u8],
        api_reply: &[u8],
    ) {
        match change {
            DuringProbe::Unchanged => {
                assert_eq!(
                    result.unwrap(),
                    serde_json::from_slice::<Value>(api_reply).unwrap()
                );
            }
            DuringProbe::ReplaceReceipt => {
                let attempt = write_result.unwrap();
                if cfg!(windows) {
                    assert_eq!(attempt.unwrap_err().raw_os_error(), Some(32));
                    assert_eq!(fs::read(worker_receipt).unwrap(), receipt_before);
                    assert_eq!(
                        result.unwrap(),
                        serde_json::from_slice::<Value>(api_reply).unwrap()
                    );
                } else {
                    attempt.unwrap();
                    assert!(result.unwrap_err().to_string().contains("被替换或修改"));
                }
            }
            DuringProbe::CloseWorkerListener => {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "监听所有者不同于登记内核进程；核对绑定端点与服务进程"
                );
            }
            DuringProbe::StaleWorkerIdentity => {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "登记进程已退出或内核创建身份变化"
                );
            }
            DuringProbe::AddWorkerConfiguration => {
                write_result.unwrap().unwrap();
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "恢复启动角色配置快照发生漂移"
                );
            }
        }
    }

    fn runtime_fixture(
        root: &Path,
        directory: &Path,
        endpoints: Value,
        identity: &Value,
        scope: &Value,
    ) -> Value {
        let executable = path(&identity["executable"]).unwrap();
        let facts = StableFile::open(&executable).unwrap().facts().unwrap();
        let artifact =
            json!({"executable":executable,"bytes":facts["bytes"],"sha256":facts["sha256"]});
        let mut runtime = json!({
            "paths":{"runtime_dir":directory},
            "restore":{"scope_id":scope},
            "endpoints":endpoints,
            "backend":{"artifacts":{"api":artifact,"worker":artifact}},
            "processes":{},
        });
        for role in generation::ROLES {
            let receipt = directory.join(format!("{role}.json"));
            strict::write_new(
                &receipt,
                &json!({
                    "format_version":1,"role":role,"scope_id":scope,"identity":identity,
                }),
            )
            .unwrap();
            let saved = document::read(&receipt, root, "探针进程收据").unwrap();
            runtime["processes"][role] = json!({
                "identity":identity,"receipt_path":receipt,"receipt_sha256":saved.sha256(),
            });
        }
        runtime
    }
}
