use std::{
    io::{self, Cursor, Read, Write},
    sync::{Arc, Mutex, mpsc},
};

use super::*;

#[test]
fn existing_data_failure_keeps_primary_cause_and_independent_cleanup_receipt() -> Result<()> {
    let root = tempfile::tempdir_in(crate::workspace::root_dir())?;
    let output = root.path().join("attempt-0001");
    std::fs::create_dir(&output)?;
    let result: Result<Value> = protocol::retain_cleanup(
        root.path(),
        &output,
        "session-cleanup",
        "ProducerCleanupError",
        Err("原业务读取结果未知".into()),
        Err("精确生产者清理失败".into()),
    );
    let error = result.unwrap_err();
    assert_eq!(error.source().unwrap().to_string(), "原业务读取结果未知");
    assert_eq!(
        document::read(&output.join("session-cleanup-error.json"))?,
        json!({"error_type":"ProducerCleanupError"})
    );
    assert!(!output.join("business-result.json").exists());
    Ok(())
}

struct AfterWrite {
    released: Option<mpsc::Receiver<()>>,
    bytes: Cursor<Vec<u8>>,
}

impl Read for AfterWrite {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        if let Some(released) = self.released.take() {
            let _ = released.recv();
        }
        self.bytes.read(data)
    }
}

struct Writer {
    release: Option<mpsc::Sender<()>>,
    block: Option<mpsc::Receiver<()>>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(block) = self.block.take() {
            block.recv().map_err(|_| io::ErrorKind::BrokenPipe)?;
        }
        self.written.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(release) = self.release.take() {
            release.send(()).map_err(|_| io::ErrorKind::BrokenPipe)?;
        }
        Ok(())
    }
}

fn mock(
    reply: Vec<u8>,
    block: Option<mpsc::Receiver<()>>,
) -> Result<(pipes::Pipes, Arc<Mutex<Vec<u8>>>)> {
    let (release, released) = mpsc::channel();
    let written = Arc::new(Mutex::new(Vec::new()));
    let pipes = pipes::Pipes::start(
        Box::new(Writer {
            release: Some(release),
            block,
            written: Arc::clone(&written),
        }),
        Box::new(AfterWrite {
            released: Some(released),
            bytes: Cursor::new(reply),
        }),
        true,
    )?;
    Ok((pipes, written))
}

#[test]
fn managed_protocol_writes_one_fixed_request_and_accepts_matching_reply() -> Result<()> {
    let (pipes, written) = mock(b"{\"id\":1,\"ok\":true,\"data\":{}}\n".to_vec(), None)?;
    let request = sequenced(1, "get", json!({"schedule_id":"fixture"}))?;
    let raw = pipes.exchange(&request, false, Instant::now() + Duration::from_secs(2))?;
    assert!(
        matches!(response(1, &parse(&raw, "fixture")?), Reply::Data(value) if value == json!({}))
    );
    let captured = written.lock().unwrap();
    assert_eq!(captured.last(), Some(&b'\n'));
    assert_eq!(parse(&captured, "fixture")?, request);
    drop(captured);
    pipes.finish()
}

#[test]
fn managed_protocol_rejects_reserved_fields_and_never_saves_unexpected_data() {
    assert!(sequenced(1, "get", json!({"id":2})).is_err());
    assert!(sequenced(1, "get", json!({"operation":"disable"})).is_err());
    let unexpected = json!({"id":2,"ok":true,"data":{"private":"fixture"}});
    assert!(matches!(response(1, &unexpected), Reply::Failure {
        diagnostic, channel: Channel::Unknown
    } if diagnostic == json!({"id":2,"ok":false,"error_type":"InvalidSessionResponse","status":null})));
}

#[test]
fn managed_protocol_waits_for_automatic_logout_only_after_a_matching_terminal_reply() -> Result<()>
{
    let request = sequenced(1, "disable", json!({"schedule_id":"fixture","version":1}))?;
    let deadline = Instant::now() + LOGOUT_TIMEOUT;
    assert_eq!(Channel::Open.logout_deadline(deadline)?, None);
    for (id, expected) in [(json!(1), true), (json!(2), false), (Value::Null, false)] {
        let rejected =
            json!({"id":id,"ok":false,"error_type":"SessionAuthorizationFailed","status":null});
        let mut frame = serde_json::to_vec(&rejected)?;
        frame.push(b'\n');
        let (pipes, written) = mock(frame, None)?;
        let raw = pipes.exchange(&request, false, Instant::now() + Duration::from_secs(2))?;
        let Reply::Failure {
            diagnostic,
            channel,
        } = response(1, &parse(&raw, "fixture")?)
        else {
            panic!("终止错误回包被当成成功");
        };
        assert_eq!(diagnostic, rejected);
        if expected {
            assert_eq!(channel.logout_deadline(deadline)?, Some(deadline));
        } else {
            assert!(channel.logout_deadline(deadline).is_err());
        }
        pipes.finish()?;
        assert_eq!(parse(&written.lock().unwrap(), "fixture")?, request);
    }
    Ok(())
}

