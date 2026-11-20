use super::*;
use std::{
    cell::Cell,
    io,
    net::{Ipv4Addr, Shutdown, TcpListener},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

const ENDPOINT: &str = "http://127.0.0.1:9473";
const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECRET: &str = "TEST_SECRET_MUST_NOT_APPEAR";
const PAYLOAD: &[u8] = b"bound original bytes";
const CHANGED_PAYLOAD: &[u8] = b"Bound original bytes";

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    artifacts: PathBuf,
    output: PathBuf,
    endpoint: String,
    configuration: Value,
    item: Value,
}

impl Fixture {
    fn new() -> Self {
        Self::with_endpoint(ENDPOINT)
    }

    fn with_endpoint(endpoint: &str) -> Self {
        let managed = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&managed).unwrap();
        let temp = tempfile::tempdir_in(&managed).unwrap();
        let backend = crate::recovery::fixture_control::portable_path(
            &fs::canonicalize(temp.path()).unwrap(),
        );
        let local = backend.join(".local-tests");
        let config = local.join("config");
        let artifacts = local.join("export");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir(&artifacts).unwrap();
        fs::write(config.join("app.test.toml"), format!(
            "[object_storage]\nendpoint = \"{endpoint}\"\nregion = \"us-east-1\"\naccess_key = \"test-access\"\nsecret_key = \"{SECRET}\"\n"
        )).unwrap();
        let configuration =
            s3::app_configuration::configuration_value(&backend, &config, "test").unwrap();
        let file = artifacts.join("payload.bin");
        fs::write(&file, PAYLOAD).unwrap();
        let artifact = digest::file(&file).unwrap();
        let item = json!({
            "bucket":"uploads", "source_key":"source-scope/item", "target_key":"target-scope/item",
            "artifact":{"file":"payload.bin","bytes":artifact.bytes,"sha256":artifact.sha256},
            "metadata":{"CacheControl":null,"ContentDisposition":null,"ContentEncoding":null,
                "ContentLanguage":null,"ContentType":"application/octet-stream","Expires":null,
                "WebsiteRedirectLocation":null,"Metadata":{}}
        });
        let work = local.join("copy-work");
        fs::create_dir(&work).unwrap();
        let output = work.join("object-write");
        Self {
            _temp: temp,
            backend,
            artifacts,
            output,
            endpoint: endpoint.to_owned(),
            configuration,
            item,
        }
    }

    fn spec(&self) -> Create<'_> {
        Create {
            backend: &self.backend,
            configuration_root: &self.backend,
            artifact_root: &self.artifacts,
            work: self.output.parent().unwrap(),
            directory: &self.output,
            object: TargetObject {
                scope: "target-scope",
                endpoint: &self.endpoint,
                item: &self.item,
            },
            configuration: &self.configuration,
            generation: GENERATION,
            step: "object-first",
        }
    }

    fn artifact(&self) -> input::ArtifactBinding {
        input::bound_file(&self.artifacts, &self.item["artifact"]).unwrap()
    }

    fn diagnostic(&self) -> Value {
        document::read(&self.output.join("diagnostic.json")).unwrap()
    }
}

fn with_loopback(test: impl FnOnce(&Fixture)) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let fixture = Fixture::with_endpoint(&format!("http://{}", listener.local_addr().unwrap()));
    thread::scope(|scope| {
        let (done, finished) = mpsc::channel::<()>();
        let observer = scope.spawn(move || {
            let mut connections = 0;
            let mut draining = false;
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        connections += 1;
                        // 若回归导致请求发出，立即结束连接，避免等待客户端超时。
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if draining {
                            return connections;
                        }
                        draining = finished.recv_timeout(Duration::from_millis(5))
                            == Err(mpsc::RecvTimeoutError::Disconnected);
                    }
                    Err(error) => panic!("无法观察回环连接：{error}"),
                }
            }
        });
        test(&fixture);
        drop(done);
        assert_eq!(observer.join().unwrap(), 0);
    });
}

