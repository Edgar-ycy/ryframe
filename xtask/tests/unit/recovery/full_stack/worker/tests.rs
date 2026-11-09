use super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Instant,
};

#[test]
fn bound_start_environment_keeps_file_identity_and_never_uses_host_values() {
    let root = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&root).unwrap();
    let temporary = tempfile::tempdir_in(root).unwrap();
    let path = temporary.path().join("environment.json");
    let original =
        json!({"environment":{"APP_ENV":"test","TEMP":"private-temp","TMP":"private-tmp"}});
    operation::write_new(&path, &original).unwrap();
    let mut file = operation::file(&path).unwrap();
    file["path"] = json!(path);
    let environment = environment::Environment::capture(&file).unwrap();
    let binding = environment.binding().clone();
    let mut command = std::process::Command::new("unused-test-command");
    command
        .env("TEMP", "host-sentinel")
        .env("TMP", "host-sentinel");
    environment.apply(&mut command).unwrap();
    let values = command
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(values["APP_ENV"].as_deref(), Some("test"));
    assert_eq!(values["TEMP"].as_deref(), Some("private-temp"));
    assert_eq!(values["TMP"].as_deref(), Some("private-tmp"));
    assert!(
        !serde_json::to_string(&binding)
            .unwrap()
            .contains("private-temp")
    );
    let changed = std::fs::write(
        &path,
        serde_json::to_vec(&json!({"environment":{"APP_ENV":"test"}})).unwrap(),
    );
    if changed.is_ok() {
        assert!(environment.check().is_err());
    }
    drop(environment);
    // 即使恢复相同字节，替换文件也不能冒充最初的文件身份。
    let replacement = temporary.path().join("replacement.json");
    operation::write_new(&replacement, &original).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert_eq!(operation::file(&path).unwrap()["sha256"], file["sha256"]);
    assert!(environment::Environment::recorded(&binding).is_err());
    std::fs::write(&path, b"{\"environment\":{\"APP_ENV\":\"test\"}}").unwrap();
    let mut absent = operation::file(&path).unwrap();
    absent["path"] = json!(path);
    let environment = environment::Environment::capture(&absent).unwrap();
    environment.apply(&mut command).unwrap();
    let values = command
        .get_envs()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(values[std::ffi::OsStr::new("TEMP")], None);
    assert_eq!(values[std::ffi::OsStr::new("TMP")], None);
}

#[test]
fn start_environment_rejects_invalid_private_inputs_and_wrong_operation() {
    let root = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&root).unwrap();
    let temporary = tempfile::tempdir_in(root).unwrap();
    let path = temporary.path().join("environment.json");
    for bytes in [
        r#"{"environment":{"APP_ENV":"prod"}}"#,
        r#"{"environment":{"APP_ENV":"test","PASSWORD":"synthetic-secret"}}"#,
        r#"{"environment":{"APP_ENV":"test","TEMP":""}}"#,
        r#"{"environment":{"APP_ENV":"test","TMP":4}}"#,
        r#"{"environment":{"APP_ENV":"test","APP_ENV":"test"}}"#,
        r#"{"environment":{"APP_ENV":"test"},"extra":true}"#,
    ] {
        std::fs::write(&path, bytes).unwrap();
        let mut file = operation::file(&path).unwrap();
        file["path"] = json!(path);
        assert!(environment::Environment::capture(&file).is_err());
    }
    let binding =
        json!({"file":{"path":path,"bytes":10,"sha256":"d".repeat(64)},"identity":[u64::MAX,2]});
    let mut args = fixture();
    args["request"]["environment_binding"] = binding.clone();
    assert!(receipts::execute(&args).is_ok());
    args["request"]["environment_binding"]["identity"] = json!([1]);
    assert!(receipts::execute(&args).is_err());
    args["request"]["environment_binding"] = binding;
    args["request"]["operation"] = json!("status");
    args["request"]["log"] = Value::Null;
    assert!(receipts::execute(&args).is_err());
    assert!(
        start::reserve(
            role::Role::Frontend,
            temporary.path(),
            temporary.path(),
            &Value::Null
        )
        .is_err()
    );
    assert!(!temporary.path().join("worker-control.lock").exists());
    assert!(!temporary.path().join("api-control.lock").exists());
}

fn common(kind: &str) -> Value {
    json!({"format_version":1,"kind":kind,"operation_id":"a".repeat(32),
        "runtime_directory":std::env::current_dir().unwrap(),"source_sha256":"b".repeat(64)})
}