#[test]
fn managed_protocol_bounds_blocked_writes_and_rejects_oversized_or_truncated_frames() -> Result<()>
{
    let (expired, written) = mock(Vec::new(), None)?;
    assert!(expired.exchange(&json!({}), false, Instant::now()).is_err());
    assert!(written.lock().unwrap().is_empty());
    expired.finish()?;

    let (stop, blocked) = mpsc::channel();
    let (pipes, written) = mock(Vec::new(), Some(blocked))?;
    assert!(
        pipes
            .exchange(
                &json!({}),
                false,
                Instant::now() + Duration::from_millis(25)
            )
            .is_err()
    );
    assert!(written.lock().unwrap().is_empty());
    drop(stop);
    pipes.finish()?;

    let mut oversized = vec![b'a'; pipes::FRAME_LIMIT];
    oversized.push(b'\n');
    for bad in [oversized, b"{}".to_vec()] {
        let (pipes, _) = mock(bad, None)?;
        assert!(
            pipes
                .exchange(&json!({}), false, Instant::now() + Duration::from_secs(2))
                .is_err()
        );
        pipes.finish()?;
    }
    Ok(())
}

#[test]
fn quota_login_uses_the_bound_plan_shape_and_preserves_authorization() -> Result<()> {
    let tenants = (1..=10)
        .map(|number| json!({"tenant_id":format!("tenant-{number:02}")}))
        .collect::<Vec<_>>();
    let identity = json!({"subject_id":"42","tenant_id":"system","username":"fixture",
        "client_address":"198.19.11.4"});
    let post = json!({"frontend_root":"fixture-frontend","source_admin":identity});
    let pacing = json!({"contract":{"fixture":1},"bindings":{"fixture":2}});
    let environment = json!({
        "frontend_dir":"fixture-frontend","system_admin":identity,"tenants":tenants,
        "scope_id":"fixture","api_url":"http://127.0.0.1:2001",
        "frontend_url":"http://127.0.0.1:2002","pacing":pacing,
    });
    let plan = json!({"environment":environment});
    let initialization = quota::Initialization::from_plan(&plan, &post)?;
    assert_eq!(initialization.frontend, post["frontend_root"]);
    assert_eq!(initialization.identity, identity);
    assert_eq!(
        initialization.tenants.first().map(String::as_str),
        Some("system")
    );
    assert_eq!(
        initialization.tenants.last().map(String::as_str),
        Some("tenant-10")
    );
    assert_eq!(
        initialization.configuration,
        json!({
            "contract":{"timeout_ms":120000,"pacing":{"fixture":1}},
            "bindings":{"scope_id":"fixture","api_url":"http://127.0.0.1:2001",
                "frontend_url":"http://127.0.0.1:2002","pacing":{"fixture":2}},
        })
    );
    let auth = json!({"subject_id":"42","tenant_id":"system","username":"fixture",
        "is_super_admin":false,"permissions":["tenant:list","tenant:edit","tenant:usage:list"]});
    let mut frame = serde_json::to_vec(&json!({"id":1,"ok":true,"data":auth}))?;
    frame.push(b'\n');
    let (pipes, _) = mock(frame, None)?;
    let values = json!({"frontend":initialization.frontend,"config":initialization.configuration,
        "identity":initialization.identity,"tenant_ids":initialization.tenants});
    let raw = pipes.exchange(
        &sequenced(1, "initialize", values)?,
        false,
        Instant::now() + Duration::from_secs(2),
    )?;
    let Reply::Data(returned) = response(1, &parse(&raw, "fixture")?) else {
        panic!("配额授权回包未确认");
    };
    assert_eq!(returned, auth);
    quota::authorization(&returned, &identity)?;
    let mut wrong = returned;
    wrong["permissions"] = json!(["tenant:list"]);
    assert!(quota::authorization(&wrong, &identity).is_err());
    pipes.finish()
}

#[test]
fn quota_producer_kind_and_rpc_whitelist_match_each_seed_mode() -> Result<()> {
    let tenants = vec!["system".to_owned()];
    for (mode, name, may_write) in [
        ("quotas-plan", "quota-plan", false),
        ("quotas-apply", "quota-apply", true),
        ("quotas-reconcile", "quota-reconcile", false),
    ] {
        let kind = quota::kind(mode)?;
        assert_eq!(kind.name(), name);
        assert_eq!(command::stage(name)?, "seed-runtime");
        assert!(command::accepts(name, mode));
        for operation in ["get", "usage"] {
            quota::check_operation(kind, &tenants, operation, &json!({"tenant_id":"system"}))?;
        }
        assert_eq!(
            quota::check_operation(
                kind,
                &tenants,
                "update",
                &json!({"tenant_id":"system","body":{"max_users":11}}),
            )
            .is_ok(),
            may_write,
        );
        assert!(
            quota::check_operation(kind, &tenants, "get", &json!({"tenant_id":"other"})).is_err()
        );
        assert!(quota::check_operation(kind, &tenants, "close", &json!({})).is_err());
    }
    assert!(quota::kind("departments-apply").is_err());
    Ok(())
}

