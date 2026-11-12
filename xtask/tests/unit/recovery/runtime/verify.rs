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
    assert!(read_authority(duplicate.as_bytes()).is_err());
    assert!(read_authority([0xff].as_slice()).is_err());
    assert!(read_authority(io::repeat(b' ').take(strict::MAX_JSON_BYTES + 1)).is_err());
    let mut unbound = value;
    unbound["target_plan"] = json!("unknown");
    assert!(read_authority(serde_json::to_vec(&unbound).unwrap().as_slice()).is_err());
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
fn only_the_exact_legacy_source_selects_the_b0_frontend_receipt() {
    let mut value = authority();
    value["backend_adapter_contract"] = json!("legacy-stable-readiness-b0-v1");
    value["backend_product_sha"] = json!("815c5eafb09d4b493319d255fb7ab88ddd02c8b6");
    value["backend_execution_sha"] = json!("c05114bcdf5c369cd74087db6317ce3c8f89bee8");
    assert!(!b0::matches(&value));
    value["frontend_sha"] = json!("0087ea2ecf62530d042b9e52f5c950fb34c66d78");
    assert!(b0::matches(&value));
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
        net::TcpListener,
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    fn respond<T: Send + 'static>(
        listener: &TcpListener,
        path: &'static str,
        content_type: &'static str,
        body: &'static [u8],
        before_response: impl FnOnce() -> T + Send + 'static,
    ) -> JoinHandle<T> {
        let listener = listener.try_clone().unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(45);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "没有收到恢复探针请求");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("恢复探针测试监听失败：{error}"),
                }
            };
            drop(listener);
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
            let outcome = before_response();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
            outcome
        })
    }

    #[derive(Clone, Copy)]
    enum DuringProbe {
        Unchanged,
        ReplaceReceipt,
        CloseWorkerListener,
        AddWorkerConfiguration,
    }

    #[test]
    fn successful_http_probes_still_require_original_processes_and_configuration() {
        for change in [
            DuringProbe::Unchanged,
            DuringProbe::ReplaceReceipt,
            DuringProbe::CloseWorkerListener,
            DuringProbe::AddWorkerConfiguration,
        ] {
            exercise(change);
        }
    }

    fn exercise(change: DuringProbe) {
        let root = crate::workspace::root_dir();
        let local = root.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let runtime_dir = tempfile::Builder::new()
            .prefix("runtime-probe-recheck-")
            .tempdir_in(local)
            .unwrap();
        let api = TcpListener::bind("127.0.0.1:0").unwrap();
        let worker = TcpListener::bind("127.0.0.1:0").unwrap();
        let frontend = TcpListener::bind("127.0.0.1:0").unwrap();
        let ports =
            [&api, &worker, &frontend].map(|listener| listener.local_addr().unwrap().port());
        let configuration = environment::tests::Fixture::with_ports(ports);
        let mut request = configuration.request();
        let endpoints = json!({
            "api":format!("http://{}/readyz", api.local_addr().unwrap()),
            "worker":format!("http://{}/readyz", worker.local_addr().unwrap()),
            "frontend":format!("http://{}", frontend.local_addr().unwrap()),
        });
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
        let processes = observe_processes(&runtime, &commands, &mut inputs).unwrap();
        assert_eq!(processes.value["worker"]["identity"], identity);
        let api_reply = br#"{"status":"ready","mysql":"up","redis":"up","object_storage":"up"}"#;
        let api_response = respond(&api, "/readyz", "application/json", api_reply, || {});
        let worker_response = respond(&worker, "/readyz", "text/plain", b"", || {});
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
        let frontend_body = b"<main>restore</main>";
        let additional_config = configuration.worker_directory().join("app.dev.toml");
        assert!(!additional_config.exists());
        let new_config = additional_config.clone();
        let frontend_response = respond(
            &frontend,
            "/index.html",
            "text/html",
            frontend_body,
            move || {
                drop(close_during_probe);
                match change {
                    DuringProbe::ReplaceReceipt => Some(fs::write(worker_receipt, changed_receipt)),
                    DuringProbe::AddWorkerConfiguration => {
                        Some(fs::write(new_config, b"unselected = true\n"))
                    }
                    _ => None,
                }
            },
        );
        let files = json!([
            {"path":"index.html","bytes":frontend_body.len(),"sha256":hex::encode(Sha256::digest(frontend_body))},
            {"path":".vite/manifest.json","bytes":2,"sha256":hex::encode(Sha256::digest(b"{}"))},
        ]);
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
        assert_recheck(
            change,
            result,
            write_result,
            &runtime_dir.path().join("worker.json"),
            &receipt_before,
            api_reply,
        );
        drop(worker);
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