#[test]
fn process_tree_cannot_cross_role_scope_or_product_identity() {
    let root = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&root).unwrap();
    let temporary = tempfile::tempdir_in(root).unwrap();
    let directory = operation::canonical(temporary.path()).unwrap();
    let runtime = json!({"scope_id":"role-isolation"});
    let identity =
        json!({"pid":42,"started":"12345","executable":std::env::current_exe().unwrap()});
    let mut monitor = identity.clone();
    monitor["pid"] = json!(43);
    for name in ["api", "worker", "frontend"] {
        let role = role::Role::parse(&json!(name)).unwrap();
        let tree_path = directory.join(format!("{name}-tree.json"));
        let product_path = directory.join(format!("{name}.json"));
        let original = json!({"format_version":2,"kind":"full-stack-process-tree",
            "runtime_directory":directory,"role":name,"scope_id":"role-isolation",
            "operation_id":"a".repeat(32),"supervisor":identity,"process":identity,
            "monitor":monitor,"group_id":42});
        operation::write_new(&tree_path, &original).unwrap();
        operation::write_new(&product_path, &json!({"identity":identity})).unwrap();
        assert_eq!(
            tree::read(&directory, &runtime, role, true).unwrap(),
            Some(original.clone())
        );
        for (field, invalid) in [
            ("role", json!("other")),
            ("scope_id", json!("other")),
            ("process", monitor.clone()),
        ] {
            let mut changed = original.clone();
            changed[field] = invalid;
            crate::recovery::clone::native::json::write_replace(&tree_path, &changed).unwrap();
            assert!(tree::read(&directory, &runtime, role, true).is_err());
        }
        crate::recovery::clone::native::json::write_replace(&tree_path, &original).unwrap();
    }
    for invalid in [Value::Null, json!("../api"), json!("Api"), json!("runtime")] {
        assert!(role::Role::parse(&invalid).is_err());
    }
}

fn fixture() -> Value {
    let identity =
        json!({"pid":42,"started":"12345","executable":std::env::current_exe().unwrap()});
    let mut owner = common("full-stack-worker-control-owner");
    owner["identity"] = identity.clone();
    owner["created_at_ns"] = json!(1);
    let mut request = common("full-stack-worker-control-request");
    for (key, value) in json!({"operation":"start","backend_root":"backend","scope_id":"隔离😀",
        "controller_identity":identity,"owner_sha256":documents::hash(&owner).unwrap(),
        "runtime_receipt_sha256":"c".repeat(64),"worker_artifact":{"path":"worker.exe","bytes":10,"sha256":"d".repeat(64)},
        "worker_ready_url":"http://127.0.0.1:19210/readyz","timeout_seconds":0.2,
        "log":format!("worker-{}.log","a".repeat(32)),"requested_at_ns":2}).as_object().unwrap() {
        request[key] = value.clone();
    }
    json!({"mode":"operation","directory":owner["runtime_directory"],"source":owner["source_sha256"],
        "owner":owner,"request":request,"value":null,"candidate":null,"supervisor":null,
        "runtime_sha256":"c".repeat(64),"backend":"backend","runtime":{
            "scope_id":"隔离😀","artifacts":{"worker":request["worker_artifact"]},
            "worker_ready_url":request["worker_ready_url"]}})
}

fn bind(args: &Value, kind: &str) -> Value {
    let mut value = common(kind);
    value["owner_sha256"] = json!(documents::hash(&args["owner"]).unwrap());
    value["request_sha256"] = json!(documents::hash(&args["request"]).unwrap());
    value
}

fn launch() -> Value {
    let mut args = fixture();
    let mut candidate = bind(&args, "full-stack-worker-control-candidate");
    candidate["identity"] = args["owner"]["identity"].clone();
    candidate["created_at_ns"] = json!(3);
    let mut supervisor = bind(&args, "full-stack-worker-control-supervisor");
    supervisor["identity"] = candidate["identity"].clone();
    supervisor["authorized_at_ns"] = json!(4);
    supervisor["candidate_sha256"] = json!(documents::hash(&candidate).unwrap());
    args["candidate"] = candidate;
    args["supervisor"] = supervisor;
    args
}

fn successful(args: &Value) -> Value {
    let mut value = bind(args, "full-stack-worker-control-result");
    for key in ["operation", "scope_id", "controller_identity", "log"] {
        value[key] = args["request"][key].clone();
    }
    for (key, value2) in
        json!({"outcome":"succeeded","state":"running","identity":args["owner"]["identity"],
        "reconciled":false,"error_type":null,"error":null,"completed_at_ns":6})
        .as_object()
        .unwrap()
    {
        value[key] = value2.clone();
    }
    value
}