#[test]
fn department_initialization_preserves_the_bound_tenant_and_principal_order() -> Result<()> {
    let tenants = [
        "z-first", "a-second", "b03", "b04", "b05", "b06", "b07", "b08", "b09", "b10",
    ]
    .map(str::to_owned);
    let templates = tenants.iter().enumerate().map(|(index, tenant)| {
        json!({"tenant_id":tenant,"subject_id":(index+1).to_string(),"username":format!("user{index}"),
            "credential_slot":format!("slot{index}"),"client_address":"198.19.11.4"})
    }).collect::<Vec<_>>();
    let tenant_ids = json!(tenants);
    let template_ids = json!(templates);
    let receipt = department::expected_receipt(
        &json!("fixture"),
        "dv-fixture-import",
        &tenants,
        &tenant_ids,
        &template_ids,
    )?;
    assert_eq!(receipt["tenant_ids"], tenant_ids);
    assert_eq!(
        receipt["template_principals"][0],
        json!({
            "tenant_id":"z-first","subject_id":"1","username":"user0"
        })
    );
    let mut sorted = tenants.clone();
    sorted.sort();
    assert!(
        department::expected_receipt(
            &json!("fixture"),
            "dv-fixture-import",
            &tenants,
            &json!(sorted),
            &template_ids,
        )
        .is_err()
    );

    let mut frame = serde_json::to_vec(&json!({"id":1,"ok":true,"data":receipt}))?;
    frame.push(b'\n');
    let (pipes, written) = mock(frame, None)?;
    let request = sequenced(
        1,
        "initialize",
        json!({
            "tenant_ids":tenant_ids,"template_identities":template_ids,
        }),
    )?;
    let raw = pipes.exchange(&request, false, Instant::now() + Duration::from_secs(2))?;
    assert!(
        matches!(response(1, &parse(&raw, "fixture")?), Reply::Data(value) if value == receipt)
    );
    pipes.finish()?;
    assert_eq!(parse(&written.lock().unwrap(), "fixture")?, request);
    Ok(())
}

#[test]
fn department_producer_preserves_all_modes_and_only_apply_can_create() -> Result<()> {
    let tenants = ["tenant".to_owned()];
    for (mode, name, may_write) in [
        ("departments-plan", "department-plan", false),
        ("departments-apply", "department-apply", true),
        ("departments-reconcile", "department-reconcile", false),
        ("departments-verify", "department-verify", false),
    ] {
        let kind = department::kind(mode)?;
        assert_eq!(kind.name(), name);
        assert_eq!(command::stage(name)?, "seed-runtime");
        assert!(command::accepts(name, mode));
        for (op, values) in [
            (
                "list",
                json!({"tenant_id":"tenant","name":"dv-fixture-import"}),
            ),
            ("detail", json!({"tenant_id":"tenant","department_id":"42"})),
            ("template", json!({"tenant_id":"tenant"})),
        ] {
            department::check_operation(kind, &tenants, op, &values)?;
        }
        assert_eq!(
            department::check_operation(
                kind,
                &tenants,
                "create",
                &json!({"tenant_id":"tenant","body":{},"idempotency_key":"fixture"}),
            )
            .is_ok(),
            may_write
        );
        assert!(
            department::check_operation(
                kind,
                &tenants,
                "list",
                &json!({"tenant_id":"system","name":"dv-fixture-import"})
            )
            .is_err()
        );
        assert!(department::check_operation(kind, &tenants, "close", &json!({})).is_err());
    }
    let deadline = Instant::now() + Duration::from_secs(210);
    assert_eq!(Channel::Terminal.logout_deadline(deadline)?, Some(deadline));
    assert!(Channel::Unknown.logout_deadline(deadline).is_err());
    Ok(())
}