fn created() -> Value {
    json!({"status":"created_unverified","http_status":200,"remote_write_may_have_occurred":true,
        "automatic_retry_permitted":false,"response":{"ETag":"\"object\""}})
}

#[test]
fn records_pending_intent_before_put_and_requires_independent_observation() {
    let fixture = Fixture::new();
    let source = fixture.artifact();
    send_once(&fixture.spec(), &source, &|| Ok(()), |request| {
        assert_eq!(request.bucket, "uploads");
        assert_eq!(request.key, "target-scope/item");
        assert_eq!(request.source, fixture.artifacts.join("payload.bin"));
        assert_eq!(request.expected_sha256, source.digest.sha256);
        assert_eq!(fixture.diagnostic()["reason"], "write_result_not_recorded");
        let intent_path = fixture.output.join("intent.json");
        let intent = document::read(&intent_path).unwrap();
        assert_eq!(intent["step"], "object-first");
        assert_eq!(intent["generation_sha256"], GENERATION);
        assert!(!fs::read_to_string(intent_path).unwrap().contains(SECRET));
        Ok(created())
    })
    .unwrap();
    let diagnostic = fixture.diagnostic();
    assert_eq!(diagnostic["outcome"], "created_unverified");
    assert_eq!(diagnostic["reason"], "independent_observation_required");
    assert_eq!(
        diagnostic["response_sha256"],
        digest::plan_hash(&created()).unwrap()
    );
}

#[test]
fn changed_payload_after_binding_stops_before_creating_intent() {
    with_loopback(|fixture| {
        let source = fixture.artifact();
        let payload = fixture.artifacts.join(&source.file);
        let checks = Cell::new(0);
        let check = || {
            checks.set(checks.get() + 1);
            fs::write(&payload, CHANGED_PAYLOAD)?;
            let changed = digest::file(&payload)?;
            assert_eq!(changed.bytes, source.digest.bytes);
            assert_ne!(changed.sha256, source.digest.sha256);
            Ok(())
        };
        let sends = Cell::new(0);
        let error = send_once(&fixture.spec(), &source, &check, |request| {
            sends.set(sends.get() + 1);
            s3::put_if_absent(request)
        })
        .unwrap_err();

        assert_eq!(error.to_string(), "原对象载荷在保护句柄建立前变化");
        assert_eq!(checks.get(), 1);
        assert_eq!(sends.get(), 0);
        assert_eq!(fs::read(&payload).unwrap(), CHANGED_PAYLOAD);
        assert!(!fixture.output.exists());
    });
}