#[test]
fn role_contracts_reject_cross_role_artifacts_and_running_finite_results() {
    for role in [role::Role::Api, role::Role::Frontend] {
        let mut args = fixture();
        args["owner"]["kind"] = json!(role.kind("owner"));
        args["request"]["kind"] = json!(role.kind("request"));
        args["request"]["owner_sha256"] = json!(documents::hash(&args["owner"]).unwrap());
        let artifact = args["request"]
            .as_object_mut()
            .unwrap()
            .remove("worker_artifact")
            .unwrap();
        let ready = args["request"]
            .as_object_mut()
            .unwrap()
            .remove("worker_ready_url")
            .unwrap();
        args["request"][role.artifact()] = artifact.clone();
        args["request"][role.ready()] = ready.clone();
        args["runtime"]["artifacts"] = json!({(role.name()):artifact});
        args["runtime"]
            .as_object_mut()
            .unwrap()
            .remove("worker_ready_url");
        args["runtime"][role.ready()] = ready;
        args["request"]["log"] = json!(format!("{}-{}.log", role.name(), "a".repeat(32)));
        if role == role::Role::Frontend {
            args["request"]["operation"] = json!("run");
            args["request"]["log"] = Value::Null;
            args["request"][role.ready()] = json!("exit-and-members");
            args["runtime"][role.ready()] = json!("exit-and-members");
            args["request"]["request_binding"] = json!({"descriptor":{},"state":{}});
            args["request"]["timeout_seconds"] = json!(1800);
        }
        args["mode"] = json!("request");
        receipts::execute(&args).unwrap();
        let mut changed = args.clone();
        changed["runtime"]["artifacts"][role.name()]["path"] = json!("different.exe");
        assert!(receipts::execute(&changed).is_err());
        changed = args.clone();
        changed["owner"]["kind"] = json!("full-stack-worker-control-owner");
        assert!(receipts::execute(&changed).is_err());
        if role == role::Role::Frontend {
            args["mode"] = json!("result");
            args["value"] = successful(&args);
            args["value"]["kind"] = json!(role.kind("result"));
            assert!(receipts::execute(&args).is_err());
            args["value"]["state"] = json!("stopped");
            args["value"]["identity"] = Value::Null;
            receipts::execute(&args).unwrap();
        }
    }
}

#[test]
fn operation_rejects_changed_owner_source_log_and_unknown_fields() {
    let original = fixture();
    receipts::execute(&original).unwrap();
    for (pointer, value) in [
        ("/owner/created_at_ns", json!(true)),
        ("/owner/identity/pid", json!(true)),
        ("/owner/identity/started", json!("")),
        ("/request/log", json!("elsewhere.log")),
        ("/request/runtime_receipt_sha256", json!("e".repeat(64))),
        ("/request/operation", json!("restart")),
        ("/request/owner_sha256", json!("e".repeat(64))),
        ("/source", json!("e".repeat(64))),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(receipts::execute(&changed).is_err(), "{pointer}");
    }
    let mut changed = original;
    changed["owner"]["extra"] = json!(1);
    assert!(receipts::execute(&changed).is_err());
}