struct EofWriter {
    eof: mpsc::Sender<()>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl Write for EofWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.written.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EofWriter {
    fn drop(&mut self) {
        let _ = self.eof.send(());
    }
}

#[test]
fn identity_one_shot_closes_input_before_reading_exact_stdout() -> Result<()> {
    let (eof, released) = mpsc::channel();
    let written = Arc::new(Mutex::new(Vec::new()));
    let apply = identity_producer::kind("identities-apply")?;
    let verify = identity_producer::kind("identities-verify")?;
    assert_eq!(apply.name(), "identity-apply");
    assert_eq!(verify.name(), "identity-verify");
    assert!(command::accepts(apply.name(), "identities-apply"));
    assert!(command::accepts(verify.name(), "identities-verify"));
    assert!(!apply.session_framing());
    assert!(!verify.session_framing());
    assert!(identity_producer::kind("identities-reconcile").is_err());

    let mut stdout = serde_json::to_vec(&json!({"status":"prepared"}))?;
    stdout.push(b'\n');
    let pipes = pipes::Pipes::start(
        Box::new(EofWriter {
            eof,
            written: Arc::clone(&written),
        }),
        Box::new(AfterWrite {
            released: Some(released),
            bytes: Cursor::new(stdout),
        }),
        apply.session_framing(),
    )?;
    let grant = identity_producer::start_grant(
        Path::new("fixture"),
        7,
        &json!("identity-owner"),
        &json!({"fixture":"launch"}),
        &json!({"fixture":"process"}),
    );
    assert_eq!(
        grant,
        json!({"operation":"start","run_dir":"fixture","attempt":7,
        "identity_lock_owner":"identity-owner","launch":{"fixture":"launch"},
        "process":{"fixture":"process"}})
    );
    let raw = pipes.exchange(&grant, true, Instant::now() + Duration::from_secs(2))?;
    assert_eq!(
        identity_producer::checked_stdout(&raw)?,
        json!({"status":"prepared"})
    );
    assert!(
        pipes
            .exchange(&grant, true, Instant::now() + Duration::from_secs(2))
            .is_err()
    );
    pipes.finish()?;
    let captured = written.lock().unwrap();
    assert_eq!(captured.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(parse(&captured, "fixture")?, grant);
    Ok(())
}

#[test]
fn identity_one_shot_rejects_duplicate_or_non_object_stdout() {
    for invalid in [
        b"{}{}" as &[u8],
        br#"{"status":1,"status":2}"#,
        br#"{"summary":{"scope":1,"scope":2}}"#,
        b"[]",
        b"null",
    ] {
        assert!(identity_producer::checked_stdout(invalid).is_err());
    }
}

#[cfg(windows)]
#[test]
fn department_job_closes_after_logout_or_terminal_reply_without_replay() -> Result<()> {
    use std::{fs, process::Command};

    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local)?;
    let fixture = tempfile::Builder::new()
        .prefix("department-producer-job-")
        .tempdir_in(local)?;
    let executable = fixture.path().join("department-producer.exe");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/post_producer_child.rs");
    let compile = Command::new("rustc")
        .args([
            "--edition=2024",
            "-C",
            "debuginfo=0",
            "-C",
            "linker=rust-lld.exe",
        ])
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()?;
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    for mode in ["logout", "terminal"] {
        let trace = fixture.path().join(format!("{mode}-requests.ndjson"));
        let argv = vec![
            executable
                .to_str()
                .ok_or("生产者测试程序路径无效")?
                .to_owned(),
            mode.to_owned(),
            trace.to_str().ok_or("生产者测试记录路径无效")?.to_owned(),
        ];
        let log = fs::File::create_new(fixture.path().join(format!("{mode}.stderr")))?;
        let mut job = job::Job::spawn(fixture.path(), &argv, log)?;
        let identity = job.identity()?;
        let pipes = job.pipes(true)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let initialize = sequenced(1, "initialize", json!({}))?;
        let raw = pipes.exchange(&initialize, false, deadline)?;
        let reply = response(1, &parse(&raw, "部门初始化替身响应")?);
        let mut expected = vec![initialize];

        let exit = match reply {
            Reply::Data(data) => {
                assert_eq!(mode, "logout");
                assert_eq!(data, json!({}));
                assert_eq!(Channel::Open.logout_deadline(deadline)?, None);
                let close = sequenced(2, "close", json!({}))?;
                let raw = pipes.exchange(&close, false, deadline)?;
                assert!(matches!(
                    response(2, &parse(&raw, "部门注销替身响应")?),
                    Reply::Data(data) if data == json!({"closed":true})
                ));
                expected.push(close);
                0
            }
            Reply::Failure {
                diagnostic,
                channel,
            } => {
                assert_eq!(mode, "terminal");
                assert_eq!(
                    diagnostic,
                    json!({"id":1,"ok":false,"error_type":"NativeBridgeFailure","status":null})
                );
                assert_eq!(channel.logout_deadline(deadline)?, Some(deadline));
                17
            }
        };

        assert_eq!(job.wait(deadline)?.code(), Some(exit));
        files::combine(
            job.finish_until(Some(identity), deadline),
            pipes.finish_until(deadline),
        )?;
        let recorded = fs::read_to_string(trace)?
            .lines()
            .map(|line| parse(line.as_bytes(), "部门替身接收记录"))
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(recorded, expected);
    }
    Ok(())
}
