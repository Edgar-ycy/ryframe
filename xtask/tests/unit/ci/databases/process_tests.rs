use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn execute(input: &Input, cancel: &Cancellation) -> Checked<Value> {
    let plan = input.validate()?;
    runtime().block_on(prepare::execute_with(&plan, cancel, |directory| {
        client::invocation(plan.binding, client::Client::Local(fixture()), directory)
    }))
}

fn gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if crate::recovery::clone::native::resources::common::process_identity(pid)
            .unwrap()
            .is_none()
        {
            return;
        }
        assert!(Instant::now() < deadline, "进程 {pid} 未回收");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn successful_preparation_freezes_config_and_publishes_exact_private_targets() {
    let root = temporary();
    let mut value = request(root.path());
    let password = "中文🔐\"\\line\nnext\r\t\u{8}";
    value["settings"]["database"]["primary"]["password"] = json!(password);
    value["settings"]["database"]["primary"]["tls_mode"] = json!("verify_identity");
    value["settings"]["database"]["primary"]["tls_ca"] = json!("/tmp/ci-ca.pem");
    let input: Input = serde_json::from_value(value).unwrap();
    fs::write(
        input.config_dir.join("app.test.toml"),
        "scope_id='foreign'\n",
    )
    .unwrap();
    let result = execute(&input, &Cancellation::default()).unwrap();
    assert!(!result.to_string().contains(password));
    let config = input.output.join("config");
    let api: toml::Table =
        toml::from_str(&fs::read_to_string(config.join("app.test.toml")).unwrap()).unwrap();
    let worker: toml::Table = toml::from_str(
        &fs::read_to_string(input.output.join("worker-config/app.test.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(api["scope_id"].as_str(), Some("ci-42-3"));
    assert_eq!(api["snowflake_worker_id"].as_integer(), Some(3));
    assert_eq!(worker["snowflake_worker_id"].as_integer(), Some(2));
    let targets = api["tenant_data"]["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 3);
    for (index, target) in targets.iter().enumerate() {
        assert_eq!(target["password"].as_str(), Some(password));
        assert_eq!(target["tls_mode"].as_str(), Some("verify_identity"));
        assert_eq!(
            target["mode"].as_str(),
            Some(if index == 0 { "shared" } else { "dedicated" })
        );
        assert_eq!(target["max_connections"].as_integer(), Some(8));
    }
    assert_eq!(
        fs::read(config.join("app.toml")).unwrap(),
        input.common_text.as_bytes()
    );
    let sql = fs::read_to_string(config.join("observed-sql.txt")).unwrap();
    assert_eq!(sql.matches("CREATE DATABASE").count(), 4);
    assert_eq!(input.validate().err(), Some("replay_refused"));
    assert!(!sql.contains("IF NOT EXISTS"));
    let receipt = fs::read_to_string(input.output.join("database-targets.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&receipt).unwrap()["state"],
        "created"
    );
    assert!(!receipt.contains("private-password") && !receipt.contains(password));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(config.join("app.test.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn partial_write_and_client_crash_leave_unknown_evidence_and_prevent_replay() {
    for mode in ["fail", "crash"] {
        let root = temporary();
        let mut value = request(root.path());
        value["settings"]["database"]["primary"]["username"] = json!(mode);
        let input: Input = serde_json::from_value(value).unwrap();
        let result = execute(&input, &Cancellation::default());
        assert_eq!(result.err(), Some("creation_unknown"));
        let receipt = fs::read_to_string(input.output.join("database-targets.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&receipt).unwrap()["state"],
            "creation-unknown"
        );
        assert!(!receipt.contains("private-password"));
        assert!(!input.output.join("config/app.test.toml").exists());
        assert!(input.output.join("config/observed-sql.txt").exists());
        assert_eq!(input.validate().err(), Some("replay_refused"));
    }
}

#[test]
fn cancellation_reclaims_client_descendant_and_keeps_unknown_receipt() {
    let root = temporary();
    let mut value = request(root.path());
    value["settings"]["database"]["primary"]["username"] = json!("hang");
    let input: Input = serde_json::from_value(value).unwrap();
    let cancel = Cancellation::default();
    let trigger = cancel.clone();
    let config = input.output.join("config");
    let observed = config.clone();
    let monitor = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !observed.join("descendant.pid").is_file() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        trigger.cancel();
    });
    assert_eq!(execute(&input, &cancel).err(), Some("creation_unknown"));
    monitor.join().unwrap();
    for name in ["client.pid", "descendant.pid"] {
        gone(
            fs::read_to_string(config.join(name))
                .unwrap()
                .parse()
                .unwrap(),
        );
    }
    let receipt: Value =
        serde_json::from_slice(&fs::read(input.output.join("database-targets.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["state"], "creation-unknown");
}

#[test]
fn successful_client_exit_still_reclaims_its_orphan() {
    let root = temporary();
    let mut value = request(root.path());
    value["settings"]["database"]["primary"]["username"] = json!("orphan");
    let input: Input = serde_json::from_value(value).unwrap();
    execute(&input, &Cancellation::default()).unwrap();
    gone(
        fs::read_to_string(input.output.join("config/descendant.pid"))
            .unwrap()
            .parse()
            .unwrap(),
    );
}

#[test]
fn cancellation_before_preparation_has_no_write_and_existing_artifacts_prevent_all_replay() {
    let root = temporary();
    let value = request(root.path());
    let input: Input = serde_json::from_value(value).unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(execute(&input, &cancel).err(), Some("request_cancelled"));
    assert_eq!(fs::read_dir(&input.output).unwrap().count(), 0);
    for name in ["config", "worker-config", "database-targets.json"] {
        let path = input.output.join(name);
        fs::write(&path, b"existing").unwrap();
        assert_eq!(
            execute(&input, &Cancellation::default()).err(),
            Some("replay_refused")
        );
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn timeout_cleans_tree_and_options_even_when_client_never_returns() {
    let root = temporary();
    let mut binding = binding();
    binding["username"] = json!("hang");
    let invocation =
        client::invocation(&binding, client::Client::Local(fixture()), root.path()).unwrap();
    let options = invocation._options.path().to_owned();
    let sql = b"CREATE DATABASE a;CREATE DATABASE b;CREATE DATABASE c;CREATE DATABASE d;";
    assert_eq!(
        runtime()
            .block_on(prepare::run(
                invocation,
                sql,
                root.path(),
                &Cancellation::default(),
                Duration::from_secs(1)
            ))
            .err(),
        Some("client_timeout")
    );
    assert!(!options.exists());
    for name in ["client.pid", "descendant.pid"] {
        gone(
            fs::read_to_string(root.path().join(name))
                .unwrap()
                .parse()
                .unwrap(),
        );
    }
}