#[test]
fn request_binds_worker_config_and_never_accepts_boolean_or_nonpositive_timeout() {
    let mut original = fixture();
    original["mode"] = json!("request");
    receipts::execute(&original).unwrap();
    for (pointer, value) in [
        ("/request/timeout_seconds", json!(true)),
        ("/request/timeout_seconds", json!(0)),
        ("/request/timeout_seconds", json!(181)),
        ("/request/timeout_seconds", json!("60")),
        ("/request/scope_id", json!("other")),
        ("/backend", json!("other")),
        ("/runtime/artifacts/worker/path", json!("api.exe")),
        (
            "/runtime/worker_ready_url",
            json!("http://127.0.0.1:8080/readyz"),
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(receipts::execute(&changed).is_err(), "{pointer}");
    }
}

#[test]
fn child_authorization_binds_candidate_and_exact_creation_identity() {
    let mut args = launch();
    args["mode"] = json!("supervisor");
    args["value"] = args["supervisor"].clone();
    receipts::execute(&args).unwrap();
    for (pointer, value) in [
        ("/candidate/identity/started", json!("999")),
        ("/value/authorized_at_ns", json!(false)),
        ("/value/identity/executable", json!("relative.exe")),
        ("/value/candidate_sha256", json!("e".repeat(64))),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(receipts::execute(&changed).is_err(), "{pointer}");
    }
}

#[test]
fn progress_requires_authorized_supervisor_and_phase_identity() {
    let mut args = launch();
    args["mode"] = json!("progress");
    let mut progress = bind(&args, "full-stack-worker-control-progress");
    progress["phase"] = json!("launching");
    progress["identity"] = Value::Null;
    progress["supervisor_sha256"] = json!(documents::hash(&args["supervisor"]).unwrap());
    progress["updated_at_ns"] = json!(5);
    args["value"] = progress;
    receipts::execute(&args).unwrap();
    args["value"]["phase"] = json!("ready");
    assert!(receipts::execute(&args).is_err());
    args["value"]["identity"] = args["owner"]["identity"].clone();
    receipts::execute(&args).unwrap();
    args["value"]["phase"] = json!("launching");
    assert!(receipts::execute(&args).is_err());
    args["value"]["phase"] = json!("started");
    args["supervisor"]["authorized_at_ns"] = json!(9);
    assert!(receipts::execute(&args).is_err());
}

#[test]
fn successful_result_cannot_hide_unknown_state_error_or_missing_identity() {
    let mut args = fixture();
    args["mode"] = json!("result");
    args["value"] = successful(&args);
    receipts::execute(&args).unwrap();
    for (pointer, value) in [
        ("/value/state", json!("unknown")),
        ("/value/state", json!("stopped")),
        ("/value/identity", Value::Null),
        ("/value/error", json!("failure")),
        ("/value/reconciled", json!(1)),
        ("/value/completed_at_ns", json!(true)),
        ("/value/controller_identity/pid", json!(55)),
    ] {
        let mut changed = args.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(receipts::execute(&changed).is_err(), "{pointer}");
    }
}

#[test]
fn failed_results_preserve_unknown_identity_and_require_both_error_fields() {
    let mut args = fixture();
    args["mode"] = json!("result");
    args["value"] = successful(&args);
    args["value"]["outcome"] = json!("failed");
    args["value"]["state"] = json!("unknown");
    args["value"]["error_type"] = json!("InterruptedControl");
    args["value"]["error"] = json!("未确认回收");
    receipts::execute(&args).unwrap();
    args["value"]["state"] = json!("running");
    assert!(receipts::execute(&args).is_err());
    args["value"]["state"] = json!("stopped");
    assert!(receipts::execute(&args).is_err());
    args["value"]["identity"] = Value::Null;
    receipts::execute(&args).unwrap();
    args["value"]["error_type"] = json!("");
    assert!(receipts::execute(&args).is_err());
}

#[test]
fn completed_operations_enforce_their_exact_goal_without_replaying_start() {
    for operation in ["stop", "crash", "status", "reconcile"] {
        let mut args = fixture();
        args["request"]["operation"] = json!(operation);
        args["request"]["log"] = Value::Null;
        args["mode"] = json!("result");
        args["value"] = successful(&args);
        if matches!(operation, "stop" | "crash") {
            assert!(receipts::execute(&args).is_err());
        } else {
            receipts::execute(&args).unwrap();
        }
        args["value"]["state"] = json!("stopped");
        args["value"]["identity"] = Value::Null;
        receipts::execute(&args).unwrap();
        args["value"]["state"] = json!("unknown");
        assert!(receipts::execute(&args).is_err());
    }
}

#[test]
fn protocol_rejects_duplicates_boolean_versions_trailing_and_oversized_inputs() {
    for input in [
        b"{}".as_slice(),
        b"{\"a\":1,\"a\":2}",
        b"{}{}",
        b"[]",
        b"\xff",
    ] {
        assert!(execute(input).is_err());
    }
    assert!(execute(&vec![b' '; LIMIT + 1]).is_err());
    let mut value = json!({"format_version":true,"kind":"ryframe-full-stack-worker","operation":"validate","arguments":fixture()});
    assert!(execute(&serde_json::to_vec(&value).unwrap()).is_err());
    value["format_version"] = json!(1);
    execute(&serde_json::to_vec(&value).unwrap()).unwrap();
}

#[test]
fn ascii_canonical_digest_preserves_unicode_and_python_numeric_lexemes() {
    let value = strict::parse_fragment(
        r#"{"b":1.0,"a":["中文","😀","\u007f"],"c":1e-07}"#,
        "fixture",
    )
    .unwrap();
    assert_eq!(
        documents::hash(&value).unwrap(),
        "60f6c2942df1a9ec1112c464e4d3b96a197144f03683d8a1d6b9efe6bbad5e91"
    );
}

fn probe(action: &str, url: String, timeout: f64) -> Result<Value> {
    probes::execute(&json!({"action":action,"url":url,"timeout":timeout}))
}

#[test]
fn occupied_port_rejects_status_and_timeout_then_accepts_closed_port() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    assert!(
        probe("free", format!("http://{address}/readyz"), 0.0)
            .unwrap_err()
            .to_string()
            .contains("端口已占用")
    );
    assert!(
        probe("wait-free", format!("http://{address}/readyz"), 0.05)
            .unwrap_err()
            .to_string()
            .contains("仍被占用")
    );
    drop(listener);
    probe("free", format!("http://{address}/readyz"), 0.0).unwrap();
}

