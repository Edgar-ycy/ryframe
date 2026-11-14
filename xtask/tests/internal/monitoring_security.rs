use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::{
    recovery::monitoring::{config, execution::ProtectedInputs, permissions, staging},
    workspace::root_dir,
};

pub(crate) const RETIRED_MONITORING_KEYS: [&str; 2] =
    ["APP_MONITOR_METRICS_BEARER_TOKEN", "APP_SCOPE_ID"];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = root_dir()
            .join(".local-tests")
            .join(format!("monitor-security-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn valid_acl() -> Value {
    json!({
        "current_sid": "S-1-5-21-123-456-789-1001",
        "owner_sid": "S-1-5-21-123-456-789-1001",
        "protected": true,
        "entries": [
            {"sid": "S-1-5-21-123-456-789-1001", "type": 0, "rights": 1179785, "inherited": false},
            {"sid": "S-1-5-18", "type": 0, "rights": 2032127, "inherited": false},
        ],
    })
}

#[test]
fn windows_acl_accepts_only_explicit_read_for_the_private_owner_and_system() {
    let value = valid_acl();
    let normalized = permissions::validate_windows_acl(&value).unwrap();
    assert_eq!(normalized["owner_sid"], value["owner_sid"]);
    let mut reordered = value.clone();
    reordered["entries"].as_array_mut().unwrap().reverse();
    assert_eq!(
        permissions::validate_windows_acl(&reordered).unwrap(),
        normalized
    );
    let mut split = value.clone();
    let mut synchronize = split["entries"][0].clone();
    synchronize["rights"] = json!(0x0010_0000);
    split["entries"][0]["rights"] = json!(0x0002_0001);
    split["entries"].as_array_mut().unwrap().push(synchronize);
    permissions::validate_windows_acl(&split).unwrap();
    for owner in ["S-1-5-18", "S-1-5-32-544"] {
        let mut value = value.clone();
        value["owner_sid"] = json!(owner);
        assert!(permissions::validate_windows_acl(&value).is_ok());
    }
}

#[test]
fn windows_acl_rejects_unknown_owner_broad_subject_deny_inheritance_and_missing_read() {
    let original = valid_acl();
    for (pointer, bad) in [
        ("/owner_sid", json!("S-1-5-21-999")),
        ("/protected", json!(false)),
        ("/entries", json!([])),
        ("/entries/0/sid", json!("S-1-1-0")),
        ("/entries/0/sid", json!("S-1-5-32-545")),
        ("/entries/0/sid", json!("S-1-5-21-999")),
        ("/entries/0/type", json!(1)),
        ("/entries/0/type", json!(9)),
        ("/entries/0/inherited", json!(true)),
        ("/entries/0/rights", json!(0)),
        ("/entries/0/rights", json!(0x0002_0001)),
        ("/entries/0/rights", json!(0x0012_0000)),
        ("/entries/0/rights", json!(-1)),
        ("/entries/0/rights", json!(4294967296_u64)),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert!(
            permissions::validate_windows_acl(&value).is_err(),
            "{pointer}: {value}"
        );
    }
    let mut unknown = original;
    unknown["entries"][0]["other"] = json!(true);
    assert!(permissions::validate_windows_acl(&unknown).is_err());
}

#[test]
fn tool_environment_does_not_inherit_application_credentials_or_proxy() {
    let fixture = Fixture::new();
    let inherited = [
        ("SystemRoot", "system"),
        ("PATH", "tools"),
        ("LC_ALL", "zh_CN"),
        ("APP_DATABASE_PASSWORD", "private-db"),
        ("APP_AUTH_JWT_SECRET", "private-jwt"),
        ("AWS_SECRET_ACCESS_KEY", "private-s3"),
        ("HTTP_PROXY", "private-proxy"),
        ("https_proxy", "private-proxy"),
        ("TEMP", "old-temp"),
        ("TMP", "old-tmp"),
        ("NO_PROXY", "*"),
    ]
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .into_iter()
    .chain(RETIRED_MONITORING_KEYS.into_iter().flat_map(|key| {
        [
            (key.to_owned(), "private-monitoring".to_owned()),
            (key.to_ascii_lowercase(), "private-conflicting".to_owned()),
        ]
    }));
    let value = config::safe_environment_from(&fixture.0, inherited).unwrap();
    assert_eq!(value.len(), 6);
    assert_eq!(value["SystemRoot"], "system");
    assert_eq!(value["PATH"], "tools");
    assert_eq!(value["LC_ALL"], "zh_CN");
    assert_eq!(value["TEMP"], fixture.0.to_string_lossy());
    assert_eq!(value["TMP"], fixture.0.to_string_lossy());
    assert_eq!(value["NO_PROXY"], "127.0.0.1,localhost,::1");
    assert!(!serde_json::to_string(&value).unwrap().contains("private-"));
}

#[test]
fn supervisor_and_logged_tools_reject_project_environment_before_side_effects() {
    use super::recovery::monitoring::supervisor;
    let fixture = Fixture::new();
    let baseline = supervisor::Protocol {
        runtime_directory: fixture.0.clone(),
        role: "webhook".into(),
        scope_id: "restore-scope".into(),
        operation_id: "1".repeat(32),
        command: vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ],
        cwd: fixture.0.clone(),
        environment: config::safe_environment(&fixture.0).unwrap(),
        log: fixture.0.join("unexpected.log"),
    };
    let encoded = baseline.encode().unwrap();
    for key in RETIRED_MONITORING_KEYS
        .into_iter()
        .chain(["CUSTOM_PROJECT_CONFIG", "HTTP_PROXY"])
    {
        for key in [key.to_owned(), key.to_ascii_lowercase()] {
            let mut protocol = baseline.clone();
            protocol
                .environment
                .insert(key.clone(), "private-config-value".into());
            let error = protocol.encode().unwrap_err().to_string();
            assert!(!error.contains("private-config-value"));
            let mut value: Value = serde_json::from_str(&encoded).unwrap();
            value["environment"][&key] = json!("private-config-value");
            assert!(supervisor::Protocol::decode(&value.to_string()).is_err());
            assert!(supervisor::run(&protocol).is_err());
            assert!(
                config::run_logged(
                    protocol.command,
                    &fixture.0,
                    &protocol.environment,
                    &protocol.log,
                    std::time::Duration::from_secs(1)
                )
                .is_err()
            );
            assert!(!protocol.log.exists());
            assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
        }
    }
}