#[cfg(any(windows, unix))]
#[test]
fn payload_change_at_final_check_preserves_intent_without_a_connection() {
    with_loopback(|fixture| {
        let source = fixture.artifact();
        let payload = fixture.artifacts.join(&source.file);
        let intent_path = fixture.output.join("intent.json");
        let original_intent = std::cell::OnceCell::new();
        let checks = Cell::new(0);
        let check = || {
            checks.set(checks.get() + 1);
            if checks.get() != 2 {
                return Ok(());
            }
            assert_eq!(fixture.diagnostic()["reason"], "write_result_not_recorded");
            original_intent.set(fs::read(&intent_path)?).unwrap();
            #[cfg(windows)]
            {
                let error = fs::write(&payload, CHANGED_PAYLOAD).unwrap_err();
                assert_eq!(error.raw_os_error(), Some(32));
                Err(error.into())
            }
            #[cfg(unix)]
            {
                fs::write(&payload, CHANGED_PAYLOAD)?;
                let changed = digest::file(&payload)?;
                assert_eq!(changed.bytes, source.digest.bytes);
                assert_ne!(changed.sha256, source.digest.sha256);
                Ok(())
            }
        };
        let sends = Cell::new(0);
        let error = send_once(&fixture.spec(), &source, &check, |request| {
            sends.set(sends.get() + 1);
            assert_eq!(request.expected_bytes, source.digest.bytes);
            assert_eq!(request.expected_sha256, source.digest.sha256);
            let result = s3::put_if_absent(request);
            #[cfg(unix)]
            assert_eq!(
                result.as_ref().unwrap_err().to_string(),
                "条件创建来源与计划字节数或 SHA256 不同；未执行写请求"
            );
            result
        })
        .unwrap_err();

        let diagnostic = fixture.diagnostic();
        assert_eq!(diagnostic["automatic_retry_permitted"], false);
        let intent = document::read(&intent_path).unwrap();
        assert_eq!(intent["expected"]["bytes"], source.digest.bytes);
        assert_eq!(intent["expected"]["sha256"], source.digest.sha256);
        assert_eq!(
            fs::read(&intent_path).unwrap(),
            original_intent.into_inner().unwrap()
        );
        #[cfg(windows)]
        {
            assert_eq!(
                error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
                Some(32)
            );
            assert_eq!(sends.get(), 0);
            assert_eq!(fs::read(&payload).unwrap(), PAYLOAD);
            assert_eq!(diagnostic["outcome"], "not_started");
            assert_eq!(diagnostic["reason"], "preconditions_changed");
            assert_eq!(diagnostic["remote_write_may_have_occurred"], false);
        }
        #[cfg(unix)]
        {
            assert_eq!(sends.get(), 1);
            assert_eq!(fs::read(&payload).unwrap(), CHANGED_PAYLOAD);
            assert_eq!(diagnostic["outcome"], "needs_reconciliation");
            assert_eq!(diagnostic["reason"], "post_write_binding_changed");
            assert_eq!(diagnostic["remote_write_may_have_occurred"], true);
            assert_eq!(
                error.to_string(),
                "条件创建返回 needs_reconciliation；保持原 intent，必须显式核对"
            );
        }
    });
}

#[test]
fn changed_guard_before_send_and_during_send_have_distinct_receipts() {
    let first = Fixture::new();
    let calls = Cell::new(0);
    let check = || {
        let next = calls.get() + 1;
        calls.set(next);
        if next == 2 {
            Err("前置证据变化".into())
        } else {
            Ok(())
        }
    };
    assert!(
        send_once(&first.spec(), &first.artifact(), &check, |_| {
            panic!("前置证据变化时不能写")
        })
        .is_err()
    );
    assert_eq!(first.diagnostic()["outcome"], "not_started");
    assert_eq!(first.diagnostic()["remote_write_may_have_occurred"], false);

    let second = Fixture::new();
    let changed = Cell::new(false);
    let check = || {
        if changed.get() {
            Err("写后证据变化".into())
        } else {
            Ok(())
        }
    };
    assert!(
        send_once(&second.spec(), &second.artifact(), &check, |_| {
            changed.set(true);
            Ok(created())
        })
        .is_err()
    );
    assert_eq!(second.diagnostic()["outcome"], "needs_reconciliation");
    assert_eq!(second.diagnostic()["remote_write_may_have_occurred"], true);
    assert_eq!(second.diagnostic()["http_status"], 200);
}

#[test]
fn conflict_and_transport_failure_preserve_unknown_write_boundary_without_leaking_errors() {
    let first = Fixture::new();
    assert!(send_once(&first.spec(), &first.artifact(), &|| Ok(()), |_| {
        Ok(json!({"status":"already_exists","http_status":412,"remote_write_may_have_occurred":false}))
    }).is_err());
    assert_eq!(first.diagnostic()["outcome"], "already_exists");
    assert_eq!(first.diagnostic()["automatic_retry_permitted"], false);

    let second = Fixture::new();
    let error = send_once(&second.spec(), &second.artifact(), &|| Ok(()), |_| {
        Err(SECRET.into())
    })
    .unwrap_err();
    assert!(!error.to_string().contains(SECRET));
    assert_eq!(second.diagnostic()["outcome"], "needs_reconciliation");
    assert!(
        !fs::read_to_string(second.output.join("diagnostic.json"))
            .unwrap()
            .contains(SECRET)
    );
}