#[test]
fn readiness_accepts_200_and_never_follows_redirect_or_waits_forever() {
    for (status, expected) in [
        ("200 OK", true),
        ("302 Found", false),
        ("500 Failed", false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).unwrap() > 0);
            write!(stream,"HTTP/1.1 {status}\r\nLocation: http://{address}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert_eq!(
            probe("ready", format!("http://{address}/readyz"), 0.0).unwrap(),
            json!(expected)
        );
        server.join().unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(std::time::Duration::from_millis(1200));
    });
    let started = Instant::now();
    assert_eq!(
        probe("ready", format!("http://{address}/readyz"), 0.0).unwrap(),
        json!(false)
    );
    assert!(started.elapsed().as_secs_f64() < 2.0);
    server.join().unwrap();
}

#[test]
fn role_atomic_receipts_support_long_unicode_paths_and_preserve_noclobber() {
    let local = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let mut directory = operation::canonical(temporary.path()).unwrap();
    while directory.to_string_lossy().encode_utf16().count() < 230 {
        directory.push("中文 空间-1234567890");
    }
    std::fs::create_dir_all(&directory).unwrap();
    for role in [role::Role::Api, role::Role::Worker, role::Role::Frontend] {
        let path = directory.join(format!(
            "{}-members-{}-ready.json",
            role.name(),
            "a".repeat(32)
        ));
        assert!(path.to_string_lossy().encode_utf16().count() > 260);
        let original = json!({"role":role.name(),"path":path,"status":"原字节"});
        operation::write_new(&path, &original).unwrap();
        assert_eq!(operation::read(&path).unwrap(), original);
        let bytes = std::fs::read(&path).unwrap();
        assert!(operation::write_new(&path, &json!({"status":"不得覆盖"})).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 3);
}

#[test]
fn atomic_receipt_duplicate_retains_io_error_without_changing_original() {
    let local = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let directory = operation::canonical(temporary.path()).unwrap();
    let path = directory.join("receipt.json");
    operation::write_new(&path, &json!({"original":true})).unwrap();
    let original = std::fs::read(&path).unwrap();
    let error = operation::write_new(&path, &json!({"original":false})).unwrap_err();
    let io = error
        .downcast_ref::<std::io::Error>()
        .expect("必须保留PersistError.error的IO类型");
    assert_eq!(io.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(io.raw_os_error().is_some());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
}

#[test]
fn operation_atomic_replacement_keeps_control_target_and_staging_boundary() {
    let local = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&local).unwrap();
    let temporary = tempfile::tempdir_in(local).unwrap();
    let directory = operation::canonical(temporary.path()).unwrap();
    let runtime = json!({"scope_id":"atomic-receipt","worker_ready_url":"http://127.0.0.1:19210/readyz",
        "artifacts":{"worker":{"path":std::env::current_exe().unwrap(),"sha256":"d".repeat(64)}}});
    operation::write_new(&directory.join("runtime.json"), &runtime).unwrap();
    let op = operation::Operation::claim(
        role::Role::Worker,
        "start",
        &directory,
        &directory,
        &runtime,
        60.0,
    )
    .unwrap();
    op.publish("progress.json", &json!({"stage":"original"}), false)
        .unwrap();
    op.publish("progress.json", &json!({"stage":"replacement"}), true)
        .unwrap();
    assert_eq!(
        operation::read(&op.path.join("progress.json")).unwrap()["stage"],
        "replacement"
    );
    assert!(!directory.join("progress.json").exists());
    std::fs::remove_file(op.path.join("progress.json")).unwrap();
    std::fs::create_dir(op.path.join("progress.json")).unwrap();
    let error = op
        .publish("progress.json", &json!({"stage":"must-fail"}), true)
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error()
            .is_some()
    );
    assert!(op.path.join("progress.json").is_dir());
    assert_eq!(std::fs::read_dir(&op.path).unwrap().count(), 3);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
}