#[test]
fn credential_read_detects_permission_drift_without_exposing_secret() {
    let fixture = Fixture::new();
    let path = fixture.0.join("metrics-token.txt");
    fs::write(&path, "private-metrics-token\n").unwrap();
    let calls = std::cell::Cell::new(0);
    let result = staging::read_credential_with_security(&path, |_| {
        let count = calls.get();
        calls.set(count + 1);
        Ok(json!({"generation": count}))
    });
    let message = result.unwrap_err().to_string();
    assert!(message.contains("权限"));
    assert!(!message.contains("private-metrics-token"));
}

#[test]
fn execution_guard_rechecks_digest_and_reports_both_primary_and_drift_errors() {
    let fixture = Fixture::new();
    let path = fixture.0.join("tool");
    fs::write(&path, b"original").unwrap();
    let guard = ProtectedInputs::open([path.clone()]).unwrap();
    guard.assert_unchanged().unwrap();
    assert_eq!(guard.finish(Ok(17)).unwrap(), 17);
    assert_eq!(
        guard
            .finish::<()>(Err("primary".into()))
            .unwrap_err()
            .to_string(),
        "primary"
    );
    #[cfg(windows)]
    {
        assert!(fs::write(&path, b"changed!").is_err());
        assert!(fs::rename(&path, fixture.0.join("replacement")).is_err());
        assert!(fs::remove_file(&path).is_err());
    }
    #[cfg(unix)]
    {
        fs::write(&path, b"changed!").unwrap();
        let message = guard
            .finish::<()>(Err("primary".into()))
            .unwrap_err()
            .to_string();
        assert!(message.contains("primary"));
        assert!(message.contains("复核同时失败"));
    }
    drop(guard);
    fs::write(&path, b"changed!").unwrap();
}

#[test]
fn execution_guard_rejects_non_file_inputs() {
    let fixture = Fixture::new();
    assert!(ProtectedInputs::open([fixture.0.clone()]).is_err());
    assert!(ProtectedInputs::open([fixture.0.join("absent")]).is_err());
}

#[test]
fn preflight_uses_clean_environment_managed_stdin_and_bounded_capture() {
    use super::recovery::monitoring::preflight;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let local = fixture.0.join(".local-tests");
    fs::create_dir(&local).unwrap();
    let executable = local.join(format!("monitoring-client{}", std::env::consts::EXE_SUFFIX));
    let mut compile = Command::new("rustc");
    compile
        .args([
            "--edition",
            "2024",
            "--crate-name",
            "monitoring_client",
            "-C",
            "debuginfo=0",
        ])
        .arg(root_dir().join("xtask/tests/fixtures/monitoring_client.rs"))
        .arg("-o")
        .arg(&executable);
    if cfg!(windows) {
        compile.args(["-C", "linker=rust-lld.exe"]);
    }
    let output = compile.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let settings = json!({"verification": {"python_program": executable}});
    fs::write(
        local.join("tools.toml"),
        toml::to_string(&settings).unwrap(),
    )
    .unwrap();
    let input = br#"{"scope_id":"stdin-only-scope"}"#;
    let command = preflight::command(&fixture.0).unwrap();
    assert_eq!(
        command.get_program(),
        std::env::current_exe().unwrap().as_os_str()
    );
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        ["__monitoring-preflight"]
    );
    let environment = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_str().unwrap().to_owned(),
                value.unwrap().to_str().unwrap().to_owned(),
            )
        })
        .collect();
    config::validate_environment(&environment).unwrap();
    assert!(!format!("{command:?}").contains("stdin-only-scope"));
    let mut command = Command::new(&executable);
    command
        .env_clear()
        .envs(config::safe_environment(&local).unwrap());
    let before = fs::read_dir(&local).unwrap().count();
    let output =
        preflight::process::capture(&local, command, input, Duration::from_secs(10)).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, input);
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read_dir(&local).unwrap().count(), before);
    for mode in ["failure", "limit", "timeout"] {
        let mut command = Command::new(&executable);
        command
            .arg(mode)
            .env_clear()
            .envs(config::safe_environment(&local).unwrap());
        let started = Instant::now();
        let result = preflight::process::capture(
            &local,
            command,
            input,
            if mode == "timeout" {
                Duration::from_millis(200)
            } else {
                Duration::from_secs(10)
            },
        );
        if mode == "failure" {
            let output = result.unwrap();
            assert_eq!(output.status.code(), Some(7));
            assert!(output.stderr.is_empty());
        } else {
            let error = result.unwrap_err().to_string();
            assert!(!error.contains("private-preflight-secret"));
            assert!(error.contains(if mode == "timeout" { "超时" } else { "1 MiB" }));
        }
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(fs::read_dir(&local).unwrap().count(), before);
    }
}
