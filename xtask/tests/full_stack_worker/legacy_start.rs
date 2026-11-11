use super::*;

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=RyFrame Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn prepared(fixture: &Fixture, side: &str) -> PathBuf {
    let local = fixture.backend.join(".local-tests");
    let run = clone_status_run(&local, &fixture.run, side, fixture.api_port);
    fs::create_dir(run.join("results")).unwrap();
    let manifest_path = run.join("manifest.json");
    let mut manifest = read(&manifest_path);
    let selected = json!({"runtime_dir":fixture.run,"api_url":format!("http://127.0.0.1:{}",fixture.api_port)});
    let source = run.join("source.json");
    write(&source, &json!({"source":selected}));
    let target = run.join("target.json");
    write(&target, &json!({"generation":{"selected":selected}}));
    manifest["source_request"] = file_binding(&source);
    manifest["initialized"] = file_binding(&target);
    manifest["source_export"] = Value::Null;
    let environment = run.join("environment.json");
    manifest["source_environment"] = file_binding(&environment);
    manifest["target_environment"] = file_binding(&environment);
    write(&manifest_path, &manifest);
    write(
        run.join("state.json"),
        &json!({"format_version":1,"manifest":file_binding(&manifest_path),"attempts":[]}),
    );
    run
}

fn invoke(fixture: &Fixture, run: &Path, side: &str, operation: &str, roles: &[&str]) -> Output {
    clone_runtime(&fixture.backend, run, side, operation)
        .arg("--roles")
        .args(roles)
        .arg("--write")
        .output()
        .unwrap()
}

fn events(fixture: &Fixture) -> Vec<(String, String)> {
    read(fixture.run.join("producer-history.json"))["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| {
            (
                event["role"].as_str().unwrap().into(),
                event["event"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[test]
fn legacy_start_gates_target_and_rolls_back_only_its_own_source_processes() {
    let fixture = Fixture::with_role("normal", Role::Api);
    let root = &fixture.backend;
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir(root.join("xtask")).unwrap();
    fs::create_dir(root.join(".local-tests")).unwrap();
    fs::write(
        root.join(".gitignore"),
        "/.local-tests/\n/runtime*/\n/api\n/api.exe\n/worker\n/worker.exe\n",
    )
    .unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "--", "Cargo.toml", ".gitignore"]);
    git(
        root,
        &[
            "-c",
            "core.hooksPath=.git/no-hooks",
            "commit",
            "--no-gpg-sign",
            "-qm",
            "fixture",
        ],
    );
    let source = prepared(&fixture, "source");
    let target = prepared(&fixture, "target");
    rejected_status(
        invoke(&fixture, &target, "target", "start", &["worker"]),
        "阶段 runtime-target 未完成",
    );
    assert!(!fixture.run.join("worker.json").exists());
    assert!(!fixture.run.join("producer-history.json").exists());

    let worker_mode = fixture.run.join("worker-mode");
    fs::write(&worker_mode, "exit").unwrap();
    rejected_status(
        invoke(&fixture, &source, "source", "start", &["api", "worker"]),
        "阶段 runtime-source 未完成",
    );
    let api = read(fixture.run.join("api.json"));
    assert!(!process_liveness::process_is_running(
        api["identity"]["pid"].as_u64().unwrap() as u32
    ));
    let history = events(&fixture);
    assert_eq!(history[0], ("api".into(), "start-intent".into()));
    assert_eq!(history[1], ("api".into(), "started".into()));
    assert_eq!(history[2], ("worker".into(), "start-intent".into()));
    assert_eq!(
        history[history.len() - 2],
        ("worker".into(), "start-failed".into())
    );
    assert_eq!(
        history.last().unwrap(),
        &("api".into(), "startup-rollback".into())
    );
    fs::remove_file(worker_mode).unwrap();

    let start = invoke(&fixture, &source, "source", "start", &["worker", "api"]);
    let stop = invoke(&fixture, &source, "source", "stop", &["worker", "api"]);
    let summary = status_result(start);
    assert_eq!(
        summary,
        json!({"status":"stage_finished","stage":"runtime-source",
        "mode":"start","attempt":2,"restore_qualified":false})
    );
    status_result(stop);
    let result = read(source.join("results").join("0002.json"));
    assert_eq!(result["kind"], "devex-clone-runtime-control");
    assert_eq!(result["processes"]["worker"]["ready"], true);
    assert_eq!(result["processes"]["api"]["ready"], true);
    let events = events(&fixture);
    assert_eq!(
        &events[history.len()..],
        &[
            ("worker".into(), "start-intent".into()),
            ("worker".into(), "started".into()),
            ("api".into(), "start-intent".into()),
            ("api".into(), "started".into()),
            ("api".into(), "stopped".into()),
            ("worker".into(), "stopped".into()),
        ]
    );
    assert!(!fixture.run.join("worker-control.lock").exists());
    #[cfg(windows)]
    rejects_unknown_replay(&fixture, &source, &events);
}

#[cfg(windows)]
fn rejects_unknown_replay(fixture: &Fixture, source: &Path, history: &[(String, String)]) {
    let pending = fixture.run.join("worker.spawn-pending.json");
    let original = b"{}";
    fs::write(&pending, original).unwrap();
    let worker_receipt = fixture.run.join("worker.json");
    let previous = fs::read(&worker_receipt).unwrap();
    let output = invoke(fixture, source, "source", "start", &["worker"]);
    if output.status.success() {
        status_result(invoke(fixture, source, "source", "stop", &["worker"]));
    }
    rejected_status(output, "阶段 runtime-source 未完成");
    assert_eq!(events(fixture), history);
    assert_eq!(fs::read(worker_receipt).unwrap(), previous);
    assert_eq!(fs::read(pending).unwrap(), original);
}
