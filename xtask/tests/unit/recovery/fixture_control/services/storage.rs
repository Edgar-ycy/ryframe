use super::*;
use crate::recovery::clone::native::lifecycle::{lock::RunLock, state};

fn diagnostic_output() -> tempfile::TempDir {
    let root = crate::workspace::root_dir().join(".local-tests");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn monitor_diagnostic_keeps_fixed_rejections_and_omits_private_io_messages() {
    let output = diagnostic_output();
    let rejection = check(false, "本次 RustFS 启动已取消").unwrap_err();
    let io = std::io::Error::other("D:/private/fixture key=synthetic");
    monitor::publish_failure_diagnostic(
        output.path(),
        "operation",
        Some(rejection.as_ref()),
        Some(&io),
    );
    assert_eq!(
        document::read(&output.path().join("actor-failure-diagnostic.json")).unwrap(),
        json!({
            "format_version": 1,
            "operation_id": "operation",
            "observation": {"category":"rejected","reason":"本次 RustFS 启动已取消"},
            "cleanup": {"category":"io","os_code":null},
        }),
    );
}

#[test]
fn monitor_diagnostic_preserves_first_record_without_exposing_unknown_error_text() {
    let output = diagnostic_output();
    let arbitrary: Box<dyn std::error::Error> = "D:/private/fixture key=synthetic".into();
    monitor::publish_failure_diagnostic(output.path(), "original", Some(arbitrary.as_ref()), None);
    let path = output.path().join("actor-failure-diagnostic.json");
    let original = std::fs::read(&path).unwrap();
    assert_eq!(
        document::read(&path).unwrap(),
        json!({
            "format_version": 1,
            "operation_id": "original",
            "observation": {"category":"unknown"},
            "cleanup": null,
        }),
    );
    let os = std::io::Error::from_raw_os_error(5);
    monitor::publish_failure_diagnostic(output.path(), "other", None, Some(&os));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let output = diagnostic_output();
    monitor::publish_failure_diagnostic(output.path(), "other", None, Some(&os));
    let diagnostic = document::read(&output.path().join("actor-failure-diagnostic.json")).unwrap();
    assert_eq!(diagnostic["cleanup"], json!({"category":"io","os_code":5}));
}

#[cfg(windows)]
#[test]
fn monitor_diagnostic_retains_windows_system_code() {
    let output = diagnostic_output();
    let error = winsafe::co::ERROR::ACCESS_DENIED;
    monitor::publish_failure_diagnostic(output.path(), "operation", Some(&error), None);
    let diagnostic = document::read(&output.path().join("actor-failure-diagnostic.json")).unwrap();
    assert_eq!(
        diagnostic["observation"],
        json!({"category":"win32","os_code":5})
    );
}

#[test]
fn frozen_inputs_reject_changed_bytes_and_replaced_kernel_file_identity() {
    let root = crate::workspace::root_dir().join(".local-tests");
    let directory = tempfile::tempdir_in(root).unwrap();
    let path = directory.path().join("source.json");
    std::fs::write(&path, b"original").unwrap();
    let original = binding::snapshot(&path).unwrap();
    std::fs::write(&path, b"changed!").unwrap();
    assert!(binding::verify_snapshot(&original).is_err());
    std::fs::write(&path, b"original").unwrap();
    binding::verify_snapshot(&original).unwrap();
    let replacement = directory.path().join("replacement.json");
    std::fs::write(&replacement, b"original").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(replacement, &path).unwrap();
    assert!(binding::verify_snapshot(&original).is_err());
}

#[test]
fn foreign_controller_owner_is_rejected_before_intent_or_process_launch() {
    let backend = crate::workspace::root_dir();
    let directory = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
    let run = directory.path();
    let output = run.join("rustfs");
    let data = run.join("data");
    std::fs::create_dir(&output).unwrap();
    std::fs::create_dir(&data).unwrap();
    document::write_new(
        &run.join("manifest.json"),
        &json!({
        "kind":"reference-fixture-service-run", "scope_id":"owner-risk"}),
    )
    .unwrap();
    state::initialize(run).unwrap();
    let (device, inode) =
        common::device_inode_for(&data, &std::fs::metadata(&data).unwrap()).unwrap();
    let request = json!({"scope_id":"owner-risk", "executable":{},
        "data_directory":{"path":data,"device":device,"inode":inode},
        "api_url":"http://127.0.0.1:22860","console_url":"http://127.0.0.1:22861",
        "credential_files":{},"timeout_seconds":60});
    document::write_new(&output.join("request.json"), &request).unwrap();
    let lock = RunLock::claim(run).unwrap();
    let sources = json!({"request":common::binding(&output.join("request.json")).unwrap()});
    let (number, attempt) = state::begin(run, "storage-target", "initial", &sources).unwrap();
    state::bind_controller(run, number, &lock.owner, &attempt).unwrap();
    let controller_path = state::controller_path(run, number);
    let mut controller = document::read(&controller_path).unwrap();
    controller["owner"]["manifest_sha256"] = json!("0".repeat(64));
    document::write_replace(&controller_path, &controller).unwrap();
    let controller = common::binding(&controller_path).unwrap();
    let manifest = common::binding(&run.join("manifest.json")).unwrap();
    let before = std::fs::read(output.join("request.json")).unwrap();
    let error = start(
        Start {
            backend: &backend,
            request: &request,
            private: &json!({"APP_ENV":"test"}),
            output: &output,
            manifest: &manifest,
            controller: &controller,
            number,
        },
        || Ok(()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("owner"));
    for name in [
        "intent.json",
        "actor.json",
        "spawned.json",
        "ready.json",
        "rustfs-tree.json",
    ] {
        assert!(!output.join(name).exists(), "{name}");
    }
    assert_eq!(std::fs::read(output.join("request.json")).unwrap(), before);
    assert_eq!(
        state::load(run, false).unwrap()["attempts"][0]["status"],
        "running"
    );
    lock.release().unwrap();
}

#[test]
fn actor_root_probe() {
    let root = crate::workspace::root_dir();
    assert!(root.join("Cargo.toml").is_file() && root.join("xtask").is_dir());
    println!(
        "RYFRAME_ACTOR_WORKSPACE={}",
        std::fs::canonicalize(root).unwrap().display()
    );
}

#[test]
fn actor_entry_root_uses_coordinator_when_output_has_an_independent_workspace() {
    let root = crate::workspace::root_dir();
    let temporary = tempfile::tempdir_in(root.join(".local-tests")).unwrap();
    let coordinator = temporary.path().join("S");
    let product = coordinator.join(".local-tests/P");
    let output = product.join(".local-tests/runtime/rustfs");
    for workspace in [&coordinator, &product] {
        std::fs::create_dir_all(workspace.join("xtask")).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), b"[workspace]\n").unwrap();
    }
    std::fs::create_dir_all(&output).unwrap();
    let actor = actor::Actor {
        backend: coordinator.clone(),
        output,
        request: json!({}),
        private: json!({}),
        manifest: Value::Null,
        controller: Value::Null,
        number: 1,
        operation_id: "a".repeat(32),
        tool: common::binding(&std::env::current_exe().unwrap()).unwrap(),
        snapshots: Vec::new(),
        owner_binding: Value::Null,
    };
    document::write_new(
        &actor.path("actor.json"),
        &serde_json::to_value(&actor).unwrap(),
    )
    .unwrap();
    let encoded =
        serde_json::to_string(&common::binding(&actor.path("actor.json")).unwrap()).unwrap();
    let command = launch::command(&actor, "monitor", &encoded).unwrap();
    let probe = format!(
        "{}::actor_root_probe",
        module_path!().split_once("::").unwrap().1
    );
    let result = std::process::Command::new(command.get_program())
        .current_dir(command.get_current_dir().unwrap())
        .args(["--exact", &probe, "--nocapture"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let observed = String::from_utf8(result.stdout).unwrap();
    let expected = format!(
        "RYFRAME_ACTOR_WORKSPACE={}",
        std::fs::canonicalize(coordinator).unwrap().display()
    );
    assert!(observed.lines().any(|line| line == expected), "{observed}");
}
